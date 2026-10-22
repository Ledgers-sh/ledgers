//! The fiscal machinery over the real journal: periods, soft close,
//! close, reopen and the year-end close, driven through
//! [`EntryJournal`] — the production implementation of the [`Journal`]
//! port over the #67 posting batch — on every backing engine the
//! environment provides (in-memory SQLite always; Postgres 16 when
//! `FZ_TEST_POSTGRES_URL` names one).
//!
//! What it proves, against the same `journal_entry` rows the entries API
//! writes:
//!
//! - a posted entry reads back through the port with its gapless number;
//! - a closed period refuses postings from the entries API and from the
//!   port, for people as well as agents;
//! - a `soft-closed` period refuses the entries API (agent-side) and
//!   admits a person posting through the port;
//! - reopening restores postings through the entries API;
//! - the year-end close posts its closing entry into the last period
//!   (through the period override), carries balances forward, and a rerun
//!   is idempotent.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_audit::{Actor, AuditContext};
use ledgers_db::{DbModule, RetryPolicy};
use ledgers_ledger::chart;
use ledgers_ledger::close::CloseCheckRegistry;
use ledgers_ledger::entry::{Line, Side};
use ledgers_ledger::fiscal::FiscalYearSpec;
use ledgers_ledger::journal::{self, JournalDraft, JournalKind};
use ledgers_ledger::journal_port::{Journal, NewEntry, NewEntryLine};
use ledgers_ledger::posting::{PostContext, post_entry};
use ledgers_ledger::{
    AccountsError, Entry, EntryDraft, EntryJournal, JournalError, LedgerModule, PeriodState,
    close_fiscal_year, close_period, commands, open_fiscal_year, period_state, reopen_period,
    soft_close_period,
};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use std::time::Duration;

const COMPANY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const NOW: &str = "2026-02-01T09:00:00Z";
const POSTED_AT: &str = "2026-04-10T10:00:00Z";
const JOURNAL_ID: &str = "01JFISCALJOURNAL00000000000";

const CHART: &str = r#"
id = "fiscal-journal"
name = "template.fiscal-journal.name"
version = 1

[[accounts]]
number = "1010"
name = "account.fiscal-journal.bank"
type = "asset"
tags = ["bank"]

[[accounts]]
number = "3000"
name = "account.fiscal-journal.costs"
type = "expense"
tags = ["fx-loss"]

[[accounts]]
number = "2000"
name = "account.fiscal-journal.fx-gain"
type = "income"
tags = ["fx-gain"]

[[accounts]]
number = "4500"
name = "account.fiscal-journal.suspense"
type = "liability"
tags = ["suspense"]

[[accounts]]
number = "6000"
name = "account.fiscal-journal.rounding"
type = "expense"
tags = ["rounding"]

[[accounts]]
number = "4000"
name = "account.fiscal-journal.sales"
type = "income"

[[accounts]]
number = "5000"
name = "account.fiscal-journal.retained-earnings"
type = "equity"
tags = ["retained-earnings"]
"#;

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR is in the currency table")
}

/// The audit contexts the scenarios post under. The event id is fresh
/// per call, so one company can be written to as often as the test
/// likes.
fn person() -> AuditContext {
    AuditContext::new(actor(true), request(), event_id())
}

fn agent() -> AuditContext {
    AuditContext::new(actor(false), request(), event_id())
}

/// The actor behind either context: the fixture's owner, or the agent
/// key the port treats as machine traffic.
fn actor(human: bool) -> Actor {
    if human {
        Actor::Person {
            person_id: "owner-1".to_owned(),
        }
    } else {
        Actor::Agent {
            key_id: "key-fiscal-1".to_owned(),
            tier: "post-within-limits".to_owned(),
        }
    }
}

/// The context matching a port entry's human flag.
fn context(human: bool) -> AuditContext {
    if human { person() } else { agent() }
}

fn request() -> String {
    "req_01FISCALONJOURNAL0000000001".to_owned()
}

fn event_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "01FISCALONJOURNAL{:012}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn eur_major(major: i128) -> Amount {
    Amount::from_major(major, eur()).expect("a representable amount")
}

fn no_sleep(_: Duration) {}

fn port_entry(entry_id: &str, lines: [(&str, i128); 2], human: bool) -> NewEntry {
    NewEntry {
        entry_id: entry_id.to_owned(),
        journal_id: JOURNAL_ID.to_owned(),
        fiscal_year: 2026,
        entry_date: "2026-04-10".to_owned(),
        period: None,
        created_at: POSTED_AT.to_owned(),
        memo: format!("port entry {entry_id}"),
        lines: lines
            .iter()
            .map(|(account, major)| NewEntryLine {
                account_number: (*account).to_owned(),
                amount: eur_major(*major),
            })
            .collect(),
        actor_subject: "person:owner-1".to_owned(),
        human,
        daily_limit: 1_000,
    }
}

fn api_entry(journal_id: &str, date: &str, number: &str) -> Entry {
    let date = time::Date::parse(
        date,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .expect("an ISO date");
    let line = |account: &str, side: Side| Line {
        account_number: account.to_owned(),
        side,
        base: eur_major(50),
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    };
    Entry::from_draft(
        EntryDraft {
            journal_id: journal_id.to_owned(),
            document_date: date,
            posting_date: date,
            tax_point_date: None,
            base_currency: eur(),
            description: format!("api entry {number}"),
            source_documents: Vec::new(),
            external_id: None,
            source_app: None,
            reverses: None,
            lines: vec![line("1010", Side::Debit), line("4000", Side::Credit)],
        },
        COMPANY,
    )
    .expect("a balanced entry validates")
}

fn api_ctx(number: &str) -> PostContext {
    PostContext {
        fiscal_year: 2026,
        entry_id: format!("01APIENTRY{number:0>16}"),
        recorded_at: POSTED_AT.to_owned(),
        outbox_id: format!("01APIOUTBOX{number:0>16}"),
    }
}

#[test]
fn fiscal_close_and_reopen_run_over_the_real_journal() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(scenarios(&*kit.db));
    }
}

async fn scenarios(db: &dyn Database) {
    // Create the company with EUR base currency
    let _ = db
        .execute(&Statement::new(
            format!(
                "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
                 VALUES ('{COMPANY}', 'Test Company', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
            ),
        ))
        .await;

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
    let port = EntryJournal;

    // A posted entry reads back through the port with its number.
    let posted = port
        .post(
            db,
            &port_entry(
                "01PORTENTRY00000000000001",
                [("1010", 100), ("4000", -100)],
                false,
            ),
            &agent(),
        )
        .await
        .expect("an open period admits the port post");
    assert_eq!(posted.number, 1);
    assert_eq!(
        port.find_entry(db, COMPANY, &posted.entry_id)
            .await
            .expect("find the posted entry"),
        Some(posted.clone())
    );

    close_and_reopen(db, &port).await;
    soft_close(db, &port).await;
    year_end(db, &port).await;
}

/// Closing April refuses postings for everyone; reopening restores them.
async fn close_and_reopen(db: &dyn Database, port: &EntryJournal) {
    // Close April as a person. From here nothing posts into it.
    assert_eq!(
        close_period(
            db,
            COMPANY,
            "2026-04",
            port,
            &CloseCheckRegistry::new(),
            None,
            NOW.to_owned(),
            &person(),
        )
        .await
        .expect("a person closes April"),
        PeriodState::Closed
    );
    let refused = post_entry(
        db,
        &api_entry(JOURNAL_ID, "2026-04-10", "1"),
        &api_ctx("1"),
        RetryPolicy::default(),
        no_sleep,
        &agent(),
    )
    .await
    .expect_err("a closed period refuses the entries API");
    assert_eq!(
        refused,
        JournalError::PeriodNotOpen {
            period: "2026-04".to_owned()
        }
    );
    for human in [false, true] {
        let refused = port
            .post(
                db,
                &port_entry(
                    "01PORTCLOSED0000000000001",
                    [("1010", 5), ("4000", -5)],
                    human,
                ),
                &context(human),
            )
            .await
            .expect_err("a closed period refuses the port for everyone");
        assert!(
            matches!(refused, AccountsError::Database(_)),
            "human={human}: {refused:?}"
        );
    }

    // Reopen with a reason: postings flow again.
    assert_eq!(
        reopen_period(
            db,
            COMPANY,
            "2026-04",
            "owner reopened for a correction",
            NOW.to_owned(),
            &person(),
        )
        .await
        .expect("a person reopens April"),
        PeriodState::Open
    );
    post_entry(
        db,
        &api_entry(JOURNAL_ID, "2026-04-10", "2"),
        &api_ctx("2"),
        RetryPolicy::default(),
        no_sleep,
        &agent(),
    )
    .await
    .expect("a reopened period admits the entries API");
}

/// Soft close: agents are refused, a person still posts through the port.
async fn soft_close(db: &dyn Database, port: &EntryJournal) {
    assert_eq!(
        soft_close_period(db, COMPANY, "2026-04", NOW.to_owned(), &person())
            .await
            .expect("a person soft-closes April"),
        PeriodState::SoftClosed
    );
    assert_eq!(
        period_state(db, COMPANY, "2026-04")
            .await
            .expect("read April"),
        PeriodState::SoftClosed
    );
    let refused = post_entry(
        db,
        &api_entry(JOURNAL_ID, "2026-04-10", "3"),
        &api_ctx("3"),
        RetryPolicy::default(),
        no_sleep,
        &agent(),
    )
    .await
    .expect_err("a soft-closed period refuses the agent-side entries API");
    assert_eq!(
        refused,
        JournalError::PeriodNotOpen {
            period: "2026-04".to_owned()
        }
    );
    port.post(
        db,
        &port_entry(
            "01PORTSOFT00000000000001",
            [("1010", 7), ("4000", -7)],
            true,
        ),
        &person(),
    )
    .await
    .expect("a person posts into a soft-closed period through the port");
    port.post(
        db,
        &port_entry(
            "01PORTSOFT00000000000002",
            [("1010", 7), ("4000", -7)],
            false,
        ),
        &agent(),
    )
    .await
    .expect_err("an agent-side port post stays refused in a soft-closed period");
}

/// The year-end close over the real journal, after the period lifecycle.
async fn year_end(db: &dyn Database, port: &EntryJournal) {
    // The year-end close closes the last period itself, so close the rest.
    for month in 1..=11 {
        let period = format!("2026-{month:02}");
        if period_state(db, COMPANY, &period)
            .await
            .expect("read a period")
            != PeriodState::Closed
        {
            close_period(
                db,
                COMPANY,
                &period,
                port,
                &CloseCheckRegistry::new(),
                None,
                NOW.to_owned(),
                &person(),
            )
            .await
            .unwrap_or_else(|err| panic!("close {period}: {err}"));
        }
    }

    // Year-end: open 2027 so the close has a successor, then close 2026.
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
    let first = close_fiscal_year(
        db,
        COMPANY,
        2026,
        port,
        &CloseCheckRegistry::new(),
        None,
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("the year-end close runs over the real journal");
    assert_eq!(
        first.closing_entry.entry_id,
        "yearend-01ARZ3NDEKTSV4RRFFQ69G5FAV-2026"
    );
    // Income 50 + 100 + 7 posted, costs none: 157 EUR clears to retained
    // earnings; the bank and retained-earnings balances carry forward.
    assert_eq!(first.retained_earnings_result.minor().abs(), 15_700);
    let carried: i64 = count_rows(
        db,
        "SELECT COUNT(*) AS n FROM fiscal_year_opening_balance WHERE company_id = ? AND fiscal_year = 2027",
    )
    .await;
    assert_eq!(carried, 2, "bank and retained earnings carry forward");
    assert_eq!(first.opening_balance_count, 2);

    // A rerun reads the stored report back and posts nothing new.
    let again = close_fiscal_year(
        db,
        COMPANY,
        2026,
        port,
        &CloseCheckRegistry::new(),
        None,
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("a rerun of the year-end close is idempotent");
    assert_eq!(again.closing_entry, first.closing_entry);
    assert_eq!(
        again.retained_earnings_result,
        first.retained_earnings_result
    );
}

async fn count_rows(db: &dyn Database, sql: &str) -> i64 {
    let rows = db
        .query(&cratefield_core::Statement::with_values(
            sql.to_owned(),
            vec![SeaValue::from(COMPANY)],
        ))
        .await
        .expect("count query runs");
    rows.rows
        .first()
        .and_then(|row| row.get::<i64>("n"))
        .expect("a count row")
}
