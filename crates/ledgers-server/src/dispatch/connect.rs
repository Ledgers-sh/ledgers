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

/// Removes the credential from a command input and returns the text to seal.
/// A connector with one field takes `token` (a string). One with several takes
/// `credential`, an object holding exactly its fields, which is sealed as JSON
/// text. An unknown field is refused, so a stray secret cannot be stored.
fn take_credential(
    input: &mut Value,
    connector: &dyn connector::Connector,
    request_id: &str,
) -> Result<Option<String>, Problem> {
    let fields = connector.credential_fields();
    if fields.len() == 1 && fields[0].name == "token" {
        return take_token(input, request_id);
    }
    let Value::Object(map) = input else {
        return Err(invalid("the request body must be an object", request_id));
    };
    if map.remove("token").is_some() {
        return Err(invalid(
            format!(
                "{} takes its credential fields, not a token",
                connector.display_name()
            ),
            request_id,
        ));
    }
    match map.remove("credential") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(given)) => {
            let mut sealed = serde_json::Map::new();
            for field in fields {
                let text = given
                    .get(field.name)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| {
                        invalid(format!("the credential needs {}", field.name), request_id)
                    })?;
                sealed.insert(field.name.to_owned(), json!(text));
            }
            if given.len() != sealed.len() {
                return Err(invalid(
                    format!(
                        "the credential of {} has only: {}",
                        connector.display_name(),
                        fields.iter().map(|f| f.name).collect::<Vec<_>>().join(", ")
                    ),
                    request_id,
                ));
            }
            serde_json::to_string(&Value::Object(sealed))
                .map(Some)
                .map_err(|_| invalid("the credential could not be read", request_id))
        }
        Some(_) => Err(invalid("credential must be an object", request_id)),
    }
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

/// The current instant in UTC, RFC 3339 (`2026-10-24T10:00:00Z`).
fn now_rfc3339(clock: &dyn Clock) -> String {
    let now = clock.now().to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
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
        let connector_id = input
            .get("connector")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("a connector is required", request_id))?
            .to_owned();
        let vendor = connector::by_id(&connector_id).ok_or_else(|| {
            invalid(
                format!("{connector_id} is not a connector Ledgers ships"),
                request_id,
            )
        })?;
        let token = take_credential(&mut input, vendor.as_ref(), request_id)?.ok_or_else(|| {
            invalid(
                "a credential is required to create a connection",
                request_id,
            )
        })?;
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
        let suggested = suggested_rule_for(&input);
        let created = self
            .execute(
                deps,
                &ConnectionsCreate,
                presented.clone(),
                request_id,
                input,
            )
            .await?;
        if let Some(rule) = suggested {
            // A suggestion, not a mapping: the rule is proposed by the connector and
            // waits in Bank > Rules until a person enables it.
            let request = ledgers_bank::model::RulesCreateRequest {
                name: rule.name,
                conditions: rule.conditions,
                account_number: rule.account_number,
                tax_code: None,
                unit: None,
            };
            let value = serde_json::to_value(ledgers_bank::commands::rule_request_to_input(
                request,
                deps.id_gen.ulid(),
                format!("connector:{}", rule.connector),
                false,
            ))
            .map_err(|err| {
                Problem::internal()
                    .with_detail(format!("suggested rule: {err}"))
                    .instance(request_id)
            })?;
            self.execute(
                deps,
                &ledgers_bank::commands::RulesCreate,
                presented,
                &format!("{request_id}-rule"),
                value,
            )
            .await?;
        }
        Ok(created)
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
        let id = input
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("the connection id is required", request_id))?
            .to_owned();
        let existing = ledgers_connect::store::connection(deps.db.as_ref(), self.company_id(), &id)
            .await
            .map_err(|err| {
                CommandError::Database(err)
                    .to_problem()
                    .instance(request_id)
            })?
            .ok_or_else(|| invalid(format!("connection {id} does not exist"), request_id))?;
        let vendor = connector::by_id(&existing.connector).ok_or_else(|| {
            invalid(
                "the connector of this connection is not shipped",
                request_id,
            )
        })?;
        let token = take_credential(&mut input, vendor.as_ref(), request_id)?;
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
    #[allow(clippy::too_many_lines)]
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
        // Planned journal entries (ledger connectors) are posted one at a time, each in
        // its own batch, before the sync records its run. A failure is recorded as the
        // run's failure; entries already posted are found again by a later sync.
        let posting = match &outcome {
            Ok(output) if !output.entries.is_empty() => Some(
                self.post_planned_entries(
                    deps,
                    presented.clone(),
                    request_id,
                    &connection,
                    &output.entries,
                )
                .await,
            ),
            _ => None,
        };
        match outcome {
            Ok(_) if matches!(posting, Some(Err(_))) => {
                if let Some(Err(message)) = posting {
                    command_input["fetch_error"] = json!(message);
                }
            }
            Ok(output) => {
                if let Some(Ok(links)) = posting {
                    command_input["ledger_links"] = serde_json::to_value(links).map_err(|err| {
                        Problem::internal()
                            .with_detail(format!("ledger links: {err}"))
                            .instance(request_id)
                    })?;
                }
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

    /// `connections.test`: a person only. Reads the vendor for today with the
    /// stored credential and reports whether it answers. Nothing is written: no
    /// run, no bill, no usage.
    pub(super) async fn connections_test(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        self.caller_label(deps, presented.as_ref(), request_id)
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
        let today = today_of(deps.clock.as_ref());
        let outcome = self
            .fetch_vendor(
                deps,
                vendor.as_ref(),
                &connection,
                &today,
                &today,
                request_id,
            )
            .await?;
        let output = match outcome {
            Ok(answer) => ledgers_connect::commands::TestOutput {
                ok: true,
                notes: answer.notes,
                error: None,
            },
            Err(message) => ledgers_connect::commands::TestOutput {
                ok: false,
                notes: Vec::new(),
                error: Some(message),
            },
        };
        serde_json::to_value(output).map_err(|err| {
            Problem::internal()
                .with_detail(format!("test answer: {err}"))
                .instance(request_id)
        })
    }

    /// Posts each planned journal entry that is not yet posted, and returns the link of
    /// every planned entry to its journal entry. An entry whose `external_id` already
    /// exists in the ledger (a sync that stopped after posting) is linked, not posted
    /// again. Units come from the connection's allocation rules, by the entry line's
    /// resource, else the connection's default unit.
    async fn post_planned_entries(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        connection: &ledgers_connect::store::ConnectionRow,
        entries: &[ledgers_connect::model::PlannedEntry],
    ) -> Result<Vec<ledgers_connect::commands::LedgerLink>, String> {
        let company = self.company_id();
        let journal = ledgers_ledger::journal::find_journal(deps.db.as_ref(), company, "GN")
            .await
            .map_err(|err| format!("the general journal could not be read: {err}"))?
            .ok_or_else(|| "no general journal GN exists to post the entries to".to_owned())?
            .id;
        let rules = ledgers_connect::store::rules(deps.db.as_ref(), company, &connection.id)
            .await
            .map_err(|err| format!("allocation rules could not be read: {err}"))?;
        let mut links = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if !entry.balances() {
                return Err(format!(
                    "{} does not balance and was not posted",
                    entry.provider_id
                ));
            }
            let existing = existing_entry(deps.db.as_ref(), company, &entry.provider_id)
                .await
                .map_err(|err| format!("the ledger could not be read: {err}"))?;
            let entry_id = if let Some(id) = existing {
                id
            } else {
                let draft = entry_draft(entry, &journal, &rules, &connection.default_unit);
                let fiscal_year: i64 = entry
                    .posting_date
                    .get(..4)
                    .and_then(|y| y.parse().ok())
                    .ok_or_else(|| format!("{} has no posting year", entry.provider_id))?;
                let input =
                    json!({"journal_id": journal, "fiscal_year": fiscal_year, "entry": draft});
                let answer = self
                    .execute(
                        deps,
                        &ledgers_ledger::surface::PostEntry,
                        presented.clone(),
                        &format!("{request_id}-ledger-{index}"),
                        input,
                    )
                    .await
                    .map_err(|problem| {
                        format!(
                            "{} was not posted: {}",
                            entry.provider_id,
                            demo_problem(&problem)
                        )
                    })?;
                answer
                    .pointer("/entry/entry_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{} posted without an entry id", entry.provider_id))?
            };
            links.push(ledgers_connect::commands::LedgerLink {
                provider_id: entry.provider_id.clone(),
                entry_id,
                posting_date: entry.posting_date.clone(),
            });
        }
        Ok(links)
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
            let now = now_rfc3339(deps.clock.as_ref());
            let context = SyncContext {
                http: &vendor_http,
                credential: "",
                now: &now,
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
        let now = now_rfc3339(deps.clock.as_ref());
        let context = SyncContext {
            http: &vendor_http,
            credential: &token,
            now: &now,
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

/// The id of the journal entry already posted under `external_id`, if any.
async fn existing_entry(
    db: &dyn cratefield_core::Database,
    company_id: &str,
    external_id: &str,
) -> Result<Option<String>, cratefield_core::DbError> {
    let rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT id FROM journal_entry WHERE company_id = ? AND external_id = ?".to_owned(),
            vec![
                sea_query::Value::from(company_id),
                sea_query::Value::from(external_id),
            ],
        ))
        .await?;
    Ok(rows.rows.first().and_then(|row| row.get::<String>("id")))
}

/// The journal entry draft for a planned entry. Each line carries its unit as the
/// `unit` dimension: the allocation rule for its resource, else the default unit.
fn entry_draft(
    entry: &ledgers_connect::model::PlannedEntry,
    journal_id: &str,
    rules: &[ledgers_connect::allocation::AllocationRule],
    default_unit: &str,
) -> Value {
    let lines: Vec<Value> = entry
        .lines
        .iter()
        .map(|line| {
            let unit = line
                .resource
                .as_deref()
                .and_then(|resource| ledgers_connect::allocation::unit_for(rules, resource))
                .unwrap_or(default_unit);
            json!({
                "account_number": line.account,
                "side": if line.debit { "debit" } else { "credit" },
                "base": {"minor": line.minor.to_string(), "currency": entry.currency},
                "fx": null,
                "tax_code": null,
                "dimensions": {"unit": unit},
                "memo": null,
            })
        })
        .collect();
    json!({
        "journal_id": journal_id,
        "document_date": entry.posting_date,
        "posting_date": entry.posting_date,
        "tax_point_date": null,
        "base_currency": entry.currency,
        "description": entry.description,
        "source_documents": [],
        "external_id": entry.provider_id,
        "source_app": "ledgers-connect",
        "reverses": null,
        "lines": lines,
    })
}

/// A rule the connector suggests when it is created: the bank line of a payout
/// from the provider, which then books to the connection's clearing account.
struct SuggestedRule {
    connector: &'static str,
    name: String,
    conditions: ledgers_bank::rules::RuleConditions,
    account_number: String,
}

/// The suggested bank rule for a new connection, when its connector has one.
/// Stripe's payouts arrive as inflows whose description names Stripe.
fn suggested_rule_for(input: &Value) -> Option<SuggestedRule> {
    let connector = input.get("connector").and_then(Value::as_str)?;
    if connector != "stripe" {
        return None;
    }
    let clearing = input
        .pointer("/config/clearing_account")
        .and_then(Value::as_str)?;
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Stripe")
        .to_owned();
    Some(SuggestedRule {
        connector: "stripe",
        name: format!("{name}: payouts (suggested)"),
        conditions: ledgers_bank::rules::RuleConditions {
            contains: Some("stripe".to_owned()),
            counterparty_iban: None,
            amount_min: Some(1),
            amount_max: None,
        },
        account_number: clearing.to_owned(),
    })
}
