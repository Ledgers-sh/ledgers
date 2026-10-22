//! Seeding the demo company (issue #51): Halden Works `GmbH`, from
//! `ledgers-demo`, posted through the same executor path as real commands.
//!
//! - Agent entries go through [`ServerDispatch`] with the agent key's own
//!   bearer token, so the key is verified and the policy decides as it does
//!   for any agent.
//! - Person entries go through the [`Executor`] under a person principal,
//!   the way a person session posts them (`ledgers-commands`).
//! - Dimension values are created with the store's domain function.
//!
//! Idempotent: an entry whose `external_id` already exists is skipped, a
//! reversal whose target is already reversed is skipped, and a dimension
//! value that exists is left as it is. Running it twice posts nothing new.

use crate::dispatch::{
    self, Deps, EntriesPost, EntriesPostInput, EntriesReverse, EntriesReverseInput, ServerDispatch,
};
use crate::routes::{Dispatch, Presented};
use async_trait::async_trait;
use cratefield_core::Database;
use ledgers_commands::auth::{Authenticator, Credential, Principal};
use ledgers_commands::error::CommandError;
use ledgers_commands::{CommandContext, Execute, Executor};
use ledgers_demo::{DemoBook, DemoEntry, DemoLine, EntryKind, Poster};
use ledgers_ledger::query;
use serde_json::{Value, json};

/// The person id the in-process seeding posts as. The company's owner
/// records it on the audit events of person entries.
const DEMO_OWNER_ID: &str = "demo-owner";

/// What a seeding run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeedReport {
    /// Entries posted by this run.
    pub posted: usize,
    /// Reversals posted by this run.
    pub reversed: usize,
    /// Entries that were already in the book and were left alone.
    pub skipped: usize,
    /// Dimension values created by this run.
    pub dimensions_created: usize,
}

/// The authenticator for a person posting in-process: the owner principal.
/// It is used only by this seeding path, which runs on the server itself and
/// never on a request.
struct OwnerAuth {
    person_id: String,
}

#[async_trait]
impl Authenticator for OwnerAuth {
    async fn authenticate(&self, _credential: &Credential) -> Result<Principal, CommandError> {
        Ok(Principal::Person {
            person_id: self.person_id.clone(),
        })
    }
}

/// Seeds `book` into `company_id`. `agent_key` is a full-tier agent key of
/// that company (the secret, as issued); the agent entries post with it.
///
/// # Errors
/// A message naming the entry that failed. Entries posted before the failure
/// stay posted; running again continues where it stopped.
pub async fn seed(
    deps: &Deps,
    company_id: &str,
    agent_key: &str,
    book: &DemoBook,
) -> Result<SeedReport, String> {
    let mut report = SeedReport::default();
    let db = deps.db.as_ref();
    let stamp = dispatch::rfc3339_utc(deps.clock.now());

    for dimension in &book.dimensions {
        let exists = dimension_exists(db, company_id, dimension.kind, dimension.code)
            .await
            .map_err(|err| format!("read dimension {}: {err}", dimension.code))?;
        if !exists {
            ledgers_ledger::store::create_dimension_value(
                db,
                company_id,
                dimension.kind,
                dimension.code,
                dimension.name,
                false,
                &stamp,
            )
            .await
            .map_err(|err| format!("create dimension {}: {err}", dimension.code))?;
            report.dimensions_created += 1;
        }
    }

    let agent = ServerDispatch::new(company_id);
    for entry in &book.entries {
        match &entry.kind {
            EntryKind::Post { lines } => {
                if query::entry_by_external_id(db, company_id, &entry.external_id)
                    .await
                    .map_err(|err| format!("read {}: {err}", entry.external_id))?
                    .is_some()
                {
                    report.skipped += 1;
                    continue;
                }
                let input = post_input(book.currency, entry, lines);
                match entry.poster {
                    Poster::Agent => {
                        post_as_agent(deps, &agent, company_id, agent_key, &input)
                            .await
                            .map_err(|err| format!("{}: {err}", entry.external_id))?;
                    }
                    Poster::Person => {
                        post_as_person(deps, company_id, &input)
                            .await
                            .map_err(|err| format!("{}: {err}", entry.external_id))?;
                    }
                }
                report.posted += 1;
            }
            EntryKind::Reverse { of } => {
                let target = query::entry_by_external_id(db, company_id, of)
                    .await
                    .map_err(|err| format!("read {of}: {err}"))?
                    .ok_or_else(|| format!("reversal {}: {of} is not posted", entry.external_id))?;
                if !target.reversed_by.is_empty() {
                    report.skipped += 1;
                    continue;
                }
                let input = EntriesReverseInput {
                    external_id: of.clone(),
                    posting_date: Some(entry.posting_date.clone()),
                };
                match entry.poster {
                    Poster::Agent => {
                        reverse_as_agent(deps, &agent, company_id, agent_key, &input)
                            .await
                            .map_err(|err| format!("reverse {of}: {err}"))?;
                    }
                    Poster::Person => {
                        reverse_as_person(deps, company_id, &input)
                            .await
                            .map_err(|err| format!("reverse {of}: {err}"))?;
                    }
                }
                report.reversed += 1;
            }
        }
    }
    Ok(report)
}

/// Development auto-load (`LEDGERS_DEV=1`): when the company has no entries,
/// seeds the Halden Works fixture for the current fiscal year. A company with
/// any entry is left as it is.
///
/// # Errors
/// As [`seed`], or a failed read of the entries.
pub async fn seed_if_empty(
    deps: &Deps,
    company_id: &str,
    agent_key: &str,
) -> Result<Option<SeedReport>, String> {
    let existing = query::list_entries(deps.db.as_ref(), company_id, None, 1)
        .await
        .map_err(|err| format!("read entries: {err}"))?;
    if !existing.is_empty() {
        return Ok(None);
    }
    let year = deps.clock.now().year();
    let book = ledgers_demo::halden::book(year);
    seed(deps, company_id, agent_key, &book).await.map(Some)
}

/// Whether the company already has the dimension value.
async fn dimension_exists(
    db: &dyn Database,
    company_id: &str,
    kind: &str,
    code: &str,
) -> Result<bool, cratefield_core::DbError> {
    let rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT 1 AS found FROM dimension_value WHERE company_id = ? AND kind = ? AND code = ?"
                .to_owned(),
            vec![
                sea_query::Value::from(company_id),
                sea_query::Value::from(kind),
                sea_query::Value::from(code),
            ],
        ))
        .await?;
    Ok(!rows.rows.is_empty())
}

/// The `entries.post` input of one demo entry, as the entries API reads it.
fn post_input(currency: &str, entry: &DemoEntry, lines: &[DemoLine]) -> EntriesPostInput {
    let lines: Vec<Value> = lines
        .iter()
        .map(|line| {
            let dimensions: serde_json::Map<String, Value> = line
                .dimensions
                .iter()
                .map(|(kind, code)| ((*kind).to_owned(), json!(code)))
                .collect();
            json!({
                "account_number": line.account,
                "side": line.side.wire(),
                "base": { "minor": line.minor.to_string(), "currency": currency },
                "fx": null,
                "tax_code": line.tax_code,
                "dimensions": dimensions,
                "memo": line.memo,
            })
        })
        .collect();
    let input = json!({
        "journal_id": null,
        "fiscal_year": null,
        "entry": {
            "journal_id": "",
            "document_date": entry.posting_date,
            "posting_date": entry.posting_date,
            "tax_point_date": null,
            "base_currency": currency,
            "description": entry.description,
            "source_documents": [],
            "external_id": entry.external_id,
            "source_app": null,
            "reverses": null,
            "lines": lines,
        }
    });
    serde_json::from_value(input).expect("the demo entry matches the entries API schema")
}

async fn post_as_agent(
    deps: &Deps,
    agent: &ServerDispatch,
    company_id: &str,
    agent_key: &str,
    input: &EntriesPostInput,
) -> Result<(), String> {
    let value = serde_json::to_value(input).map_err(|err| err.to_string())?;
    let outcome = agent
        .dispatch(
            deps,
            "entries.post",
            value,
            &request_id(deps),
            Some(Presented::Bearer(agent_key.to_owned())),
        )
        .await
        .map_err(|problem| problem.to_string())?;
    if outcome.get("status").and_then(Value::as_str) == Some("needs-approval") {
        return Err(format!(
            "the agent key needs a person to approve this entry in {company_id}"
        ));
    }
    Ok(())
}

async fn reverse_as_agent(
    deps: &Deps,
    agent: &ServerDispatch,
    company_id: &str,
    agent_key: &str,
    input: &EntriesReverseInput,
) -> Result<(), String> {
    let value = serde_json::to_value(input).map_err(|err| err.to_string())?;
    let outcome = agent
        .dispatch(
            deps,
            "entries.reverse",
            value,
            &request_id(deps),
            Some(Presented::Bearer(agent_key.to_owned())),
        )
        .await
        .map_err(|problem| problem.to_string())?;
    if outcome.get("status").and_then(Value::as_str) == Some("needs-approval") {
        return Err(format!(
            "the agent key needs a person to approve the reversal in {company_id}"
        ));
    }
    Ok(())
}

/// Posts one entry as the company's owner, through the executor under a
/// person principal, the way a person session posts. Used by the seeding and
/// by tests that need a person-posted entry of their own.
///
/// # Errors
/// The executor's refusal, as text.
pub async fn post_as_owner(
    deps: &Deps,
    company_id: &str,
    currency: &str,
    entry: &DemoEntry,
) -> Result<(), String> {
    let lines = match &entry.kind {
        EntryKind::Post { lines } => lines,
        EntryKind::Reverse { .. } => {
            return Err("a reversal is not posted as a new entry".to_owned());
        }
    };
    let input = post_input(currency, entry, lines);
    post_as_person(deps, company_id, &input).await
}

async fn post_as_person(
    deps: &Deps,
    company_id: &str,
    input: &EntriesPostInput,
) -> Result<(), String> {
    let auth = OwnerAuth {
        person_id: DEMO_OWNER_ID.to_owned(),
    };
    run_as_person(deps, company_id, &EntriesPost, auth, input).await
}

async fn reverse_as_person(
    deps: &Deps,
    company_id: &str,
    input: &EntriesReverseInput,
) -> Result<(), String> {
    let auth = OwnerAuth {
        person_id: DEMO_OWNER_ID.to_owned(),
    };
    run_as_person(deps, company_id, &EntriesReverse, auth, input).await
}

/// Runs one command through the executor under the person principal.
async fn run_as_person<C>(
    deps: &Deps,
    company_id: &str,
    command: &C,
    auth: OwnerAuth,
    input: &C::Input,
) -> Result<(), String>
where
    C: ledgers_commands::Command + Sync,
    C::Input: Sync,
{
    let policy = dispatch::load_policy(deps.db.as_ref(), company_id, deps.clock.now())
        .await
        .map_err(|err| err.to_string())?;
    let ctx = CommandContext {
        company_id: company_id.to_owned(),
        request_id: request_id(deps),
        event_id: deps.id_gen.ulid(),
        now: dispatch::rfc3339_utc(deps.clock.now()),
    };
    let executor = Executor::new(deps.db.as_ref(), auth, policy);
    executor
        .run(command, DEMO_OWNER_ID.to_owned(), ctx, input, Execute::Run)
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn request_id(deps: &Deps) -> String {
    format!("demo-{}", deps.id_gen.ulid())
}
