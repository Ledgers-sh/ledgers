//! The balance sheet's current-year result, over the real journal.
//!
//! The sheet balances on its own: assets = liabilities + equity, where equity
//! is the equity accounts plus the current year result (income minus expenses
//! from the fiscal year start to the as-of date). These scenarios post through
//! the production [`EntryJournal`] port and read `report::balance_sheet` back:
//!
//! - a mid-year company with income and costs balances, and the result line is
//!   the year's profit;
//! - a year with a loss balances, and the result line is negative;
//! - after the year-end close the result line is zero: the closing entry has
//!   already moved the result into retained earnings, so it is not counted
//!   twice.
//!
//! The scenarios run on every backing engine the environment provides.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_audit::{Actor, AuditContext};
use ledgers_db::DbModule;
use ledgers_ledger::chart;
use ledgers_ledger::close::CloseCheckRegistry;
use ledgers_ledger::fiscal::FiscalYearSpec;
use ledgers_ledger::journal::{self, JournalDraft, JournalKind};
use ledgers_ledger::journal_port::{Journal, NewEntry, NewEntryLine};
use ledgers_ledger::{
    BalanceSheet, EntryJournal, LedgerModule, close_fiscal_year, close_period, commands,
    open_fiscal_year,
};
use ledgers_money::{Amount, Currency};

const COMPANY: &str = "01ARZ3NDEKTSV4RRFFQ69G5BSH";
const NOW: &str = "2026-02-01T09:00:00Z";
const JOURNAL_ID: &str = "01BALANCESHEETJOURNAL0000000";

const CHART: &str = r#"
id = "balance-sheet"
name = "template.balance-sheet.name"
version = 1

[[accounts]]
number = "1010"
name = "account.balance-sheet.bank"
type = "asset"
tags = ["bank"]

[[accounts]]
number = "3000"
name = "account.balance-sheet.capital"
type = "equity"

[[accounts]]
number = "4000"
name = "account.balance-sheet.sales"
type = "income"

[[accounts]]
number = "6100"
name = "account.balance-sheet.costs"
type = "expense"

[[accounts]]
number = "5000"
name = "account.balance-sheet.retained-earnings"
type = "equity"
tags = ["retained-earnings"]

[[accounts]]
number = "2000"
name = "account.balance-sheet.fx-gain"
type = "income"
tags = ["fx-gain"]

[[accounts]]
number = "4500"
name = "account.balance-sheet.suspense"
type = "liability"
tags = ["suspense"]

[[accounts]]
number = "6200"
name = "account.balance-sheet.fx-loss"
type = "expense"
tags = ["fx-loss"]

[[accounts]]
number = "6000"
name = "account.balance-sheet.rounding"
type = "expense"
tags = ["rounding"]
"#;

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR is in the currency table")
}

fn person() -> AuditContext {
    let actor = Actor::Person {
        person_id: "owner-1".to_owned(),
    };
    AuditContext::new(
        actor,
        "req_01BALANCESHEET000000000001".to_owned(),
        event_id(),
    )
}

fn event_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "01BALANCESHEET{:014}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// One balanced entry dated `date`: `(account, signed major amount)` lines,
/// debits positive, credits negative.
fn entry(id: &str, date: &str, lines: &[(&str, i128)]) -> NewEntry {
    NewEntry {
        entry_id: id.to_owned(),
        journal_id: JOURNAL_ID.to_owned(),
        fiscal_year: 2026,
        entry_date: date.to_owned(),
        period: None,
        created_at: format!("{date}T10:00:00Z"),
        memo: format!("balance sheet entry {id}"),
        lines: lines
            .iter()
            .map(|(account, major)| NewEntryLine {
                account_number: (*account).to_owned(),
                amount: Amount::from_major(*major, eur()).expect("a representable amount"),
            })
            .collect(),
        actor_subject: "person:owner-1".to_owned(),
        human: true,
        daily_limit: 1_000_000,
    }
}

/// Imports the chart, opens the journal and fiscal year 2026.
async fn setup(db: &dyn Database) {
    db.execute(&Statement::new(format!(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
         VALUES ('{COMPANY}', 'Balance sheet company', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )))
    .await
    .expect("create the company");
    let template = chart::parse_chart(CHART).expect("the chart parses");
    commands::import_template(db, COMPANY, &template, NOW.to_owned(), &person())
        .await
        .expect("import the chart");
    journal::create_journal(
        db,
        COMPANY,
        JournalDraft {
            code: "GJ".to_owned(),
            kind: JournalKind::General,
            name: "General journal".to_owned(),
        },
        JOURNAL_ID,
        NOW,
        &person(),
    )
    .await
    .expect("create the general journal");
    open_fiscal_year(
        db,
        COMPANY,
        FiscalYearSpec {
            label: 2026,
            start_month: 1,
            has_adjustment_period: false,
        },
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("open fiscal 2026");
}

/// Capital of 1000 in February, sales of 500 in March, costs of `costs` in April.
async fn post_year(db: &dyn Database, port: &EntryJournal, costs: i128) {
    for (id, date, lines) in [
        (
            "01BSCAPITAL0000000000000001",
            "2026-02-01",
            vec![("1010", 1000), ("3000", -1000)],
        ),
        (
            "01BSSALES000000000000000001",
            "2026-03-05",
            vec![("1010", 500), ("4000", -500)],
        ),
        (
            "01BSCOSTS000000000000000001",
            "2026-04-09",
            vec![("6100", costs), ("1010", -costs)],
        ),
    ] {
        port.post(db, &entry(id, date, &lines), &person())
            .await
            .unwrap_or_else(|err| panic!("post {id}: {err}"));
    }
}

async fn sheet(db: &dyn Database, as_of: &str) -> BalanceSheet {
    ledgers_ledger::balance_sheet(db, COMPANY, as_of)
        .await
        .expect("the balance sheet reads")
}

fn assert_balances(sheet: &BalanceSheet) {
    assert!(sheet.balanced, "the sheet balances: {sheet:?}");
    assert_eq!(sheet.difference.minor(), 0, "{sheet:?}");
    assert_eq!(
        sheet.assets.minor(),
        sheet.liabilities.minor() + sheet.equity.minor(),
        "assets = liabilities + equity: {sheet:?}"
    );
}

#[test]
fn a_mid_year_sheet_balances_with_the_current_year_result() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(mid_year(&*kit.db));
    }
}

async fn mid_year(db: &dyn Database) {
    setup(db).await;
    let port = EntryJournal;
    post_year(db, &port, 200).await;

    let sheet = sheet(db, "2026-04-30").await;
    assert_balances(&sheet);
    // Capital 1000 is the equity account; sales 500 less costs 200 is the
    // year's profit, 300, which sits inside equity.
    assert_eq!(sheet.equity_accounts, eur_major(1000));
    assert_eq!(sheet.current_year_result, eur_major(300));
    assert_eq!(sheet.equity, eur_major(1300));
    assert_eq!(sheet.assets, eur_major(1300));
}

#[test]
fn a_year_with_a_loss_balances_with_a_negative_result() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(loss(&*kit.db));
    }
}

async fn loss(db: &dyn Database) {
    setup(db).await;
    let port = EntryJournal;
    // Costs of 1200 against sales of 500: a loss of 700.
    post_year(db, &port, 1200).await;

    let sheet = sheet(db, "2026-12-31").await;
    assert_balances(&sheet);
    assert_eq!(sheet.current_year_result, eur_major(-700));
    assert_eq!(sheet.equity_accounts, eur_major(1000));
    assert_eq!(sheet.equity, eur_major(300));
    assert_eq!(sheet.assets, eur_major(300));
}

#[test]
fn the_result_is_not_counted_twice_after_the_year_end_close() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(after_close(&*kit.db));
    }
}

async fn after_close(db: &dyn Database) {
    setup(db).await;
    let port = EntryJournal;
    post_year(db, &port, 200).await;

    // The year-end close needs the periods before December closed and a
    // successor year to carry balances into.
    for month in 1..=11 {
        close_period(
            db,
            COMPANY,
            &format!("2026-{month:02}"),
            &port,
            &CloseCheckRegistry::new(),
            None,
            NOW.to_owned(),
            &person(),
        )
        .await
        .unwrap_or_else(|err| panic!("close 2026-{month:02}: {err}"));
    }
    open_fiscal_year(
        db,
        COMPANY,
        FiscalYearSpec {
            label: 2027,
            start_month: 1,
            has_adjustment_period: false,
        },
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("open fiscal 2027");
    close_fiscal_year(
        db,
        COMPANY,
        2026,
        &port,
        &CloseCheckRegistry::new(),
        None,
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("the year-end close runs");

    let sheet = sheet(db, "2026-12-31").await;
    assert_balances(&sheet);
    // The closing entry netted the P&L to zero and credited the 300 profit to
    // retained earnings: the result line is zero, equity is 1000 + 300 once.
    assert_eq!(sheet.current_year_result, eur_major(0));
    assert_eq!(sheet.equity_accounts, eur_major(1300));
    assert_eq!(sheet.equity, eur_major(1300));
    assert_eq!(sheet.assets, eur_major(1300));
}

fn eur_major(major: i128) -> Amount {
    Amount::from_major(major, eur()).expect("a representable amount")
}
