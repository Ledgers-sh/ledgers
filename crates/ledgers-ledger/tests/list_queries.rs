//! The journal listing reads a page in a constant number of queries.
//!
//! `entries.list` used to read each entry's header, lines and reversals in
//! separate queries (an N+1 that took seconds on D1 for 50 entries). These
//! scenarios count the queries the database port receives and pin three
//! things: the count does not grow with the page size, the page matches what
//! a single-entry read returns, and walking the pages by cursor visits every
//! entry once. They run on every dialect the environment provides.

mod common;

use async_trait::async_trait;
use common::{COMPANY_ID, FY, NOW, POSTED_AT};
use cratefield_core::{Database, DbError, Rows, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::{DbModule, RetryPolicy};
use ledgers_ledger::commands;
use ledgers_ledger::journal::{self, JournalDraft, JournalKind};
use ledgers_ledger::query;
use ledgers_ledger::{Entry, EntryDraft, LedgerModule, Line, PostContext, Side, post_entry};
use ledgers_money::{Amount, Currency};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use time::{Date, Month};

/// Counts every statement that reaches the wrapped database.
struct Counting<'a> {
    inner: &'a dyn Database,
    statements: AtomicUsize,
}

impl<'a> Counting<'a> {
    fn new(inner: &'a dyn Database) -> Self {
        Self {
            inner,
            statements: AtomicUsize::new(0),
        }
    }

    fn count(&self) -> usize {
        self.statements.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Database for Counting<'_> {
    async fn execute(&self, stmt: &Statement) -> Result<u64, DbError> {
        self.statements.fetch_add(1, Ordering::SeqCst);
        self.inner.execute(stmt).await
    }

    async fn query(&self, stmt: &Statement) -> Result<Rows, DbError> {
        self.statements.fetch_add(1, Ordering::SeqCst);
        self.inner.query(stmt).await
    }

    async fn batch_atomic(&self, stmts: &[Statement]) -> Result<(), DbError> {
        self.statements
            .fetch_add(stmts.len().max(1), Ordering::SeqCst);
        self.inner.batch_atomic(stmts).await
    }
}

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR is in the table")
}

/// A balanced 10,000 EUR entry: bank debit, payables credit.
fn lines(sign: Side) -> Vec<Line> {
    let (debit, credit) = match sign {
        Side::Debit => ("1010", "1400"),
        Side::Credit => ("1400", "1010"),
    };
    vec![
        Line {
            account_number: debit.to_owned(),
            side: Side::Debit,
            base: Amount::new(10_000, eur()),
            fx: None,
            tax_code: None,
            dimensions: BTreeMap::new(),
            memo: Some(format!("memo for {debit}")),
        },
        Line {
            account_number: credit.to_owned(),
            side: Side::Credit,
            base: Amount::new(10_000, eur()),
            fx: None,
            tax_code: None,
            dimensions: BTreeMap::new(),
            memo: None,
        },
    ]
}

fn april_draft(
    journal_id: &str,
    description: &str,
    external_id: &str,
    reverses: Option<&str>,
) -> EntryDraft {
    let date = Date::from_calendar_date(2026, Month::April, 10).expect("a calendar date");
    EntryDraft {
        journal_id: journal_id.to_owned(),
        document_date: date,
        posting_date: date,
        tax_point_date: None,
        base_currency: eur(),
        description: description.to_owned(),
        source_documents: vec![],
        external_id: Some(external_id.to_owned()),
        source_app: Some("list-test".to_owned()),
        reverses: reverses.map(str::to_owned),
        lines: if reverses.is_some() {
            lines(Side::Credit)
        } else {
            lines(Side::Debit)
        },
    }
}

/// Posts one entry under `entry_id` and returns that id.
async fn post(db: &dyn Database, journal_id: &str, n: usize, reverses: Option<&str>) -> String {
    let entry_id = format!("01LISTENTRY{n:015}");
    let draft = april_draft(
        journal_id,
        &format!("entry {n}"),
        &format!("ext-{n}-x"),
        reverses,
    );
    let entry = Entry::from_draft(draft, COMPANY_ID).expect("a valid draft");
    let ctx = PostContext {
        fiscal_year: FY,
        entry_id: entry_id.clone(),
        recorded_at: POSTED_AT.to_owned(),
        outbox_id: format!("outbox-{n}"),
    };
    post_entry(
        db,
        &entry,
        &ctx,
        RetryPolicy::default(),
        |_| {},
        &common::agent(),
    )
    .await
    .unwrap_or_else(|err| panic!("post entry {n}: {err}"));
    entry_id
}

/// The company, chart, journal and April period, ready for postings.
async fn fixtures(db: &dyn Database) -> String {
    db.execute(&Statement::new(format!(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
         VALUES ('{COMPANY_ID}', 'Test Company', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )))
    .await
    .ok();
    common::create_chart(db, COMPANY_ID).await;
    let journal = journal::create_journal(
        db,
        COMPANY_ID,
        JournalDraft {
            code: "GN".to_owned(),
            kind: JournalKind::General,
            name: "General journal".to_owned(),
        },
        "01JLISTGN0000000000000000000",
        NOW,
        &common::person(),
    )
    .await
    .expect("the journal is created");
    common::open_period(db, COMPANY_ID, "2026-04").await;
    journal.id
}

/// Lists one page and returns the statements it took.
async fn list_counted(
    db: &dyn Database,
    period: Option<&str>,
    after: Option<&str>,
    limit: u32,
) -> (Vec<query::EntryRecord>, usize) {
    let counting = Counting::new(db);
    let page = query::list_entries(&counting, COMPANY_ID, period, after, limit)
        .await
        .expect("the page reads")
        .expect("the cursor is known");
    (page, counting.count())
}

#[test]
fn list_is_a_constant_number_of_queries_on_every_dialect() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(constant_query_count(&*kit.db));
    }
}

/// Lists 1, 5 and 50 entries and checks the statement count is the same.
async fn constant_query_count(db: &dyn Database) {
    let journal_id = fixtures(db).await;
    let mut posted = 0;
    let mut counts = Vec::new();
    for target in [1usize, 5, 50] {
        while posted < target {
            posted += 1;
            post(db, &journal_id, posted, None).await;
        }
        let (page, count) = list_counted(db, None, None, 200).await;
        assert_eq!(page.len(), target, "every posted entry is listed");
        assert!(
            page.iter().all(|record| record.lines.len() == 2),
            "lines come back with each entry"
        );
        counts.push((target, count));
    }
    let first = counts[0].1;
    for (target, count) in &counts {
        assert_eq!(
            *count, first,
            "listing {target} entries took {count} statements, listing {} took {first}: \
             the count must not grow with the page",
            counts[0].0
        );
    }
}

#[test]
fn list_matches_single_reads_and_pages_visit_every_entry_once() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(page_contents(&*kit.db));
    }
}

/// The page carries the same records a single read does (including the
/// reversal link), and cursor pages of two concatenate to one full page.
async fn page_contents(db: &dyn Database) {
    let journal_id = fixtures(db).await;
    let mut ids = Vec::new();
    for n in 1..=7 {
        ids.push(post(db, &journal_id, n, None).await);
    }
    // Entry 3 is reversed by entry 8.
    let reversal = post(db, &journal_id, 8, Some(&ids[2])).await;

    let (all, _) = list_counted(db, None, None, 200).await;
    assert_eq!(all.len(), 8);
    let listed: Vec<&str> = all.iter().map(|record| record.id.as_str()).collect();
    assert_eq!(listed[0], reversal, "newest posting first");

    for record in &all {
        let single = query::entry_by_id(db, COMPANY_ID, &record.id)
            .await
            .expect("the single read works")
            .expect("the entry exists");
        assert_eq!(
            format!("{record:?}"),
            format!("{single:?}"),
            "page record == single read"
        );
    }
    let original = all
        .iter()
        .find(|record| record.id == ids[2])
        .expect("entry 3 is listed");
    assert_eq!(
        original.reversed_by,
        vec![reversal.clone()],
        "the reversal is linked"
    );

    // Two-at-a-time cursor pages give the same order, with no gaps or repeats.
    let mut walked = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let (page, _) = list_counted(db, None, after.as_deref(), 2).await;
        if page.is_empty() {
            break;
        }
        after = page.last().map(|record| record.id.clone());
        walked.extend(page.into_iter().map(|record| record.id));
    }
    assert_eq!(
        walked, listed,
        "cursor pages visit every entry once, in order"
    );

    let (period, _) = list_counted(db, Some("2026-04"), None, 200).await;
    assert_eq!(period.len(), 8, "the VAT period filter keeps April");
    let (empty, _) = list_counted(db, Some("2026-05"), None, 200).await;
    assert!(empty.is_empty(), "no entries in May");

    let unknown = query::list_entries(db, COMPANY_ID, None, Some("01NOSUCHENTRY0000000000000"), 2)
        .await
        .expect("the read works");
    assert!(
        unknown.is_none(),
        "an unknown cursor is reported, not an empty page"
    );
}

/// The chart listing with balances (`accounts.list`) takes a constant number
/// of statements however many accounts the chart holds: one account select,
/// one grouped child count, one grouped balance. Counted for a 4-account
/// chart and for a 24-account chart.
/// Statements `accounts.list` takes: the chart, then the balances.
async fn count_chart(db: &dyn Database) -> usize {
    let counting = Counting::new(db);
    let accounts = ledgers_ledger::commands::list_accounts(&counting, COMPANY_ID, true)
        .await
        .expect("the chart reads");
    let balances = query::account_balances(&counting, COMPANY_ID)
        .await
        .expect("the balances read");
    assert!(!accounts.is_empty() && !balances.is_empty());
    counting.count()
}

#[test]
fn accounts_list_is_a_constant_number_of_queries_on_every_dialect() {
    use ledgers_ledger::account::{AccountDraft, AccountType};
    use ledgers_ledger::commands;

    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(async {
            let db = &*kit.db;
            let journal_id = fixtures(db).await;
            post(db, &journal_id, 1, None).await;

            let small = count_chart(db).await;

            for n in 0..20 {
                commands::create_account(
                    db,
                    COMPANY_ID,
                    AccountDraft {
                        number: format!("7{n:03}"),
                        name_key: "account.inline.sales".to_owned(),
                        account_type: AccountType::Income,
                        normal_balance: None,
                        currency: None,
                        parent: None,
                        tags: vec![],
                        display_name: None,
                    },
                    NOW.to_owned(),
                    &common::person(),
                )
                .await
                .unwrap_or_else(|err| panic!("create account {n}: {err}"));
            }
            let large = count_chart(db).await;
            assert_eq!(
                small, large,
                "reading a 24-account chart took {large} statements, a 4-account chart took {small}"
            );
        });
    }
}

/// Statements the account page takes: the account with its child probe,
/// its balances and its recent activity. None of them grows with the number
/// of entries that touched the account (the activity page is capped).
async fn count_account_detail(db: &dyn Database) -> usize {
    let counting = Counting::new(db);
    let entry = commands::show_account(&counting, COMPANY_ID, "1010")
        .await
        .expect("the account reads");
    let balances = query::account_balances(&counting, COMPANY_ID)
        .await
        .expect("the balances read");
    let activity = query::account_activity(&counting, COMPANY_ID, "1010", 20)
        .await
        .expect("the activity reads");
    assert_eq!(entry.account.number, "1010");
    assert!(!balances.is_empty() && !activity.is_empty());
    counting.count()
}

#[test]
fn account_detail_is_a_constant_number_of_queries_on_every_dialect() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(async {
            let db = &*kit.db;
            let journal_id = fixtures(db).await;
            post(db, &journal_id, 1, None).await;
            let small = count_account_detail(db).await;

            for n in 2..30 {
                post(db, &journal_id, n, None).await;
            }
            let large = count_account_detail(db).await;
            assert_eq!(
                small, large,
                "the account page with 29 entries took {large} statements, with 1 entry {small}"
            );
        });
    }
}
