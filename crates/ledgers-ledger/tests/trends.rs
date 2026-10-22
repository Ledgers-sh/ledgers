//! The trend report over the real journal, on every backing engine: monthly
//! revenue, expenses, net result, cash, receivables and payables; the top
//! expense accounts; and the split by cost centre. Each figure is checked by
//! hand against the entries posted here.

mod common;

use common::{FY, POSTED_AT, agent};
use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::DbModule;
use ledgers_db::retry::RetryPolicy;
use ledgers_ledger::account::{AccountDraft, AccountType};
use ledgers_ledger::{Entry, EntryDraft, LedgerModule, Line, PostContext, Side, commands, report};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use time::Date;

const COMPANY: &str = "01TRENDSCOMPANY0000000001";
const JOURNAL: &str = "01TRENDSJOURNAL00000000001";
const NOW: &str = "2026-02-01T09:00:00Z";

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR")
}

/// Accounts with the tags the trend report reads: cash and bank, receivables
/// (`ar`), payables (`ap`), sales and an expense.
const ACCOUNTS: [(&str, &str, AccountType, &[&str]); 5] = [
    ("1020", "account.trends.bank", AccountType::Asset, &["bank"]),
    (
        "1030",
        "account.trends.receivables",
        AccountType::Asset,
        &["ar"],
    ),
    (
        "2010",
        "account.trends.payables",
        AccountType::Liability,
        &["ap"],
    ),
    ("4000", "account.trends.sales", AccountType::Income, &[]),
    (
        "5010",
        "account.trends.materials",
        AccountType::Expense,
        &[],
    ),
];

async fn setup(db: &dyn Database) {
    db.execute(&Statement::with_values(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(COMPANY),
            SeaValue::from("Trends company"),
            SeaValue::from("EUR"),
            SeaValue::from("2026-01-01T00:00:00Z"),
            SeaValue::from("2026-01-01T00:00:00Z"),
        ],
    ))
    .await
    .expect("create the company");
    for (number, name_key, account_type, tags) in ACCOUNTS {
        commands::create_account(
            db,
            COMPANY,
            AccountDraft {
                number: number.to_owned(),
                name_key: name_key.to_owned(),
                account_type,
                normal_balance: None,
                currency: None,
                parent: None,
                tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
                display_name: None,
            },
            NOW.to_owned(),
            &agent(),
        )
        .await
        .unwrap_or_else(|err| panic!("create {number}: {err}"));
    }
    db.execute(&Statement::with_values(
        "INSERT INTO journal (id, company_id, code, kind, name, created_at) VALUES (?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(JOURNAL),
            SeaValue::from(COMPANY),
            SeaValue::from("GN"),
            SeaValue::from("general"),
            SeaValue::from("General"),
            SeaValue::from(NOW),
        ],
    ))
    .await
    .expect("create the journal");
    for period in ["2026-01", "2026-02", "2026-03"] {
        db.execute(&ledgers_db::period::set_status_statement(
            COMPANY, period, "open",
        ))
        .await
        .expect("open the period");
    }
    ledgers_ledger::store::create_dimension_value(
        db,
        COMPANY,
        "cost_centre",
        "cc-a",
        "Cost centre A",
        false,
        NOW,
    )
    .await
    .expect("create the cost centre");
}

fn line(account: &str, side: Side, minor: i128, cost_centre: Option<&str>) -> Line {
    let mut dimensions = BTreeMap::new();
    if let Some(code) = cost_centre {
        dimensions.insert("cost_centre".to_owned(), code.to_owned());
    }
    Line {
        account_number: account.to_owned(),
        side,
        base: Amount::new(minor, eur()),
        fx: None,
        tax_code: None,
        dimensions,
        memo: None,
    }
}

/// Posts a balanced entry dated `date` (`YYYY-MM-DD`) with `lines`.
async fn post(db: &dyn Database, id: &str, date: &str, lines: Vec<Line>) {
    let posting = Date::parse(date, &time::format_description::well_known::Iso8601::DATE)
        .expect("an ISO date");
    let draft = EntryDraft {
        journal_id: JOURNAL.to_owned(),
        document_date: posting,
        posting_date: posting,
        tax_point_date: None,
        base_currency: eur(),
        description: format!("trend entry {id}"),
        source_documents: vec![],
        external_id: None,
        source_app: None,
        reverses: None,
        lines,
    };
    let entry = Entry::from_draft(draft, COMPANY).expect("a balanced entry");
    let ctx = PostContext {
        fiscal_year: FY,
        entry_id: id.to_owned(),
        recorded_at: POSTED_AT.to_owned(),
        outbox_id: format!("{id}-outbox"),
    };
    ledgers_ledger::post_entry(db, &entry, &ctx, RetryPolicy::default(), |_| {}, &agent())
        .await
        .unwrap_or_else(|err| panic!("post {id}: {err}"));
}

async fn post_the_quarter(db: &dyn Database) {
    // January: a cost-centre sale paid in at once, and a cost-centre cost paid out.
    post(
        db,
        "01TRENDS0000000000000001",
        "2026-01-10",
        vec![
            line("1020", Side::Debit, 100_000, Some("cc-a")),
            line("4000", Side::Credit, 100_000, Some("cc-a")),
        ],
    )
    .await;
    post(
        db,
        "01TRENDS0000000000000002",
        "2026-01-20",
        vec![
            line("5010", Side::Debit, 30_000, Some("cc-a")),
            line("1020", Side::Credit, 30_000, None),
        ],
    )
    .await;
    // February: a sale on credit (untagged), and a supplier bill on credit.
    post(
        db,
        "01TRENDS0000000000000003",
        "2026-02-05",
        vec![
            line("1030", Side::Debit, 80_000, None),
            line("4000", Side::Credit, 80_000, None),
        ],
    )
    .await;
    post(
        db,
        "01TRENDS0000000000000004",
        "2026-02-15",
        vec![
            line("5010", Side::Debit, 20_000, Some("cc-a")),
            line("2010", Side::Credit, 20_000, None),
        ],
    )
    .await;
    // March: the customer pays.
    post(
        db,
        "01TRENDS0000000000000005",
        "2026-03-01",
        vec![
            line("1020", Side::Debit, 80_000, None),
            line("1030", Side::Credit, 80_000, None),
        ],
    )
    .await;
}

#[test]
fn the_trend_report_matches_the_entries_on_every_engine() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(scenario(&*kit.db));
    }
}

async fn scenario(db: &dyn Database) {
    setup(db).await;
    post_the_quarter(db).await;

    let trends = report::trends(db, COMPANY, "2026-01", "2026-03", Some("cost_centre"))
        .await
        .expect("the trend report reads");
    assert_eq!(trends.months.len(), 3);

    let [jan, feb, mar] = &trends.months[..] else {
        panic!("three months: {trends:?}");
    };
    assert_eq!(jan.period, "2026-01");
    assert_eq!(jan.revenue.minor(), 100_000);
    assert_eq!(jan.expenses.minor(), 30_000);
    assert_eq!(jan.net_result.minor(), 70_000);
    assert_eq!(jan.cash.minor(), 70_000);
    assert_eq!(jan.receivables.minor(), 0);
    assert_eq!(jan.payables.minor(), 0);

    assert_eq!(feb.revenue.minor(), 80_000);
    assert_eq!(feb.expenses.minor(), 20_000);
    assert_eq!(feb.net_result.minor(), 60_000);
    assert_eq!(
        feb.cash.minor(),
        70_000,
        "cash is carried forward from January"
    );
    assert_eq!(feb.receivables.minor(), 80_000, "the sale is still owed");
    assert_eq!(
        feb.payables.minor(),
        20_000,
        "the bill is still owed, shown positive"
    );

    assert_eq!(mar.revenue.minor(), 0);
    assert_eq!(
        mar.cash.minor(),
        150_000,
        "the customer's payment lands in cash"
    );
    assert_eq!(mar.receivables.minor(), 0);
    assert_eq!(mar.payables.minor(), 20_000);

    let [materials] = &trends.top_expense_accounts[..] else {
        panic!("one expense account: {trends:?}");
    };
    assert_eq!(materials.account_code, "5010");
    assert_eq!(materials.name_key, "account.trends.materials");
    assert_eq!(materials.amount.minor(), 50_000);
    assert_eq!(materials.share_bps, 10_000);

    let [cost_centre] = &trends.by_dimension[..] else {
        panic!("one cost centre: {trends:?}");
    };
    assert_eq!(cost_centre.code, "cc-a");
    assert_eq!(cost_centre.name, "Cost centre A");
    assert_eq!(
        cost_centre.revenue.minor(),
        100_000,
        "the untagged February sale is not in it"
    );
    assert_eq!(cost_centre.expenses.minor(), 50_000);

    // A range that starts after the postings still carries the balances in.
    let later = report::trends(db, COMPANY, "2026-03", "2026-03", None)
        .await
        .expect("the trend report reads");
    assert_eq!(later.months[0].cash.minor(), 150_000);
    assert!(later.by_dimension.is_empty());
}
