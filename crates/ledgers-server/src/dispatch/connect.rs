//! Connected sources over the command surface (issue #142): the connections
//! and allocation commands, and the sync that calls a vendor.
//!
//! Credentials. A token arrives once, in `connections.create` or
//! `connections.update`. This module takes it out of the input, seals it with
//! the server key (`LEDGERS_CONNECT_KEY`, bound to the connection id), and
//! hands only the sealed form to the command. The token is never in the
//! executor's input, the audit log or a response, and the key is never logged.
//!
//! Sync. The server reads the sealed token, calls the vendor through the
//! harness HTTP port (`ledgers_connect::http`: fixed hosts, timeouts, retries),
//! stores the vendor's source documents in the blob store, and then runs
//! `connections.sync_now` with what it got. A refused or failed call is still
//! recorded as a failed run. Everything written to the books goes through the
//! command, so the executor and the policy apply to it like any other write.

use super::{Deps, ServerDispatch};
use crate::routes::Presented;
use cratefield_core::{Clock, Problem};
use ledgers_commands::CommandError;
use ledgers_connect::commands::{
    ConnectionsCreate, ConnectionsDelete, ConnectionsSyncNow, ConnectionsUpdate, invoice_source,
};
use ledgers_connect::connector::{self, SyncContext};
use ledgers_connect::http::{Pause, VendorHttp};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// Waits between vendor retries on the runtime's clock (the same wait the
/// extractor uses).
pub struct ConnectPause(pub Arc<dyn Clock>);

#[async_trait::async_trait]
impl Pause for ConnectPause {
    async fn pause(&self, delay: Duration) {
        // The timeout of a never-ready future is the wait itself.
        let _ =
            cratefield_core::timeout::<()>(self.0.as_ref(), std::future::pending::<()>(), delay)
                .await;
    }
}

fn invalid(message: impl Into<String>, request_id: &str) -> Problem {
    CommandError::InvalidInput(message.into())
        .to_problem()
        .instance(request_id)
}

/// Removes the token from a command input and returns it, refusing an empty one.
fn take_token(input: &mut Value, request_id: &str) -> Result<Option<String>, Problem> {
    let Value::Object(map) = input else {
        return Err(invalid("the request body must be an object", request_id));
    };
    match map.remove("token") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(token)) if !token.trim().is_empty() => Ok(Some(token.trim().to_owned())),
        Some(_) => Err(invalid("token must be a non-empty string", request_id)),
    }
}

fn today_of(clock: &dyn Clock) -> String {
    let now = clock.now();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

impl ServerDispatch {
    /// The sealing key, or the reason the server cannot hold credentials.
    fn sealing_key(
        &self,
        request_id: &str,
    ) -> Result<&ledgers_connect::secret::ConnectKey, Problem> {
        self.connect_key().ok_or_else(|| {
            Problem::internal()
                .with_detail(
                    "LEDGERS_CONNECT_KEY is not set on this server, so connections cannot hold credentials",
                )
                .instance(request_id)
        })
    }

    /// `connections.create`: a person only. Seals the token under the new id.
    pub(super) async fn connections_create(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        mut input: Value,
    ) -> Result<Value, Problem> {
        let caller = self
            .caller_label(deps, presented.as_ref(), request_id)
            .await?;
        let token = take_token(&mut input, request_id)?
            .ok_or_else(|| invalid("a token is required to create a connection", request_id))?;
        let id = deps.id_gen.ulid();
        let sealed = self
            .sealing_key(request_id)?
            .seal(&token, &id)
            .map_err(|_| {
                Problem::internal()
                    .with_detail("the token could not be sealed")
                    .instance(request_id)
            })?;
        drop(token);
        let Value::Object(map) = &mut input else {
            return Err(invalid("the request body must be an object", request_id));
        };
        map.insert("id".to_owned(), json!(id));
        map.insert("sealed_credential".to_owned(), json!(sealed));
        map.insert("created_by".to_owned(), json!(caller));
        // Demo connections are the seeder's alone: a client can never mark one.
        map.insert("demo".to_owned(), json!(false));
        self.execute(deps, &ConnectionsCreate, presented, request_id, input)
            .await
    }

    /// `connections.update`: a person only. A new token is sealed under the
    /// connection id; without one the stored token is kept.
    pub(super) async fn connections_update(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        mut input: Value,
    ) -> Result<Value, Problem> {
        let token = take_token(&mut input, request_id)?;
        let id = input
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("the connection id is required", request_id))?
            .to_owned();
        if let Some(token) = token {
            let sealed = self
                .sealing_key(request_id)?
                .seal(&token, &id)
                .map_err(|_| {
                    Problem::internal()
                        .with_detail("the token could not be sealed")
                        .instance(request_id)
                })?;
            drop(token);
            if let Value::Object(map) = &mut input {
                map.insert("sealed_credential".to_owned(), json!(sealed));
            }
        } else if let Value::Object(map) = &mut input {
            map.remove("sealed_credential");
        }
        self.execute(deps, &ConnectionsUpdate, presented, request_id, input)
            .await
    }

    /// `connections.delete`: a person only. Removes the token and stops syncs.
    pub(super) async fn connections_delete(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        self.execute(deps, &ConnectionsDelete, presented, request_id, input)
            .await
    }

    /// `connections.sync_now`: a person only. Reads the vendor for the window
    /// and records the answer through the command. The window defaults to three
    /// months ending today, or ending with the last complete month for vendors
    /// whose totals are final only after a month closes.
    pub(super) async fn connections_sync_now(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let caller = self
            .caller_label(deps, presented.as_ref(), request_id)
            .await?;
        let connection_id = input
            .get("connection_id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("the connection id is required", request_id))?
            .to_owned();
        let connection =
            ledgers_connect::store::connection(deps.db.as_ref(), self.company_id(), &connection_id)
                .await
                .map_err(|err| {
                    CommandError::Database(err)
                        .to_problem()
                        .instance(request_id)
                })?
                .ok_or_else(|| {
                    invalid(
                        format!("connection {connection_id} does not exist"),
                        request_id,
                    )
                })?;
        let vendor = connector::by_id(&connection.connector).ok_or_else(|| {
            invalid(
                "the connector of this connection is not shipped",
                request_id,
            )
        })?;
        let (default_since, default_until) = connector::default_window(
            &today_of(deps.clock.as_ref()),
            vendor.completed_periods_only(),
        )
        .map_err(|detail| invalid(detail, request_id))?;
        let since = input
            .get("since")
            .and_then(Value::as_str)
            .map_or(default_since, str::to_owned);
        let until = input
            .get("until")
            .and_then(Value::as_str)
            .map_or(default_until, str::to_owned);

        let outcome = self
            .fetch_vendor(
                deps,
                vendor.as_ref(),
                &connection,
                &since,
                &until,
                request_id,
            )
            .await?;
        let mut command_input = json!({
            "connection_id": connection_id,
            "since": since,
            "until": until,
            "created_by": caller,
        });
        match outcome {
            Ok(output) => {
                let stored = self
                    .store_vendor_documents(deps, &output, request_id)
                    .await?;
                command_input["fetched"] = serde_json::to_value(&output).map_err(|err| {
                    Problem::internal()
                        .with_detail(format!("sync answer: {err}"))
                        .instance(request_id)
                })?;
                tracing::info!(
                    invoices = output.invoices.len(),
                    stored,
                    "vendor answered a sync"
                );
            }
            Err(message) => {
                tracing::warn!(connector = %connection.connector, "vendor sync failed");
                command_input["fetch_error"] = json!(message);
            }
        }
        self.execute(
            deps,
            &ConnectionsSyncNow,
            presented,
            request_id,
            command_input,
        )
        .await
    }

    /// Calls the vendor for the window. `Ok` is the vendor's answer, `Err` the
    /// reason for the failed run, with no credential in it.
    async fn fetch_vendor(
        &self,
        deps: &Deps,
        vendor: &dyn connector::Connector,
        connection: &ledgers_connect::store::ConnectionRow,
        since: &str,
        until: &str,
        request_id: &str,
    ) -> Result<Result<ledgers_connect::model::SyncOutput, String>, Problem> {
        if connection.is_demo {
            // A demo connection holds no token: it reads the recorded demo fixture
            // through the same connector, dated to the demo year.
            let year = deps.clock.now().year();
            let demo = ledgers_connect::demo::DemoVendor::factory_zero(year);
            let vendor_http =
                VendorHttp::new(Arc::new(demo), Arc::new(ConnectPause(deps.clock.clone())));
            let config: Value =
                serde_json::from_str(&connection.config_json).unwrap_or(Value::Null);
            let context = SyncContext {
                http: &vendor_http,
                credential: "",
                config: &config,
                since,
                until,
            };
            return Ok(vendor.sync(&context).await.map_err(|err| err.to_string()));
        }
        let Some(sealed) = ledgers_connect::store::sealed_credential(
            deps.db.as_ref(),
            self.company_id(),
            &connection.id,
        )
        .await
        .map_err(|err| {
            CommandError::Database(err)
                .to_problem()
                .instance(request_id)
        })?
        else {
            return Ok(Err("no token is held for this connection".to_owned()));
        };
        let Some(key) = self.connect_key() else {
            return Ok(Err(
                "LEDGERS_CONNECT_KEY is not set on this server".to_owned()
            ));
        };
        let Ok(token) = key.open(&sealed, &connection.id) else {
            return Ok(Err(
                "the stored token cannot be opened for this connection".to_owned()
            ));
        };
        let Some(http) = deps.http.clone() else {
            return Ok(Err("this runtime has no outbound HTTP client".to_owned()));
        };
        if deps.blob.is_none() {
            return Ok(Err(
                "this runtime has no document storage for the vendor's invoices".to_owned(),
            ));
        }
        let config: Value = serde_json::from_str(&connection.config_json).unwrap_or(Value::Null);
        let vendor_http = VendorHttp::new(http, Arc::new(ConnectPause(deps.clock.clone())));
        let context = SyncContext {
            http: &vendor_http,
            credential: &token,
            config: &config,
            since,
            until,
        };
        let outcome = vendor.sync(&context).await.map_err(|err| err.to_string());
        drop(token);
        Ok(outcome)
    }

    /// Stores each invoice's source record in the blob store, keyed by its hash.
    /// Writes are content-addressed, so a repeated sync stores the same bytes.
    async fn store_vendor_documents(
        &self,
        deps: &Deps,
        output: &ledgers_connect::model::SyncOutput,
        request_id: &str,
    ) -> Result<usize, Problem> {
        let Some(blob) = &deps.blob else {
            return Ok(0);
        };
        let mut stored = 0;
        for invoice in &output.invoices {
            let (sha, bytes) = invoice_source(invoice);
            blob.put(
                &crate::documents::blob_key(&sha),
                &bytes,
                "application/json",
            )
            .await
            .map_err(|_| {
                Problem::internal()
                    .with_detail("document storage failed")
                    .instance(request_id)
            })?;
            stored += 1;
        }
        Ok(stored)
    }
}

type DemoPlan<'a> = (&'a str, &'a str, Value, &'a [(&'a str, &'a str)]);

/// The demo connections of the Factory Zero book: a Cloudflare account, the
/// Anthropic organization and a `PlanetScale` organization, each with the
/// allocation rules that map its resources to the book's ventures. They are
/// created and synced through the same commands as a real connection, from
/// recorded fixtures (`ledgers_connect::demo`), with no token.
impl ServerDispatch {
    /// Seeds the demo connections for `year` once. Returns how many were created
    /// (0 when the company already has demo connections).
    ///
    /// # Errors
    /// A sentence naming the step that failed.
    pub async fn seed_demo_connections(&self, deps: &Deps, year: i32) -> Result<usize, String> {
        let existing = ledgers_connect::store::connections(deps.db.as_ref(), self.company_id())
            .await
            .map_err(|err| format!("read connections: {err}"))?;
        if existing.iter().any(|row| row.is_demo) {
            return Ok(0);
        }
        let owner = Presented::Principal(ledgers_commands::auth::Principal::Person {
            person_id: "demo-seed".to_owned(),
            role: ledgers_commands::auth::Role::Owner,
        });
        // (name, connector, config, allocation rules as (pattern, unit)).
        let plan: [DemoPlan<'_>; 3] = [
            (
                "Cloudflare (demo)",
                "cloudflare",
                json!({"account_id": "demo0000000000000000000000000001", "expense_account": "6100", "vat_rate_bp": 0}),
                &[
                    ("fz-ledgers-db", "LDG"),
                    ("sqeezin-db", "SQZ"),
                    ("ledgers-*", "LDG"),
                    ("sqeezin-*", "SQZ"),
                    ("livingbrain-*", "LB"),
                    ("cratefield-*", "CF"),
                    ("owlpost-*", "OWL"),
                    ("colonizer-*", "COL"),
                    ("sealbin-*", "SEAL"),
                    ("fz-*", "FZ"),
                ],
            ),
            (
                "Anthropic (demo)",
                "anthropic",
                json!({"expense_account": "6200", "vat_rate_bp": 0}),
                &[
                    ("apikey_ledgers_web", "LDG"),
                    ("apikey_sqeezin", "SQZ"),
                    ("apikey_cratefield", "CF"),
                    ("apikey_owlpost", "OWL"),
                    ("apikey_fz_research", "FZ"),
                ],
            ),
            (
                "PlanetScale (demo)",
                "planetscale",
                json!({"organization": "factory-zero-demo", "currency": "SGD", "expense_account": "6300", "vat_rate_bp": 0}),
                &[],
            ),
        ];
        let window_since = format!("{year:04}-01-01");
        let window_until = format!("{year:04}-12-31");
        let mut created = 0;
        for (index, (name, connector, config, rules)) in plan.iter().enumerate() {
            let id = deps.id_gen.ulid();
            let request = format!("demo-create-{index}");
            self.execute(
                deps,
                &ConnectionsCreate,
                Some(owner.clone()),
                &request,
                json!({
                    "id": id,
                    "connector": connector,
                    "name": name,
                    "config": config,
                    "default_unit": "FZ",
                    "sealed_credential": "",
                    "created_by": "demo-seed",
                    "demo": true,
                }),
            )
            .await
            .map_err(|problem| format!("create {name}: {}", demo_problem(&problem)))?;
            if !rules.is_empty() {
                let rule_list: Vec<Value> = rules
                    .iter()
                    .map(|(pattern, unit)| json!({"pattern": pattern, "unit_code": unit}))
                    .collect();
                self.execute(
                    deps,
                    &ledgers_connect::commands::AllocationRulesSet,
                    Some(owner.clone()),
                    &format!("demo-rules-{index}"),
                    json!({"connection_id": id, "rules": rule_list}),
                )
                .await
                .map_err(|problem| format!("rules for {name}: {}", demo_problem(&problem)))?;
            }
            self.connections_sync_now(
                deps,
                Some(owner.clone()),
                &format!("demo-sync-{index}"),
                json!({"connection_id": id, "since": window_since, "until": window_until}),
            )
            .await
            .map_err(|problem| format!("sync {name}: {}", demo_problem(&problem)))?;
            created += 1;
        }
        Ok(created)
    }
}

/// The reason a demo step failed, in words a log can carry.
fn demo_problem(problem: &Problem) -> String {
    match &problem.detail {
        Some(detail) => format!("{}: {detail}", problem.title),
        None => problem.title.to_owned(),
    }
}
