//! The concurrency proofs: one thousand posts through one journal, one
//! counter, one chain — from a hundred OS threads.
//!
//! What the first test proves, per dialect:
//!
//! - **exactly 1 000 posts commit** — every unique-violation race the
//!   threads lose is retried by the library under the herd budget, and
//!   a post whose budget still exhausts comes back as
//!   [`GuardError::Conflict`] — *busy, try again* — which the worker
//!   resubmits, unchanged, exactly as a caller would (see "Exhaustion
//!   is an answer" below);
//! - **an exhausted post left nothing behind** — before each
//!   resubmission the worker checks its entry has no `demo_entry` row,
//!   no `demo_line`, no `audit_event`; the meter reading exactly 1 000
//!   at the end proves no exhausted attempt charged the allowance, and
//!   the gapless numbering proves none burned a number;
//! - **the numbering is gapless** — `SELECT number ... ORDER BY number`
//!   is exactly `1..=1000`, no holes from rolled-back batches, no
//!   duplicates from races (the counter row lock plus the UNIQUE
//!   backstop);
//! - **the chain is linked** — seqs exactly `1..=1000`, each row's
//!   `prev_hash` the hash of the row before it, the first anchored on
//!   the genesis hash;
//! - the usage meter counted every post.
//!
//! The second test holds the daily limit at 200 across the same
//! thousand attempts: **exactly 200 commit, 800 are refused**, every
//! refusal a [`GuardError::Database`] (the `NOT NULL` of the spent
//! allowance — never a retried-into-conflict, and never resubmitted),
//! the meter reads exactly 200, and the 200 committed entries are still
//! gapless and linked.
//!
//! Threads drive the port with `pollster::block_on`: on the Postgres
//! leg the testing kit's database marshals every call onto the kit's
//! own runtime and blocks the calling thread on a channel, which is
//! correct from any plain thread — so no test-side runtime is needed
//! (that marshalling is the empirical point of running this against
//! Postgres, not just SQLite).
//!
//! # Backoff needs jitter (the empirical finding)
//!
//! A plain ladder — sleep 10 ms, 20, 40 … — retried by a hundred
//! threads that all conflicted in the same round keeps them
//! synchronised: they wake together, re-read the same head, and one
//! winner per wave starves the rest. Measured on the SQLite leg, the
//! first version of this test stranded **3 of 1 000 posts** as
//! exhausted conflicts with [`RetryPolicy::default`]. The library's
//! ladder stays pure (it is a policy, not a clock); the jitter belongs
//! in the injected sleeper, exactly where a deployment puts it. The
//! sleeper below adds a deterministic per-(worker, retry) offset, and
//! the same 1 000 posts commit with zero exhaustions.
//!
//! # The budget must fit the herd (the Postgres finding)
//!
//! On Postgres a posting batch holds the numbering row lock for its
//! whole transaction, so the 1 000 posts serialise: one commit per
//! batch-time, ~85 per second on the test server — a ~12 s drain no
//! retry cleverness removes. [`RetryPolicy::default`] (8 attempts,
//! ~1.1 s of total sleep) is sized for ordinary contention, not for a
//! hundred-thread herd on one journal: measured with it — even with
//! the full-jitter sleeper below — only **796 of 1 000** posts
//! committed (698 before the jitter), the rest exhausting their
//! retries while the herd was still thick. The proofs below therefore
//! run an explicit
//! herd-sized budget — same shape, same ladder law, ~9 s of sleep.
//! That sizing is the deployment's call these tests exist to inform;
//! the light-contention scenarios in [`crate::suite`] stay on the
//! default.
//!
//! # Exhaustion is an answer, not a failure (the CI finding)
//!
//! No attempt budget outlasts every machine. The drain is
//! throughput-bound — on Postgres each attempt reads the chain head and
//! then submits its batch through two separate pool checkouts, so under
//! a hundred-thread herd nearly every attempt's head is stale by the
//! time its batch runs, and the number of attempts a post needs scales
//! with how slow the server is, not with the policy. The first GitHub
//! Actions run of the Postgres leg drained in 18.9 s (vs ~2.7 s
//! locally) and **one** post of 1 000 exhausted the 40-attempt budget;
//! a local Postgres 16 slowed with `commit_delay` stranded 120-170 per
//! run. Asserting "zero exhaustions" therefore asserted the runner's
//! speed, not the ledger's correctness.
//!
//! What the design promises for an exhausted post is that it is a
//! clean, retryable [`GuardError::Conflict`]: the batch rolled back as
//! one, no entry, no line, no audit event, no allowance spent, no
//! number burned. So the proof holds that, and holds liveness the way
//! a caller gets it — by resubmitting the same post on `Conflict`
//! (never on a refusal). Every post must still commit, the numbering
//! must still be exactly `1..=1000` and the chain unforked; the
//! surfaced conflicts are counted and printed, not forbidden.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::{DbModule, GuardError, RetryPolicy, chain, suite};
use sea_query::Value as SeaValue;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WORKERS: usize = 100;
const POSTS_PER_WORKER: usize = 10;
const TOTAL: usize = WORKERS * POSTS_PER_WORKER;
const FY: i64 = 2026;
const ENTRY_DATE: &str = "2026-04-10";
const CREATED_AT: &str = "2026-04-10T10:00:00Z";

/// The retry budget these proofs run under: forty attempts over the
/// same doubling ladder capped at 250 ms — roughly nine seconds of
/// sleep, which outlasts the ~12 s it takes a hundred threads to drain
/// one journal through the numbering row lock (see the module docs for
/// the measurement; [`RetryPolicy::default`] stranded ~200 of 1 000
/// posts here).
const HERD_POLICY: RetryPolicy = RetryPolicy {
    max_attempts: 40,
    base_delay: Duration::from_millis(10),
    max_delay: Duration::from_millis(250),
};

/// How many times a worker resubmits one post after the library
/// returns [`GuardError::Conflict`] — the caller's "busy, try again"
/// loop. Generous on purpose: it bounds a hung test, it is not the
/// budget under test (that is [`HERD_POLICY`]).
const MAX_RESUBMITS: usize = 50;

/// What the herd produced: posts committed, `Conflict`s the library
/// surfaced (each one resubmitted), posts refused, and posts abandoned
/// after [`MAX_RESUBMITS`] conflicts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Tally {
    committed: usize,
    conflicts: usize,
    refused: usize,
    abandoned: usize,
}

impl Tally {
    fn add(self, other: Self) -> Self {
        Self {
            committed: self.committed + other.committed,
            conflicts: self.conflicts + other.conflicts,
            refused: self.refused + other.refused,
            abandoned: self.abandoned + other.abandoned,
        }
    }
}

#[test]
fn a_thousand_posts_stay_gapless_and_linked() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule)]) {
        pollster::block_on(set_up(&kit.db, "jrn-conc"));
        let started = Instant::now();
        let tally = hammer(&kit.db, "jrn-conc", "agent-a", 100_000);
        let elapsed = started.elapsed();
        println!(
            "[{}] {TOTAL} posts from {WORKERS} threads: {} committed, {} exhausted (resubmitted), \
             {} refused, in {elapsed:?}",
            kit.dialect, tally.committed, tally.conflicts, tally.refused
        );
        assert_eq!(
            (tally.committed, tally.refused, tally.abandoned),
            (TOTAL, 0, 0),
            "every post commits — exhausted ones on resubmission — and none is refused: {tally:?}"
        );

        let numbers = pollster::block_on(entry_numbers(&*kit.db, "jrn-conc"));
        assert_eq!(
            numbers,
            (1..=i64::try_from(TOTAL).expect("a small total")).collect::<Vec<_>>(),
            "the numbers are exactly 1..={TOTAL}: no gaps, no duplicates"
        );
        let counter = pollster::block_on(counter_value(&*kit.db, "jrn-conc"));
        assert_eq!(counter, Some(i64::try_from(TOTAL).expect("a small total")));

        let events = pollster::block_on(chain_rows(&*kit.db, "jrn-conc"));
        assert_linked_from_genesis(&events, TOTAL);
        assert_eq!(
            pollster::block_on(used_for(&*kit.db, "agent-a")),
            Some(i64::try_from(TOTAL).expect("a small total")),
            "the meter counted every post"
        );
    }
}

#[test]
fn the_daily_limit_is_exact_under_concurrency() {
    const LIMIT: usize = 200;
    const REFUSED: usize = TOTAL - LIMIT;
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule)]) {
        pollster::block_on(set_up(&kit.db, "jrn-limit-conc"));
        let started = Instant::now();
        let tally = hammer(&kit.db, "jrn-limit-conc", "agent-a", 200);
        let elapsed = started.elapsed();
        println!(
            "[{}] {TOTAL} posts against a daily limit of {LIMIT}: {} committed, {} exhausted \
             (resubmitted), {} refused, in {elapsed:?}",
            kit.dialect, tally.committed, tally.conflicts, tally.refused
        );
        assert_eq!(
            (tally.committed, tally.refused, tally.abandoned),
            (LIMIT, REFUSED, 0),
            "the allowance admits exactly {LIMIT}; every refusal is the NOT NULL of the guard: \
             {tally:?}"
        );

        assert_eq!(
            pollster::block_on(used_for(&*kit.db, "agent-a")),
            Some(i64::try_from(LIMIT).expect("a small limit")),
            "the meter reads exactly the limit"
        );
        let numbers = pollster::block_on(entry_numbers(&*kit.db, "jrn-limit-conc"));
        assert_eq!(
            numbers,
            (1..=i64::try_from(LIMIT).expect("a small limit")).collect::<Vec<_>>(),
            "the committed entries are still gapless under refused batches"
        );
        let events = pollster::block_on(chain_rows(&*kit.db, "jrn-limit-conc"));
        assert_linked_from_genesis(&events, LIMIT);
    }
}

/// Opens the posting period and provisions the demo tables.
async fn set_up(db: &Arc<dyn Database>, journal: &str) {
    suite::create_demo_tables(db.as_ref())
        .await
        .expect("create the demo tables");
    db.execute(&ledgers_db::period::set_status_statement(
        journal, "2026-04", "open",
    ))
    .await
    .expect("open the posting period");
}

/// Runs the workers: `WORKERS` scoped threads, each posting
/// `POSTS_PER_WORKER` balanced entries under one journal, one agent
/// subject and one daily limit, sharing the kit's database handle.
///
/// A post the library returns as [`GuardError::Conflict`] is checked
/// to have left nothing behind, then resubmitted unchanged (same entry
/// id), up to [`MAX_RESUBMITS`] times; a refusal is final.
fn hammer(db: &Arc<dyn Database>, journal: &str, subject: &str, daily_limit: u64) -> Tally {
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(WORKERS);
        for worker in 0..WORKERS {
            let db = db.as_ref();
            handles.push(scope.spawn(move || {
                let mut tally = Tally::default();
                let mut sleeps = 0u32;
                for post_index in 0..POSTS_PER_WORKER {
                    let input = suite::PostInput {
                        entry_id: format!("{journal}-e-{worker:03}-{post_index:02}"),
                        journal_id: journal.to_owned(),
                        fiscal_year: FY,
                        entry_date: ENTRY_DATE.to_owned(),
                        created_at: CREATED_AT.to_owned(),
                        lines: vec![
                            suite::DemoLine {
                                line_no: 1,
                                account_number: "1010".to_owned(),
                                currency: "USD".to_owned(),
                                amount_minor: 100,
                            },
                            suite::DemoLine {
                                line_no: 2,
                                account_number: "4000".to_owned(),
                                currency: "USD".to_owned(),
                                amount_minor: -100,
                            },
                        ],
                        agent_subject: subject.to_owned(),
                        daily_limit,
                    };
                    // The real sleeper, full-jittered over the ladder's
                    // rung: the policy's delay plus a deterministic
                    // per-(worker, retry) spread of up to one more rung,
                    // so a herd that conflicted together wakes spread
                    // uniformly over [delay, 2×delay) instead of as one
                    // wave (see the module docs for the measurement).
                    let mut resubmits = 0;
                    loop {
                        let outcome =
                            pollster::block_on(suite::post(db, &input, HERD_POLICY, |delay| {
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
                            Ok(()) => tally.committed += 1,
                            Err(GuardError::Conflict { .. }) => {
                                tally.conflicts += 1;
                                pollster::block_on(assert_left_nothing(db, &input.entry_id));
                                if resubmits < MAX_RESUBMITS {
                                    resubmits += 1;
                                    continue;
                                }
                                tally.abandoned += 1;
                            }
                            Err(GuardError::Database(_)) => tally.refused += 1,
                        }
                        break;
                    }
                }
                tally
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("worker thread intact"))
            .fold(Tally::default(), Tally::add)
    })
}

/// An exhausted post's whole batch rolled back: no entry row, no line,
/// no audit event carries its entry id. (The allowance and the number
/// are proved at the end: the meter and the numbering count exactly the
/// committed posts.)
async fn assert_left_nothing(db: &dyn Database, entry_id: &str) {
    for (table, column) in [
        ("demo_entry", "entry_id"),
        ("demo_line", "entry_id"),
        ("audit_event", "payload"),
    ] {
        let rows = db
            .query(&Statement::with_values(
                format!("SELECT COUNT(*) AS n FROM {table} WHERE {column} = ?"),
                vec![SeaValue::from(entry_id)],
            ))
            .await
            .unwrap_or_else(|err| panic!("count {table} for {entry_id}: {err}"));
        let n = rows.first().and_then(|row| row.get::<i64>("n"));
        assert_eq!(
            n,
            Some(0),
            "the exhausted post {entry_id} left a {table} row behind"
        );
    }
}

/// `demo_entry.number` for one journal, ascending.
async fn entry_numbers(db: &dyn Database, journal: &str) -> Vec<i64> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT number FROM demo_entry WHERE journal_id = ? ORDER BY number".to_owned(),
            vec![SeaValue::from(journal)],
        ))
        .await
        .expect("read the entry numbers");
    rows.rows
        .iter()
        .map(|row| row.get::<i64>("number").expect("number is an integer"))
        .collect()
}

/// The journal's counter value.
async fn counter_value(db: &dyn Database, journal: &str) -> Option<i64> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT next_number FROM numbering_counter WHERE journal_id = ? AND fiscal_year = ?"
                .to_owned(),
            vec![SeaValue::from(journal), SeaValue::BigInt(Some(FY))],
        ))
        .await
        .expect("read the counter");
    rows.first().and_then(|row| row.get::<i64>("next_number"))
}

/// The chain in seq order: `(seq, prev_hash, hash)`.
async fn chain_rows(db: &dyn Database, journal: &str) -> Vec<(i64, String, String)> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT seq, prev_hash, hash FROM audit_event WHERE company_id = ? ORDER BY seq"
                .to_owned(),
            vec![SeaValue::from(journal)],
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

/// The `used` value of `(subject, "post")`, or `None` without a row.
async fn used_for(db: &dyn Database, subject: &str) -> Option<i64> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT used FROM agent_usage WHERE subject = ? AND meter = 'post'".to_owned(),
            vec![SeaValue::from(subject)],
        ))
        .await
        .expect("read the meter");
    rows.first().and_then(|row| row.get::<i64>("used"))
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
