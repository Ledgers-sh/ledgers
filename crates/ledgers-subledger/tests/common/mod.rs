#![allow(dead_code)]
//! Shared scaffolding for subledger integration tests.
//!
//! Sets up a `TestHarness` with all required modules (storage, ledger, db, subledger),
//! creates a test company Halden Works `GmbH` with a standard chart, and provides
//! helper functions for posting items and reading balances.

use cratefield_core::{Database, Statement};
use ledgers_audit::{Actor, AuditContext};
use ledgers_ledger::LedgerModule;
use ledgers_ledger::account::{AccountDraft, AccountType};
use ledgers_ledger::commands;
use sea_query::Value as SeaValue;

/// Halden Works `GmbH` — the test company (AGENTS.md).
pub const COMPANY_ID: &str = "halden_works";

/// Test timestamp.
pub const NOW: &str = "2026-10-15T10:00:00Z";

/// Fiscal year for tests.
pub const FY: i64 = 2026;

/// Person ID for audit context.
const PERSON_ID: &str = "test_person";

/// Fresh event counter so every context gets a unique `event_id`.
static EVENT_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Generate a fresh ULID-shaped event ID.
fn event_id() -> String {
    let n = EVENT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("01HQZX9K3M7VN2P4R6T8W0{n:06X}")
}

/// `AuditContext` for a person (required for chart setup, period management).
pub fn person() -> AuditContext {
    AuditContext::new(
        Actor::Person {
            person_id: PERSON_ID.to_owned(),
        },
        "req_test",
        event_id(),
    )
}

/// `AuditContext` for an agent (for testing agent behavior in commands).
pub fn agent() -> AuditContext {
    AuditContext::new(
        Actor::Agent {
            key_id: "test_agent_key".to_owned(),
            tier: "post-within-limits".to_owned(),
        },
        "req_test",
        event_id(),
    )
}

/// The scenario's chart: bank, receivables, payables, revenue, expense, fx gain/loss, write-off, retained earnings.
pub const CHART: [(&str, &str, AccountType); 9] = [
    ("1200", "account.bank", AccountType::Asset),     // bank
    ("1400", "account.ar", AccountType::Asset),       // receivables (ar)
    ("1600", "account.ap", AccountType::Liability),   // payables (ap)
    ("4000", "account.revenue", AccountType::Income), // revenue
    ("6000", "account.expense", AccountType::Expense), // expense
    ("4900", "account.fx-gain", AccountType::Income), // fx gain
    ("6900", "account.fx-loss", AccountType::Expense), // fx loss
    ("6950", "account.write-off", AccountType::Expense), // write-offs
    ("3000", "account.retained-earnings", AccountType::Equity), // retained earnings
];

/// Create the scenario chart for a company.
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
                tags: match number {
                    "1400" => vec!["ar".to_string()],
                    "1600" => vec!["ap".to_string()],
                    "4900" => vec!["fx-gain".to_string()],
                    "6900" => vec!["fx-loss".to_string()],
                    "6950" => vec!["write-offs".to_string()],
                    "3000" => vec!["retained-earnings".to_string()],
                    _ => vec![],
                },
                display_name: None,
            },
            NOW.to_owned(),
            &person(),
        )
        .await;
        if let Err(err) = created {
            assert!(
                matches!(err, ledgers_ledger::AccountsError::AccountExists { .. }),
                "create account {number} for {company_id}: {err}"
            );
        }
    }
}

/// Open a fiscal period.
pub async fn open_period(db: &dyn Database, company_id: &str, period: &str) {
    db.execute(&ledgers_db::period::set_status_statement(
        company_id, period, "open",
    ))
    .await
    .unwrap_or_else(|err| panic!("open {period} for {company_id}: {err}"));
}

/// Set up the test harness with all required modules.
pub fn harness() -> cratefield_testing::TestHarness {
    cratefield_testing::TestHarness::new(vec![
        Box::new(ledgers_storage::StorageModule),
        Box::new(LedgerModule),
        Box::new(ledgers_db::DbModule),
        Box::new(ledgers_subledger::SubledgerModule),
    ])
}

/// Mount a company: create it with EUR base, set up the chart, open the period.
pub async fn mount(db: &dyn Database, company_id: &str) {
    // Create company with EUR base if it doesn't exist
    let _ = db
        .execute(&Statement::new(
            format!(
                "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
                 VALUES ('{company_id}', 'Halden Works GmbH', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
            ),
        ))
        .await;

    create_chart(db, company_id).await;
    open_period(db, company_id, &format!("{FY:04}-10")).await;
}

/// Create a test journal and return its ID.
pub async fn create_journal(db: &dyn Database, company_id: &str) -> String {
    let journal_id = format!("journal_{company_id}");
    db.execute(&Statement::with_values(
        "INSERT INTO journal (id, company_id, code, kind, name, created_at) VALUES (?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(journal_id.as_str()),
            SeaValue::from(company_id),
            SeaValue::from("GN"),
            SeaValue::from("general"),
            SeaValue::from("General Journal"),
            SeaValue::from(NOW),
        ],
    ))
    .await
    .expect("create journal");
    journal_id
}

/// Count rows matching a condition.
pub async fn count_rows(db: &dyn Database, table: &str, column: &str, value: &str) -> i64 {
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

/// Read a GL account balance as of a date. Returns the balance in minor units.
pub async fn gl_balance(db: &dyn Database, company_id: &str, account: &str, as_of: &str) -> i64 {
    let rows = db
        .query(&Statement::with_values(
            "SELECT balance_minor FROM account_balance WHERE company_id = ? AND account_number = ? AND date = ? LIMIT 1"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(account),
                SeaValue::from(as_of),
            ],
        ))
        .await
        .expect("query balance");

    rows.first()
        .and_then(|row| row.get::<i64>("balance_minor"))
        .unwrap_or(0)
}

/// Read an open item by ID. Returns None if not found.
pub async fn open_item_row(db: &dyn Database, item_id: &str) -> Option<OpenItemRow> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT company_id, ledger, kind, party_id, source_id, entry_id, control_account, \
             currency, rate_scaled, document_date, posting_date, due_date, \
             tx_original_minor, base_original_minor, tx_remaining_minor, base_remaining_minor, \
             created_at, updated_at FROM open_item WHERE id = ? LIMIT 1"
                .to_owned(),
            vec![SeaValue::from(item_id)],
        ))
        .await
        .expect("query open_item");

    rows.first().map(|row| OpenItemRow {
        id: item_id.to_string(),
        company_id: row.get("company_id").unwrap_or_default(),
        ledger: row.get("ledger").unwrap_or_default(),
        kind: row.get("kind").unwrap_or_default(),
        party_id: row.get("party_id").unwrap_or_default(),
        source_id: row.get("source_id"),
        entry_id: row.get("entry_id").unwrap_or_default(),
        control_account: row.get("control_account").unwrap_or_default(),
        currency: row.get("currency").unwrap_or_default(),
        rate_scaled: row.get("rate_scaled"),
        document_date: row.get("document_date").unwrap_or_default(),
        posting_date: row.get("posting_date").unwrap_or_default(),
        due_date: row.get("due_date"),
        tx_original_minor: row.get("tx_original_minor").unwrap_or(0),
        base_original_minor: row.get("base_original_minor").unwrap_or(0),
        tx_remaining_minor: row.get("tx_remaining_minor").unwrap_or(0),
        base_remaining_minor: row.get("base_remaining_minor").unwrap_or(0),
        created_at: row.get("created_at").unwrap_or_default(),
        updated_at: row.get("updated_at").unwrap_or_default(),
    })
}

/// Open item row data.
#[derive(Clone, Debug)]
pub struct OpenItemRow {
    pub id: String,
    pub company_id: String,
    pub ledger: String,
    pub kind: String,
    pub party_id: String,
    pub source_id: Option<String>,
    pub entry_id: String,
    pub control_account: String,
    pub currency: String,
    pub rate_scaled: Option<String>,
    pub document_date: String,
    pub posting_date: String,
    pub due_date: Option<String>,
    pub tx_original_minor: i64,
    pub base_original_minor: i64,
    pub tx_remaining_minor: i64,
    pub base_remaining_minor: i64,
    pub created_at: String,
    pub updated_at: String,
}
