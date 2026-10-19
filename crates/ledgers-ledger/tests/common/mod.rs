#![allow(dead_code)]
//! Shared scaffolding for the ledgers-ledger integration tests: the
//! invented company fixtures (AGENTS.md — never a real firm), the
//! scenario chart, and the small read helpers every matrix file
//! asserts with.

use cratefield_core::{Database, Statement};
use ledgers_audit::{Actor, AuditContext};
use ledgers_ledger::LedgerModule;
use ledgers_ledger::account::{AccountDraft, AccountType};
use ledgers_ledger::commands;
use sea_query::Value as SeaValue;

/// The single-company fixture every scenario posts for.
pub const COMPANY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
/// A second company, for the cross-company refusals: its journal and
/// entries must be invisible to the first company's posts.
pub const OTHER_COMPANY_ID: &str = "01OTHERCOMPANY000000000000A";
pub const NOW: &str = "2026-02-01T09:00:00Z";
pub const POSTED_AT: &str = "2026-04-10T10:00:00Z";
pub const FY: i64 = 2026;

/// The scenario's person id and the agent key it also acts as. Invented
/// fixtures (AGENTS.md), never a real user or key.
const PERSON_ID: &str = "usr_01HQZX9K3M7VN2P4R6T8W0Y1B3";
const KEY_ID: &str = "key_01HQZX9K3M7VN2P4R6T8W0Y1B5";

/// Counters behind the audit ids, so every call site gets a **fresh**
/// event id without a clock or a random source.
static PERSON_EVENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AGENT_EVENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A ULID-shaped id with a fresh counter value, so two contexts never
/// share an event id and an audit chain stays append-only.
fn event_id(counter: &std::sync::atomic::AtomicU64) -> String {
    let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("01HQZX9K3M7VN2P4R6T8W0{n:06X}")
}

/// An [`AuditContext`] for a **person** — the actor the human-only
/// commands (close, reopen, year-end close) accept.
pub fn person() -> AuditContext {
    AuditContext::new(
        Actor::Person {
            person_id: PERSON_ID.to_owned(),
        },
        "req_01HQZX9K3M7VN2P4R6T8W0Y1B4",
        event_id(&PERSON_EVENT),
    )
}

/// An [`AuditContext`] for an **agent key** — refused by every
/// human-only command.
pub fn agent() -> AuditContext {
    AuditContext::new(
        Actor::Agent {
            key_id: KEY_ID.to_owned(),
            tier: "post-within-limits".to_owned(),
        },
        "req_01HQZX9K3M7VN2P4R6T8W0Y1B4",
        event_id(&AGENT_EVENT),
    )
}

/// The chart the scenarios post against: bank, payables, FX loss,
/// sales.
pub const CHART: [(&str, &str, AccountType); 4] = [
    ("1010", "account.inline.bank", AccountType::Asset),
    ("1400", "account.inline.payables", AccountType::Liability),
    ("3000", "account.inline.fx-loss", AccountType::Expense),
    ("4000", "account.inline.sales", AccountType::Income),
];

/// Creates the scenario chart for `company_id`. The scenarios share one
/// database per dialect leg, so a chart that already exists (this
/// company's, from an earlier scenario) is left as it is.
pub async fn create_chart(db: &dyn Database, company_id: &str) {
    for (number, name_key, account_type) in CHART {
        let created = commands::create_account(
            db,
            company_id,
            AccountDraft {
                number: number.to_owned(),
                name_key: name_key.to_owned(),
                account_type,
                normal_balance: None,
                currency: None,
                parent: None,
                tags: Vec::new(),
            },
            NOW.to_owned(),
            &person(),
        )
        .await;
        if let Err(err) = created
            && !matches!(err, ledgers_ledger::AccountsError::AccountExists { .. })
        {
            panic!("create account {number} for {company_id}: {err}");
        }
    }
}

/// Marks one period open for `company_id`.
pub async fn open_period(db: &dyn Database, company_id: &str, period: &str) {
    db.execute(&ledgers_db::period::set_status_statement(
        company_id, period, "open",
    ))
    .await
    .unwrap_or_else(|err| panic!("open {period} for {company_id}: {err}"));
}

/// A SQLite harness over both modules — the ledger's own tables and
/// ledgers-db's guards, chain and outbox. Every table a command writes,
/// `audit_event` included, exists after this.
pub fn harness() -> cratefield_testing::TestHarness {
    cratefield_testing::TestHarness::new(vec![
        Box::new(LedgerModule),
        Box::new(ledgers_db::DbModule),
    ])
}

/// Gives `company_id` the shared chart, so a case that posts or closes
/// has accounts to move money between.
pub async fn mount(db: &dyn Database, company_id: &str) {
    create_chart(db, company_id).await;
}

/// `COUNT(*)` over `table` filtered by one TEXT column.
pub async fn count(db: &dyn Database, table: &str, column: &str, value: &str) -> i64 {
    let rows = db
        .query(&Statement::with_values(
            format!("SELECT COUNT(*) AS n FROM {table} WHERE {column} = ?"),
            vec![SeaValue::from(value)],
        ))
        .await
        .unwrap_or_else(|err| panic!("count {table}: {err}"));
    rows.first()
        .and_then(|row| row.get::<i64>("n"))
        .expect("a count row")
}

/// The journal's counter value: `None` when no row exists — a refused
/// batch rolls its provisioning back, so "no counter" is the shape an
/// untouched sequence takes.
pub async fn counter_value(db: &dyn Database, journal_id: &str, fiscal_year: i64) -> Option<i64> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT next_number FROM numbering_counter WHERE journal_id = ? AND fiscal_year = ?"
                .to_owned(),
            vec![
                SeaValue::from(journal_id),
                SeaValue::BigInt(Some(fiscal_year)),
            ],
        ))
        .await
        .expect("read the counter");
    rows.first().and_then(|row| row.get::<i64>("next_number"))
}
