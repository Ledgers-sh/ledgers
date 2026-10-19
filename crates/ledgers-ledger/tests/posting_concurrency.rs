//! The ledger-level concurrency proof: one thousand journal posts
//! through one journal, one counter, one chain, one outbox — from a
//! hundred OS threads, each post the real [`post_entry`] (domain
//! validation, guard statements, audit append, outbox enqueue).
//!
//! What it proves, per dialect:
//!
//! - **exactly 1 000 posts commit** — every numbering race the threads
//!   lose is retried to success under the herd-sized budget;
//! - **the numbering is gapless** — `SELECT number ... ORDER BY number`
//!   over `journal_entry` is exactly `1..=1000`, the UNIQUE
//!   `(journal_id, fiscal_year, number)` never firing a duplicate;
//! - **the counter** reads exactly 1 000;
//! - **the chain is linked** — seqs exactly `1..=1000`, each
//!   `prev_hash` the hash of the row before, the first anchored on the
//!   genesis hash;
//! - **the outbox** holds exactly 1 000 `journal.entry.posted` events.
//!
//! The retry budget and the full-jitter sleeper mirror the
//! guard-foundation's own concurrency proof
//! (`crates/ledgers-db/tests/concurrency.rs`), whose module docs record
//! the measurements: a plain ladder strands a synchronised herd
//! (3 of 1 000 on SQLite), and [`RetryPolicy::default`] (8 attempts)
//! exhausts under a hundred-thread Postgres herd (796 of 1 000). The
//! jitter belongs in the injected sleeper — the deployment's clock —
//! and the budget is the deployment's call; both are set here to the
//! measured herd sizes.

mod common;

use common::{COMPANY_ID, FY, NOW, POSTED_AT, counter_value, create_chart, open_period};
use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::{DbModule, RetryPolicy, chain};
use ledgers_ledger::journal::{self, JournalDraft, JournalKind};
use ledgers_ledger::posting::{PostContext, TOPIC_ENTRY_POSTED, post_entry};
use ledgers_ledger::{Entry, EntryDraft, JournalError, LedgerModule, Line, Side};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use time::{Date, Month};

const WORKERS: usize = 100;
const POSTS_PER_WORKER: usize = 10;
const TOTAL: usize = WORKERS * POSTS_PER_WORKER;
/// The concurrency journal's id, one constant so the set-up, the
/// workers and the read-backs agree.
const JOURNAL_ID: &str = "01JCJ000000000000000000000000000";

/// The herd-sized budget of the guard-foundation proof: forty attempts
/// over the doubling ladder capped at 250 ms, which outlasts the ~12 s
/// a hundred threads need to drain one journal through the numbering
/// row lock on Postgres.
const HERD_POLICY: RetryPolicy = RetryPolicy {
    max_attempts: 40,
    base_delay: Duration::from_millis(10),
    max_delay: Duration::from_millis(250),
};

#[test]
fn a_thousand_journal_posts_stay_gapless_linked_and_emitted() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(LedgerModule), Box::new(DbModule)]) {
        pollster::block_on(set_up(&kit.db));
        let started = Instant::now();
        let (ok, conflicts, refused) = hammer(&kit.db);
        let elapsed = started.elapsed();
        println!(
            "[{}] {TOTAL} journal posts from {WORKERS} threads: {ok} committed, {conflicts} \
             exhausted, {refused} refused, in {elapsed:?}",
            kit.dialect
        );
        assert_eq!(
            (ok, conflicts, refused),
            (TOTAL, 0, 0),
            "every post commits under the herd budget"
        );

        let numbers = pollster::block_on(entry_numbers(&*kit.db));
        assert_eq!(
            numbers,
            (1..=i64::try_from(TOTAL).expect("a small total")).collect::<Vec<_>>(),
            "the numbers are exactly 1..={TOTAL}: no gaps, no duplicates"
        );
        assert_eq!(
            pollster::block_on(counter_value(&*kit.db, JOURNAL_ID, FY)),
            Some(i64::try_from(TOTAL).expect("a small total")),
            "the counter advanced once per commit"
        );

        let events = pollster::block_on(chain_rows(&*kit.db));
        assert_linked_from_genesis(&events, TOTAL);

        let emitted = pollster::block_on(outbox_count(&*kit.db));
        assert_eq!(
            emitted,
            i64::try_from(TOTAL).expect("a small total"),
            "one outbox event per committed post"
        );
    }
}

/// The chart, the journal, and the open posting period.
async fn set_up(db: &Arc<dyn Database>) {
    create_chart(db.as_ref(), COMPANY_ID).await;
    journal::create_journal(
        db.as_ref(),
        COMPANY_ID,
        JournalDraft {
            code: "CJ".to_owned(),
            kind: JournalKind::General,
            name: "Concurrency journal".to_owned(),
        },
        JOURNAL_ID,
        NOW,
    )
    .await
    .expect("create the journal");
    open_period(db.as_ref(), COMPANY_ID, "2026-04").await;
}

/// Runs the workers: `WORKERS` scoped threads, each posting
/// `POSTS_PER_WORKER` validated two-line entries through the real
/// posting path, sharing the kit's database handle.
fn hammer(db: &Arc<dyn Database>) -> (usize, usize, usize) {
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(WORKERS);
        for worker in 0..WORKERS {
            let db = db.as_ref();
            handles.push(scope.spawn(move || {
                let entry = the_entry(JOURNAL_ID);
                let mut ok = 0;
                let mut conflicts = 0;
                let mut refused = 0;
                let mut sleeps = 0u32;
                for post_index in 0..POSTS_PER_WORKER {
                    let ctx = PostContext {
                        fiscal_year: FY,
                        entry_id: format!("01ENTRYCONC{worker:03}{post_index:02}0000000000"),
                        recorded_at: POSTED_AT.to_owned(),
                        outbox_id: format!("01OUTBOXCONC{worker:03}{post_index:02}000000000"),
                    };
                    // The full-jitter sleeper of the guard-foundation
                    // proof: the policy's rung plus a deterministic
                    // per-(worker, retry) spread of up to one more
                    // rung, so a herd that conflicted together wakes
                    // spread over [delay, 2×delay) instead of as one
                    // wave.
                    let outcome =
                        pollster::block_on(post_entry(db, &entry, &ctx, HERD_POLICY, |delay| {
                            sleeps += 1;
                            let rung = u64::try_from(delay.as_nanos())
                                .expect("a positive rung")
                                .max(1);
                            let ticket = u64::try_from(worker).expect("a small worker index")
                                * 7_919
                                + u64::from(sleeps) * 104_729;
                            std::thread::sleep(Duration::from_nanos(
                                rung + (ticket.wrapping_mul(6_364_136_223_846_793_005) % rung),
                            ));
                        }));
                    match outcome {
                        Ok(_) => ok += 1,
                        Err(JournalError::Conflict { .. }) => conflicts += 1,
                        Err(JournalError::Database(_)) => refused += 1,
                        Err(other) => panic!("unexpected error: {other:?}"),
                    }
                }
                (ok, conflicts, refused)
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("worker thread intact"))
            .fold((0, 0, 0), |sum, one| {
                (sum.0 + one.0, sum.1 + one.1, sum.2 + one.2)
            })
    })
}

/// The one validated entry every post re-uses: an EUR 1.00 transfer
/// from sales to bank, posting on the open April day.
fn the_entry(journal_id: &str) -> Entry {
    let line = |account: &str, side: Side| Line {
        account_number: account.to_owned(),
        side,
        base: Amount::from_major(1, Currency::from_code("EUR").expect("EUR"))
            .expect("a representable amount"),
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    };
    Entry::from_draft(
        EntryDraft {
            journal_id: journal_id.to_owned(),
            document_date: Date::from_calendar_date(2026, Month::April, 10).expect("a date"),
            posting_date: Date::from_calendar_date(2026, Month::April, 10).expect("a date"),
            tax_point_date: None,
            base_currency: Currency::from_code("EUR").expect("EUR"),
            description: "Concurrency post".to_owned(),
            source_documents: Vec::new(),
            reverses: None,
            lines: vec![line("1010", Side::Debit), line("4000", Side::Credit)],
        },
        COMPANY_ID,
    )
    .expect("the entry validates")
}

/// `journal_entry.number` for the concurrency journal, ascending.
async fn entry_numbers(db: &dyn Database) -> Vec<i64> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT number FROM journal_entry WHERE journal_id = ? ORDER BY number".to_owned(),
            vec![SeaValue::from(JOURNAL_ID)],
        ))
        .await
        .expect("read the entry numbers");
    rows.rows
        .iter()
        .map(|row| row.get::<i64>("number").expect("number is an integer"))
        .collect()
}

/// The chain in seq order: `(seq, prev_hash, hash)`.
async fn chain_rows(db: &dyn Database) -> Vec<(i64, String, String)> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT seq, prev_hash, hash FROM audit_event WHERE company_id = ? ORDER BY seq"
                .to_owned(),
            vec![SeaValue::from(COMPANY_ID)],
        ))
        .await
        .expect("read the chain");
    rows.rows
        .iter()
        .map(|row| {
            (
                row.get::<i64>("seq").expect("seq is an integer"),
                row.get::<String>("prev_hash").expect("prev_hash is text"),
                row.get::<String>("hash").expect("hash is text"),
            )
        })
        .collect()
}

/// The number of `journal.entry.posted` events sitting in the outbox.
async fn outbox_count(db: &dyn Database) -> i64 {
    let rows = db
        .query(&Statement::with_values(
            "SELECT COUNT(*) AS n FROM outbox WHERE topic = ?".to_owned(),
            vec![SeaValue::from(TOPIC_ENTRY_POSTED)],
        ))
        .await
        .expect("count the outbox");
    rows.first()
        .and_then(|row| row.get::<i64>("n"))
        .expect("a count row")
}

/// Seqs run exactly `1..=expected`, each `prev_hash` equals the row
/// before's `hash`, the first anchored on the genesis hash.
fn assert_linked_from_genesis(events: &[(i64, String, String)], expected: usize) {
    assert_eq!(events.len(), expected, "one event per committed post");
    let mut previous_hash = chain::GENESIS_HASH.to_owned();
    for (index, (seq, prev_hash, hash)) in events.iter().enumerate() {
        let position = i64::try_from(index + 1).expect("a small chain");
        assert_eq!(*seq, position, "seqs run 1..={expected} with no gaps");
        assert_eq!(
            prev_hash, &previous_hash,
            "the event at seq {seq} links to the hash before it"
        );
        previous_hash.clone_from(hash);
    }
}
