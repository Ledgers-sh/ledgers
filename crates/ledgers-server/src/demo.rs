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
use cratefield_core::{Database, constant_time_eq};
use ledgers_commands::auth::{Authenticator, Credential, Limit, Limits, Principal, Tier};
use ledgers_commands::error::CommandError;
use ledgers_commands::{CommandContext, Execute, Executor};
use ledgers_demo::{DemoBook, DemoEntry, DemoLine, EntryKind, Poster};
use ledgers_ledger::query;
use serde_json::{Value, json};

/// The person id the in-process seeding posts as. The company's owner
/// records it on the audit events of person entries.
const DEMO_OWNER_ID: &str = "demo-owner";

/// The key id the internal seeding agent is recorded under on audit events.
/// No key with this id is issued or stored.
const INTERNAL_AGENT_KEY_ID: &str = "demo-seed";

/// The per-entry ceiling of the internal agent, the same generous ceiling the
/// dev bootstrap grants its keys.
const INTERNAL_AGENT_CEILING: i128 = 100_000_000_000_000;

/// Who posts the agent entries of a seeding run.
#[derive(Clone, Copy, Debug)]
pub enum AgentPoster<'a> {
    /// An agent key of the company, presented as a bearer token: the key is
    /// verified and the policy decides as it does for any agent.
    Key(&'a str),
    /// The seeding path's internal agent principal, with the post-within-limits
    /// tier. No key is issued, stored or presented; the server vouches for it
    /// in-process only (the deployed `POST /admin/seed-demo` route).
    Internal,
}

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
    seed_as(deps, company_id, AgentPoster::Key(agent_key), book).await
}

/// Seeds `book` into `company_id` as the internal agent principal, with no
/// agent key. This is the loader behind `POST /admin/seed-demo`; the caller
/// must have passed [`seed_demo_gate`] first.
///
/// # Errors
/// As [`seed`].
pub async fn seed_internal(
    deps: &Deps,
    company_id: &str,
    book: &DemoBook,
) -> Result<SeedReport, String> {
    seed_as(deps, company_id, AgentPoster::Internal, book).await
}

/// The loader both paths share: dimensions first, then every entry in posting
/// order, skipping what is already in the book.
///
/// # Errors
/// As [`seed`].
pub async fn seed_as(
    deps: &Deps,
    company_id: &str,
    poster: AgentPoster<'_>,
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
                        let presented = agent_presented(poster, book.currency);
                        post_as_agent(deps, &agent, company_id, presented, &input)
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
                        let presented = agent_presented(poster, book.currency);
                        reverse_as_agent(deps, &agent, company_id, presented, &input)
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

/// How the agent entries present themselves: the key as a bearer token, or
/// the internal principal in the book's currency.
fn agent_presented(poster: AgentPoster<'_>, currency: &str) -> Presented {
    match poster {
        AgentPoster::Key(key) => Presented::Bearer(key.to_owned()),
        AgentPoster::Internal => Presented::Principal(internal_agent(currency)),
    }
}

/// The internal seeding agent: post-within-limits, with a ceiling in the
/// book's currency and nothing else. It exists only in memory.
fn internal_agent(currency: &str) -> Principal {
    let limits = Limits::new()
        .insert(
            currency,
            Limit {
                base_ceiling: INTERNAL_AGENT_CEILING,
                transaction_ceiling: INTERNAL_AGENT_CEILING,
            },
        )
        .unwrap_or_default();
    Principal::Agent {
        key_id: INTERNAL_AGENT_KEY_ID.to_owned(),
        tier: Tier::PostWithinLimits,
        limits,
    }
}

async fn post_as_agent(
    deps: &Deps,
    agent: &ServerDispatch,
    company_id: &str,
    presented: Presented,
    input: &EntriesPostInput,
) -> Result<(), String> {
    let value = serde_json::to_value(input).map_err(|err| err.to_string())?;
    let outcome = agent
        .dispatch(
            deps,
            "entries.post",
            value,
            &request_id(deps),
            Some(presented),
        )
        .await
        .map_err(|problem| problem.to_string())?;
    if outcome.get("status").and_then(Value::as_str) == Some("needs-approval") {
        return Err(format!(
            "the agent needs a person to approve this entry in {company_id}"
        ));
    }
    Ok(())
}

async fn reverse_as_agent(
    deps: &Deps,
    agent: &ServerDispatch,
    company_id: &str,
    presented: Presented,
    input: &EntriesReverseInput,
) -> Result<(), String> {
    let value = serde_json::to_value(input).map_err(|err| err.to_string())?;
    let outcome = agent
        .dispatch(
            deps,
            "entries.reverse",
            value,
            &request_id(deps),
            Some(presented),
        )
        .await
        .map_err(|problem| problem.to_string())?;
    if outcome.get("status").and_then(Value::as_str) == Some("needs-approval") {
        return Err(format!(
            "the agent needs a person to approve the reversal in {company_id}"
        ));
    }
    Ok(())
}

/// What `POST /admin/seed-demo` answers before it reads or writes anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedGate {
    /// Every check passed: the seed may run.
    Open,
    /// `SEED_TOKEN` is unset or empty: the route does not exist (404).
    NotFound,
    /// The bearer token is missing or wrong (401).
    Unauthorized,
    /// `LEDGERS_ALLOW_DEMO_SEED` is not `"1"` (403).
    Forbidden,
}

impl SeedGate {
    /// The HTTP status the refusal answers with, `None` when open.
    #[must_use]
    pub fn status(self) -> Option<u16> {
        match self {
            SeedGate::Open => None,
            SeedGate::NotFound => Some(404),
            SeedGate::Unauthorized => Some(401),
            SeedGate::Forbidden => Some(403),
        }
    }
}

/// The checks of `POST /admin/seed-demo`, in order, mirroring the bootstrap
/// route: the route is absent while the secret is unset (404), a wrong bearer
/// token is refused (401), and the demo also needs `LEDGERS_ALLOW_DEMO_SEED=1`,
/// which is never set in production (403).
#[must_use]
pub fn seed_demo_gate(secret: Option<&str>, presented: &str, allow: Option<&str>) -> SeedGate {
    let Some(secret) = secret.filter(|value| !value.is_empty()) else {
        return SeedGate::NotFound;
    };
    if !constant_time_eq(presented.as_bytes(), secret.as_bytes()) {
        return SeedGate::Unauthorized;
    }
    if allow != Some("1") {
        return SeedGate::Forbidden;
    }
    SeedGate::Open
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

/// What verifying the seeded company checks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// Entries posted and now verified.
    pub entries_verified: usize,
    /// Reversals posted and now verified.
    pub reversals_verified: usize,
    /// Fixture entries found and checked.
    pub fixture_entries_found: usize,
    /// Fixture reversals found reversed.
    pub fixture_reversals_found: usize,
}

/// Verifies that the seeded company passes every invariant check.
///
/// # Errors
/// A message naming the first broken invariant if any check fails.
pub async fn check(deps: &Deps, company_id: &str, book: &DemoBook) -> Result<CheckReport, String> {
    let db = deps.db.as_ref();
    let mut report = CheckReport::default();

    // Check 1: audit chain verifies
    ledgers_audit::verify_db(db, company_id)
        .await
        .map_err(|err| format!("audit chain: {err}"))?;

    // Check 2: every posted entry's lines balance in base currency
    check_entry_balances(db, company_id, book.currency).await?;

    // Check 3: entry numbers are gapless per journal and fiscal year
    check_numbering_gaps(db, company_id).await?;

    // Check 4: trial balance debits == credits for the year
    let year = deps.clock.now().year();
    check_trial_balance(db, company_id, year).await?;

    // Check 5: balance sheet balances
    check_balance_sheet(db, company_id, year).await?;

    // Check 6: every fixture entry is present and reversals are reversed
    for entry in &book.entries {
        match &entry.kind {
            EntryKind::Post { .. } => {
                let found = query::entry_by_external_id(db, company_id, &entry.external_id)
                    .await
                    .map_err(|err| format!("read {}: {err}", entry.external_id))?;
                if found.is_none() {
                    return Err(format!("fixture entry {} is not posted", entry.external_id));
                }
                report.fixture_entries_found += 1;
            }
            EntryKind::Reverse { of } => {
                // Verify the target entry is posted and has been reversed
                let target = query::entry_by_external_id(db, company_id, of)
                    .await
                    .map_err(|err| format!("read {of}: {err}"))?
                    .ok_or_else(|| {
                        format!(
                            "fixture reversal {} target {} is not posted",
                            entry.external_id, of
                        )
                    })?;
                if target.reversed_by.is_empty() {
                    return Err(format!(
                        "fixture reversal {} does not reverse {}",
                        entry.external_id, of
                    ));
                }
                report.fixture_reversals_found += 1;
            }
        }
    }

    report.entries_verified = count_posted_entries(db, company_id)
        .await
        .map_err(|err| format!("count posted entries: {err}"))?;
    report.reversals_verified = count_reversals(db, company_id)
        .await
        .map_err(|err| format!("count reversals: {err}"))?;

    Ok(report)
}

/// Count all posted (non-reversed) journal entries.
async fn count_posted_entries(
    db: &dyn Database,
    company_id: &str,
) -> Result<usize, cratefield_core::DbError> {
    let rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT COUNT(*) AS cnt FROM journal_entry WHERE company_id = ? AND reverses IS NULL"
                .to_owned(),
            vec![sea_query::Value::from(company_id)],
        ))
        .await?;
    Ok(rows
        .rows
        .first()
        .and_then(|r| r.get("cnt"))
        .unwrap_or(0usize))
}

/// Count all reversed journal entries.
async fn count_reversals(
    db: &dyn Database,
    company_id: &str,
) -> Result<usize, cratefield_core::DbError> {
    let rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT COUNT(*) AS cnt FROM journal_entry WHERE company_id = ? AND reverses IS NOT NULL"
                .to_owned(),
            vec![sea_query::Value::from(company_id)],
        ))
        .await?;
    Ok(rows
        .rows
        .first()
        .and_then(|r| r.get("cnt"))
        .unwrap_or(0usize))
}

/// Verify every entry's lines sum to zero in base currency.
async fn check_entry_balances(
    db: &dyn Database,
    company_id: &str,
    _currency: &str,
) -> Result<(), String> {
    let rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT entry_id FROM journal_line \
             WHERE entry_id IN (SELECT id FROM journal_entry WHERE company_id = ?) \
             GROUP BY entry_id HAVING SUM(base_minor) != 0"
                .to_owned(),
            vec![sea_query::Value::from(company_id)],
        ))
        .await
        .map_err(|err| format!("read unbalanced entries: {err}"))?;

    if !rows.rows.is_empty() {
        return Err("entry does not balance in base currency".to_owned());
    }
    Ok(())
}

/// Verify numbering is gapless per journal and fiscal year.
async fn check_numbering_gaps(db: &dyn Database, company_id: &str) -> Result<(), String> {
    let rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT journal_id, fiscal_year FROM journal_entry \
             WHERE company_id = ? \
             GROUP BY journal_id, fiscal_year HAVING COUNT(*) != MAX(number)"
                .to_owned(),
            vec![sea_query::Value::from(company_id)],
        ))
        .await
        .map_err(|err| format!("check numbering: {err}"))?;

    if !rows.rows.is_empty() {
        return Err("journal has gaps in numbering".to_owned());
    }
    Ok(())
}

/// Verify trial balance debits == credits.
async fn check_trial_balance(db: &dyn Database, company_id: &str, year: i32) -> Result<(), String> {
    let period_id = format!("{year}-12-31");
    let tb = ledgers_ledger::report::trial_balance(db, company_id, &period_id)
        .await
        .map_err(|err| format!("trial balance: {err}"))?;

    let (mut debits_minor, mut credits_minor) = (0i128, 0i128);
    for row in tb {
        let balance_minor = row.balance.minor();
        if balance_minor >= 0 {
            debits_minor += balance_minor;
        } else {
            credits_minor += -balance_minor;
        }
    }

    if debits_minor != credits_minor {
        return Err("trial balance does not balance".to_owned());
    }
    Ok(())
}

/// Verify balance sheet balances.
async fn check_balance_sheet(db: &dyn Database, company_id: &str, year: i32) -> Result<(), String> {
    let as_of = format!("{year}-12-31");
    let bs = ledgers_ledger::report::balance_sheet(db, company_id, &as_of)
        .await
        .map_err(|err| format!("balance sheet: {err}"))?;

    if !bs.balanced {
        return Err("balance sheet does not balance".to_owned());
    }
    Ok(())
}
