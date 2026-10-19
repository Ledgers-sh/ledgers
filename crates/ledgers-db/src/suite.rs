//! The worked guard-statement example: one posting batch carrying every
//! guard at once, plus the scenarios that prove each guard refuses.
//!
//! These are plain `async` functions over `&dyn Database`, runnable
//! against every backing engine (D1, rusqlite, Postgres) with no
//! runtime dependency — whatever executor the caller has drives them,
//! exactly like `ledgers-storage`'s conformance suite. `tests/matrix.rs`
//! runs [`assert_guard_conformance`] over every dialect the environment
//! provides; `tests/concurrency.rs` hammers [`post`] from a hundred
//! threads.
//!
//! The demo tables (`demo_entry`, `demo_line`) are the suite's
//! stand-in for the future journal-entry tables: ad-hoc DDL created
//! with `CREATE TABLE IF NOT EXISTS`, deliberately **not** in the
//! crate's migrations and **not** in the module's `tables()` — the real
//! journal schema belongs to the journal crate. What they carry is the
//! shape the guards need: a `NOT NULL BIGINT number` column under
//! `UNIQUE (journal_id, fiscal_year, number)`, and lines that sum to
//! zero.
//!
//! # The reference batch
//!
//! [`plan_post`] reads exactly one thing before the batch (the audit
//! head) and returns the statements in the canonical order — the ADR
//! example, and the shape every posting crate should keep:
//!
//! ```sql
//! -- 1. numbering: provision (idempotent), bump, insert reading the counter
//! -- 2. the entry's lines
//! -- 3. period guard (open to all; soft-closed to people only;
//! --    closed or unknown to no one)
//! -- 4. balanced guard (single currency, zero sum)
//! -- 5. releases for both probes
//! -- 6. the daily-limit consume (harness Usage, NOT NULL refusal)
//! -- 7. the audit append (claims head.seq + 1; PK makes forks impossible)
//! ```
//!
//! Everything the batch touches either commits together or not at all.
//! The demo collapses the company scope onto the journal id (the real
//! engine scopes periods and the chain by `company_id`); amounts are
//! signed minor units, debit positive, credit negative; and the
//! balanced guard here checks the entry's own currency only — the real
//! posting batch additionally converts foreign-currency lines to the
//! base currency via `ledgers-money` before summing, which is the
//! engine's job, not the guard's.

use crate::chain::{self, GENESIS_HASH};
use crate::guard;
use crate::limit;
use crate::numbering;
use crate::period;
use crate::retry::{self, GuardError, RetryPolicy};
use cratefield_core::{Database, DbError, Statement};
use sea_query::Value as SeaValue;
use std::time::Duration;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

/// The usage meter the demo posts under.
const METER_POST: &str = "post";
/// The fixed timestamp audit events in the scenarios carry.
const AUDIT_TIME: &str = "2026-04-10T10:00:00Z";

/// Probe DDL and fixtures for the scenarios.
pub mod demo {
    /// The entry stand-in: the journal crate's future table, carrying
    /// exactly the shape the guards need — a `NOT NULL BIGINT number`
    /// under `UNIQUE (journal_id, fiscal_year, number)` (the numbering
    /// backstop) and the scoping keys the guards read.
    pub const ENTRY_TABLE: &str = "CREATE TABLE IF NOT EXISTS demo_entry (\n    \
         entry_id TEXT PRIMARY KEY,\n    \
         journal_id TEXT NOT NULL,\n    \
         fiscal_year INTEGER NOT NULL,\n    \
         number BIGINT NOT NULL,\n    \
         entry_date TEXT NOT NULL,\n    \
         created_at TEXT NOT NULL,\n    \
         UNIQUE (journal_id, fiscal_year, number)\n);";
    /// The line stand-in: signed minor-unit amounts (debit +, credit −),
    /// `BIGINT` per the storage rules.
    pub const LINE_TABLE: &str = "CREATE TABLE IF NOT EXISTS demo_line (\n    \
         entry_id TEXT NOT NULL REFERENCES demo_entry(entry_id),\n    \
         line_no INTEGER NOT NULL,\n    \
         account_number TEXT NOT NULL,\n    \
         currency TEXT NOT NULL,\n    \
         amount_minor BIGINT NOT NULL,\n    \
         PRIMARY KEY (entry_id, line_no)\n);";
}

/// One posting line: signed minor units, debit positive, credit
/// negative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoLine {
    /// 1-based position within the entry.
    pub line_no: i64,
    /// The account's number in the chart.
    pub account_number: String,
    /// ISO 4217 code; an entry guards to a single currency.
    pub currency: String,
    /// Minor units, signed: debit `+`, credit `−`.
    pub amount_minor: i64,
}

/// Everything [`plan_post`] needs to build one posting batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostInput {
    /// Fresh ULID per post; two posts never share one.
    pub entry_id: String,
    /// The journal (and, in this demo, the company scope the period
    /// guard and audit chain key on).
    pub journal_id: String,
    /// The fiscal year the numbering counter is scoped to.
    pub fiscal_year: i64,
    /// ISO 8601 calendar date, `"YYYY-MM-DD"` — names the posting
    /// period unless `period_override` says otherwise, and the
    /// usage-meter day.
    pub entry_date: String,
    /// ISO 8601 UTC timestamp bound from code, never `NOW()`.
    pub created_at: String,
    /// The entry's lines; the balanced guard requires at least the
    /// zero sum over them.
    pub lines: Vec<DemoLine>,
    /// Who is posting: a person (`true`) or an agent key (`false`) —
    /// the period guard admits a person into a `soft-closed` period
    /// and refuses an agent key there.
    pub human: bool,
    /// The period the guard checks: `None` derives `"YYYY-MM"` from
    /// `entry_date`; `Some(period)` checks that period instead — the
    /// adjustment-period path, an entry dated in one month posted into
    /// another the operator left open for adjustments.
    pub period_override: Option<String>,
    /// Who the daily limit is charged to (the agent subject).
    pub agent_subject: String,
    /// The daily allowance for `agent_subject` under `meter = "post"`.
    pub daily_limit: u64,
}

/// Creates the demo tables if they do not exist yet. Idempotent, so
/// every scenario can self-provision (the precedent is the int64 probe
/// in `ledgers-storage`'s suite).
///
/// # Errors
///
/// [`DbError`] when the DDL fails.
pub async fn create_demo_tables(db: &dyn Database) -> Result<(), DbError> {
    db.execute(&Statement::new(demo::ENTRY_TABLE)).await?;
    db.execute(&Statement::new(demo::LINE_TABLE)).await?;
    Ok(())
}

/// Builds the reference posting batch for `input` (see the module docs
/// for the statement order and what each guard proves). The only
/// pre-batch read is the audit head — which is why a retry, re-running
/// this whole function, is correct. The period guard checks
/// `input.period_override` when set — the adjustment-period path —
/// else the entry date's own month, and admits a `soft-closed` period
/// only when `input.human` says a person is posting.
///
/// # Errors
///
/// [`DbError`] when the head read fails, or immediately — no batch is
/// built — when `entry_date` is not a real `"YYYY-MM-DD"` calendar
/// date.
pub async fn plan_post(db: &dyn Database, input: &PostInput) -> Result<Vec<Statement>, DbError> {
    let derived_period = period::period_of_date(&input.entry_date).ok_or_else(|| {
        DbError::Query(format!(
            "entry_date {} is not a YYYY-MM-DD calendar date",
            input.entry_date
        ))
    })?;
    let posting_period = input.period_override.clone().unwrap_or(derived_period);
    let day = posting_day(&input.entry_date)?;
    let head = chain::read_head(db, &input.journal_id).await?;

    let mut statements = Vec::with_capacity(input.lines.len() + 8);

    // 1. Gapless numbering: provision (idempotent), bump, insert reading
    //    the counter — the entry's NOT NULL number comes from the row
    //    this batch just advanced.
    statements.push(numbering::provision_statement(
        &input.journal_id,
        input.fiscal_year,
    ));
    statements.push(numbering::bump_statement(
        &input.journal_id,
        input.fiscal_year,
    ));
    statements.push(numbering::insert_with_number(
        "demo_entry",
        &[
            "entry_id",
            "journal_id",
            "fiscal_year",
            "entry_date",
            "created_at",
        ],
        vec![
            SeaValue::from(input.entry_id.as_str()),
            SeaValue::from(input.journal_id.as_str()),
            SeaValue::BigInt(Some(input.fiscal_year)),
            SeaValue::from(input.entry_date.as_str()),
            SeaValue::from(input.created_at.as_str()),
        ],
        "number",
        &input.journal_id,
        input.fiscal_year,
    ));

    // 2. The lines.
    for line in &input.lines {
        statements.push(Statement::with_values(
            "INSERT INTO demo_line (entry_id, line_no, account_number, currency, amount_minor)\n\
             VALUES (?, ?, ?, ?, ?)"
                .to_owned(),
            vec![
                SeaValue::from(input.entry_id.as_str()),
                SeaValue::BigInt(Some(line.line_no)),
                SeaValue::from(line.account_number.as_str()),
                SeaValue::from(line.currency.as_str()),
                SeaValue::BigInt(Some(line.amount_minor)),
            ],
        ));
    }

    // 3. The period guard: `open` admits anyone, `soft-closed` only a
    //    person, `closed` (or no row — the guard fails closed) no one.
    //    The abort rolls back the lines written above with it.
    let period_key = guard::guard_key(&[
        "period-postable",
        input.journal_id.as_str(),
        posting_period.as_str(),
        input.entry_id.as_str(),
    ]);
    statements.push(period::assert_postable_statement(
        &period_key,
        &input.journal_id,
        &posting_period,
        input.human,
    ));

    // 4. The balanced guard: single currency and a zero sum over this
    //    entry's lines, read inside the batch so it sees exactly what
    //    this batch wrote. The real posting batch converts to the base
    //    currency via ledgers-money before summing — the engine's job;
    //    the guard's job is that whatever it is given to sum, sums.
    let balance_key = guard::guard_key(&["balanced", input.entry_id.as_str()]);
    statements.push(guard::assert_statement(
        &balance_key,
        "(SELECT COUNT(DISTINCT currency) FROM demo_line WHERE entry_id = ?) = 1\n\
             AND (SELECT COALESCE(SUM(amount_minor), 0) FROM demo_line WHERE entry_id = ?) = 0",
        vec![
            SeaValue::from(input.entry_id.as_str()),
            SeaValue::from(input.entry_id.as_str()),
        ],
    ));

    // 5. Releases: a committed batch leaves no probe rows behind.
    statements.push(guard::release_statement(&period_key));
    statements.push(guard::release_statement(&balance_key));

    // 6. The daily-limit consume: the harness's guarded upsert, beside
    //    the work it pays for. A spent allowance fails NOT NULL here.
    statements.push(limit::daily_consume_statement(
        &input.agent_subject,
        METER_POST,
        day,
        1,
        input.daily_limit,
    ));

    // 7. The audit append: claims head.seq + 1 against this batch's
    //    work. A stale head takes a unique violation; retry re-plans.
    let seq = head.seq + 1;
    let hash = demo_hash(seq, &input.entry_id);
    statements.push(chain::append_statement(
        &input.journal_id,
        &head,
        &hash,
        &input.entry_id,
        &input.created_at,
    ));

    Ok(statements)
}

/// Posts one entry: plans the reference batch fresh and submits it
/// under `policy`, retrying (re-planning) on unique-violation races and
/// returning refusals immediately. `sleep` is the caller's back-off —
/// tests pass `std::thread::sleep`; the library itself never sleeps.
///
/// # Errors
///
/// [`GuardError::Conflict`] when the retry budget ran out on races;
/// [`GuardError::Database`] when a guard refused (a closed period, a
/// soft-closed period refusing an agent key, a spent allowance,
/// unbalanced lines) or the engine failed.
pub async fn post(
    db: &dyn Database,
    input: &PostInput,
    policy: RetryPolicy,
    sleep: impl FnMut(Duration),
) -> Result<(), GuardError> {
    retry::run_batch_with_retry(db, policy, async || plan_post(db, input).await, sleep).await
}

/// The composite every dialect must pass: each guard refusing, the
/// numbering round trip, the chain's fork-freedom, the retry budget,
/// and the usage-table DDL pin. `tests/matrix.rs` runs it against every
/// backing engine the environment provides.
///
/// # Panics
///
/// Panics when any constituent scenario panics — exactly what a
/// conformance run should surface.
pub async fn assert_guard_conformance(db: &dyn Database) {
    numbering_round_trip(db).await;
    closed_period_rejects(db).await;
    unknown_period_rejects(db).await;
    period_states_gate_posting(db).await;
    unbalanced_lines_reject(db).await;
    mixed_currency_rejects(db).await;
    daily_limit_rejects(db).await;
    chain_appends_and_retries(db).await;
    retry_exhaustion_is_a_conflict(db).await;
    usage_table_matches(db).await;
}

/// Ten sequential posts take the numbers 1..=10 with no holes, and the
/// counter rests at 10.
///
/// # Panics
///
/// Panics when a post fails or the numbers are not exactly `1..=10`.
pub async fn numbering_round_trip(db: &dyn Database) {
    const JOURNAL: &str = "jrn-numbering";
    create_demo_tables(db).await.expect("create demo tables");
    open_period(db, JOURNAL, "2026-04").await;
    for n in 1..=10 {
        let input = balanced_input(
            &format!("e-numbering-{n:02}"),
            JOURNAL,
            "agent-numbering",
            1000,
        );
        post(db, &input, RetryPolicy::default(), no_sleep)
            .await
            .unwrap_or_else(|err| panic!("post {n} in the round trip: {err}"));
    }

    let numbers = entry_numbers(db, JOURNAL).await;
    let expected: Vec<i64> = (1..=10).collect();
    assert_eq!(numbers, expected, "ten posts take the numbers 1..=10");

    let counter = counter_value(db, JOURNAL, 2026).await;
    assert_eq!(counter, Some(10), "the counter rests at the last number");
}

/// A closed period refuses the post inside the batch: the error is the
/// `NOT NULL` refusal (never a conflict), and nothing the batch wrote
/// survives — no entry, no lines, no audit event, no spent allowance.
///
/// # Panics
///
/// Panics when the post does not fail exactly that way, or when any of
/// the batch's effects survived the rollback.
pub async fn closed_period_rejects(db: &dyn Database) {
    const JOURNAL: &str = "jrn-closed";
    create_demo_tables(db).await.expect("create demo tables");
    db.execute(&period::set_status_statement(JOURNAL, "2026-04", "closed"))
        .await
        .expect("close the period");

    let input = balanced_input("e-closed-1", JOURNAL, "agent-closed", 1000);
    let err = post(db, &input, RetryPolicy::default(), no_sleep)
        .await
        .expect_err("a closed period refuses the post");
    assert!(
        matches!(err, GuardError::Database(_)),
        "the refusal is a fired guard (NOT NULL), not a conflict: {err:?}"
    );

    assert_eq!(entry_count(db, JOURNAL).await, 0, "no entry survived");
    assert_eq!(audit_count(db, JOURNAL).await, 0, "no audit event survived");
    assert_eq!(
        used_for(db, "agent-closed").await,
        None,
        "the refused post spent no allowance"
    );
}

/// A period with no `accounting_period` row at all refuses too — the
/// guard fails closed: unknown means closed.
///
/// # Panics
///
/// Panics when the post succeeds or leaves anything behind.
pub async fn unknown_period_rejects(db: &dyn Database) {
    const JOURNAL: &str = "jrn-unknown";
    create_demo_tables(db).await.expect("create demo tables");
    // No set_status_statement: the period row simply does not exist.

    let input = balanced_input("e-unknown-1", JOURNAL, "agent-unknown", 1000);
    let err = post(db, &input, RetryPolicy::default(), no_sleep)
        .await
        .expect_err("an unknown period refuses the post");
    assert!(
        matches!(err, GuardError::Database(_)),
        "fail-closed is a fired guard, not a conflict: {err:?}"
    );
    assert_eq!(entry_count(db, JOURNAL).await, 0, "no entry survived");
    assert_eq!(audit_count(db, JOURNAL).await, 0, "no audit event survived");
}

/// The three period states gate posting exactly as policy demands:
/// `open` admits a person and an agent key alike; `soft-closed` admits
/// the person and refuses the agent key; `closed` refuses both — a post
/// dated in a closed period fails even when called directly on the
/// domain service — and so does a period with no row at all (the guard
/// fails closed). The override path guards the period it names, not
/// the entry date's: with `"2026-04"` closed by then, a soft-closed
/// `"2026-13"` adjustment period still takes the person's entry dated
/// `"2026-04-10"` and still refuses the agent key's.
///
/// # Panics
///
/// Panics when any leg posts or refuses the wrong way, or when the
/// committed audit events, metered posts or entry numbers do not read
/// back exactly the four that should exist.
pub async fn period_states_gate_posting(db: &dyn Database) {
    const JOURNAL: &str = "jrn-periods";
    const AGENT: &str = "agent-periods";
    create_demo_tables(db).await.expect("create demo tables");
    // The scenario's fixture: the balanced entry, posted by `human`
    // (a person) or by an agent key.
    let posted_by = |entry_id: &str, human: bool| PostInput {
        human,
        ..balanced_input(entry_id, JOURNAL, AGENT, 1000)
    };

    // Open: a person and an agent key both post.
    open_period(db, JOURNAL, "2026-04").await;
    for (who, human) in [("person", true), ("agent", false)] {
        let input = posted_by(&format!("e-states-open-{who}"), human);
        post(db, &input, RetryPolicy::default(), no_sleep)
            .await
            .unwrap_or_else(|err| panic!("an open period admits {who}: {err}"));
    }

    // Soft-closed: the person still posts, the agent key is refused.
    set_period_status(db, JOURNAL, "2026-04", period::STATUS_SOFT_CLOSED).await;
    expect_refusal(
        db,
        &posted_by("e-states-soft-agent", false),
        "a soft-closed period refuses an agent key",
    )
    .await;
    post(
        db,
        &posted_by("e-states-soft-person", true),
        RetryPolicy::default(),
        no_sleep,
    )
    .await
    .expect("a soft-closed period still admits a person");

    // Closed: nobody posts — a person calling the domain service
    // directly is refused exactly like the agent key.
    set_period_status(db, JOURNAL, "2026-04", period::STATUS_CLOSED).await;
    for (who, human) in [("person", true), ("agent", false)] {
        expect_refusal(
            db,
            &posted_by(&format!("e-states-closed-{who}"), human),
            "a closed period refuses person and agent alike",
        )
        .await;
    }

    // No accounting_period row at all (2026-05 was never opened): the
    // guard fails closed, again for both — the derived period comes
    // from the shifted entry date.
    for (who, human) in [("person", true), ("agent", false)] {
        expect_refusal(
            db,
            &PostInput {
                entry_date: "2026-05-10".to_owned(),
                ..posted_by(&format!("e-states-unknown-{who}"), human)
            },
            "an unknown period refuses person and agent alike",
        )
        .await;
    }

    // The override path: guards "2026-13" (soft-closed), not the
    // derived "2026-04" (closed) — the agent key is refused into the
    // adjustment period, the person's entry dated into the closed
    // month still commits.
    set_period_status(db, JOURNAL, "2026-13", period::STATUS_SOFT_CLOSED).await;
    expect_refusal(
        db,
        &PostInput {
            period_override: Some("2026-13".to_owned()),
            ..posted_by("e-states-adj-agent", false)
        },
        "the override path refuses an agent key into the adjustment period",
    )
    .await;
    post(
        db,
        &PostInput {
            period_override: Some("2026-13".to_owned()),
            ..posted_by("e-states-adj-person", true)
        },
        RetryPolicy::default(),
        no_sleep,
    )
    .await
    .expect("the override path admits a person into the adjustment period");

    assert_eq!(
        audit_count(db, JOURNAL).await,
        4,
        "one audit event per committed post"
    );
    assert_eq!(
        used_for(db, AGENT).await,
        Some(4),
        "exactly the four committed posts spent allowance"
    );
    assert_eq!(
        entry_numbers(db, JOURNAL).await,
        vec![1, 2, 3, 4],
        "the four committed posts stay gapless across the refusals"
    );
}

/// Lines summing to +5 and −4 abort the batch: the balanced guard fires
/// and nothing is written.
///
/// # Panics
///
/// Panics when the post succeeds or leaves anything behind.
pub async fn unbalanced_lines_reject(db: &dyn Database) {
    const JOURNAL: &str = "jrn-unbalanced";
    create_demo_tables(db).await.expect("create demo tables");
    open_period(db, JOURNAL, "2026-04").await;

    let input = balanced_input("e-unbalanced-1", JOURNAL, "agent-unbalanced", 1000);
    let unbalanced = PostInput {
        lines: vec![
            DemoLine {
                line_no: 1,
                account_number: "1010".to_owned(),
                currency: "USD".to_owned(),
                amount_minor: 5,
            },
            DemoLine {
                line_no: 2,
                account_number: "4000".to_owned(),
                currency: "USD".to_owned(),
                amount_minor: -4,
            },
        ],
        ..input
    };
    let err = post(db, &unbalanced, RetryPolicy::default(), no_sleep)
        .await
        .expect_err("an unbalanced entry refuses the post");
    assert!(
        matches!(err, GuardError::Database(_)),
        "the fired guard is a refusal, not a conflict: {err:?}"
    );
    assert_eq!(entry_count(db, JOURNAL).await, 0, "no entry survived");
    assert_eq!(audit_count(db, JOURNAL).await, 0, "no audit event survived");
    assert_eq!(
        used_for(db, "agent-unbalanced").await,
        None,
        "the refused post spent no allowance"
    );
}

/// Lines balancing per currency but in two currencies abort the batch:
/// the single-currency half of the guard fires.
///
/// # Panics
///
/// Panics when the post succeeds or leaves anything behind.
pub async fn mixed_currency_rejects(db: &dyn Database) {
    const JOURNAL: &str = "jrn-mixed";
    create_demo_tables(db).await.expect("create demo tables");
    open_period(db, JOURNAL, "2026-04").await;

    let input = balanced_input("e-mixed-1", JOURNAL, "agent-mixed", 1000);
    let mixed = PostInput {
        lines: vec![
            DemoLine {
                line_no: 1,
                account_number: "1010".to_owned(),
                currency: "USD".to_owned(),
                amount_minor: 100,
            },
            DemoLine {
                line_no: 2,
                account_number: "4000".to_owned(),
                currency: "EUR".to_owned(),
                amount_minor: -100,
            },
        ],
        ..input
    };
    let err = post(db, &mixed, RetryPolicy::default(), no_sleep)
        .await
        .expect_err("a mixed-currency entry refuses the post");
    assert!(
        matches!(err, GuardError::Database(_)),
        "the fired guard is a refusal, not a conflict: {err:?}"
    );
    assert_eq!(entry_count(db, JOURNAL).await, 0, "no entry survived");
    assert_eq!(audit_count(db, JOURNAL).await, 0, "no audit event survived");
}

/// A daily limit of two admits exactly two posts and refuses the third
/// — the harness's guarded upsert spending in the same batch as the
/// work — and the refusal is a `Database` error (the `NOT NULL` on
/// `used`), never a retried-into-conflict one.
///
/// # Panics
///
/// Panics when the third post succeeds, fails the wrong way, or the
/// meter does not read exactly the limit.
pub async fn daily_limit_rejects(db: &dyn Database) {
    const JOURNAL: &str = "jrn-limit";
    create_demo_tables(db).await.expect("create demo tables");
    open_period(db, JOURNAL, "2026-04").await;

    for n in 1..=2 {
        let input = balanced_input(&format!("e-limit-{n}"), JOURNAL, "agent-limit", 2);
        post(db, &input, RetryPolicy::default(), no_sleep)
            .await
            .unwrap_or_else(|err| panic!("post {n} within the limit: {err}"));
    }
    let input = balanced_input("e-limit-3", JOURNAL, "agent-limit", 2);
    let err = post(db, &input, RetryPolicy::default(), no_sleep)
        .await
        .expect_err("the third post exceeds the daily limit");
    assert!(
        matches!(err, GuardError::Database(_)),
        "a spent allowance is a NOT NULL refusal, never retried into a conflict: {err:?}"
    );

    assert_eq!(
        used_for(db, "agent-limit").await,
        Some(2),
        "the meter reads exactly the limit"
    );
    assert_eq!(entry_count(db, JOURNAL).await, 2, "exactly two entries");
}

/// Two posts append two linked events; a deterministic fork — two
/// appends built from the same stale head — loses to the primary key,
/// and the loser's error is recognised as the unique violation the
/// retry policy exists for.
///
/// # Panics
///
/// Panics when the chain does not read back linked, or the forked
/// append does not fail as a unique violation.
pub async fn chain_appends_and_retries(db: &dyn Database) {
    const JOURNAL: &str = "jrn-chain";
    create_demo_tables(db).await.expect("create demo tables");
    open_period(db, JOURNAL, "2026-04").await;

    for n in 1..=2 {
        let input = balanced_input(&format!("e-chain-{n}"), JOURNAL, "agent-chain", 1000);
        post(db, &input, RetryPolicy::default(), no_sleep)
            .await
            .unwrap_or_else(|err| panic!("post {n} onto the chain: {err}"));
    }

    let head = chain::read_head(db, JOURNAL).await.expect("read the head");
    assert_eq!(head.seq, 2, "two appends advanced the head to seq 2");
    assert_linked(db, JOURNAL).await;

    // The deterministic fork: both statements were planned from the
    // same (now stale) head. The first commits; the second claims an
    // already-claimed seq and must fail as a unique violation — the
    // failure class retry::run_batch_with_retry retries.
    let stale = chain::read_head(db, JOURNAL)
        .await
        .expect("read the head again");
    let first = chain::append_statement(JOURNAL, &stale, "fork-a", "fork-a", AUDIT_TIME);
    let second = chain::append_statement(JOURNAL, &stale, "fork-b", "fork-b", AUDIT_TIME);
    db.execute(&first)
        .await
        .expect("the first stale append commits");
    let err = db
        .execute(&second)
        .await
        .expect_err("the second stale append forks the chain");
    assert!(
        retry::is_unique_violation(&err),
        "the fork must surface as the retriable unique violation: {err:?}"
    );
}

/// A conflict that never clears exhausts the budget and comes back as
/// [`GuardError::Conflict`] — after exactly `max_attempts − 1` sleeps.
///
/// # Panics
///
/// Panics when the exhausted retry is not a `Conflict` or the sleep
/// count is wrong.
pub async fn retry_exhaustion_is_a_conflict(db: &dyn Database) {
    // One probe row seeded, then a batch that re-asserts the same key:
    // a unique violation on every attempt, by construction.
    db.execute(&guard::assert_statement("stuck", "1 = 1", Vec::new()))
        .await
        .expect("seed the stuck probe row");

    let mut sleeps = 0u32;
    let policy = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
    };
    let err = retry::run_batch_with_retry(
        db,
        policy,
        async || Ok(vec![guard::assert_statement("stuck", "1 = 1", Vec::new())]),
        |_| sleeps += 1,
    )
    .await
    .expect_err("the conflict never clears");
    assert!(
        matches!(err, GuardError::Conflict { attempts: 3 }),
        "exhaustion is a Conflict naming the attempts: {err:?}"
    );
    assert_eq!(sleeps, 2, "one sleep per gap between the three attempts");

    db.execute(&guard::release_statement("stuck"))
        .await
        .expect("clean the probe row");
}

/// The migration's `agent_usage` DDL is byte-identical to what the
/// harness's `Usage` generates, and the migrated table really is the
/// one the meter writes.
///
/// # Panics
///
/// Panics when the const, the generated DDL and the migrated table
/// disagree.
pub async fn usage_table_matches(db: &dyn Database) {
    assert_eq!(
        limit::USAGE_TABLE_SQL,
        cratefield_core::Usage::new(limit::AGENT_USAGE_TABLE).create_table_sql(),
        "the migration DDL const must equal the harness's generated DDL"
    );
    let migration = include_str!("../migrations/sqlite/0001_guards.sql");
    assert!(
        migration.contains(limit::USAGE_TABLE_SQL),
        "the migration must carry the DDL verbatim"
    );

    // The migrated table really is the one the meter writes: queryable,
    // and holding no row for a subject no scenario ever used.
    let rows = db
        .query(&Statement::with_values(
            format!(
                "SELECT COUNT(*) AS n FROM {} WHERE subject = ?",
                limit::AGENT_USAGE_TABLE
            ),
            vec![SeaValue::from("agent-unused")],
        ))
        .await
        .expect("query the migrated usage table");
    let n = rows
        .first()
        .and_then(|row| row.get::<i64>("n"))
        .expect("a count row");
    assert_eq!(n, 0, "no usage row for a subject that never posted");
}

/// The scenario sleep: nothing. The scenarios are sequential, so there
/// is nothing to back off from, and the library never touches
/// `std::thread` (the concurrency tests pass the real sleeper).
fn no_sleep(_: Duration) {}

/// Posts `input` expecting the refusal: the error must be a fired
/// guard ([`GuardError::Database`] — the probe's `NOT NULL`), never a
/// conflict, or the scenario panics with `why`.
///
/// # Panics
///
/// Panics when the post succeeds or fails as anything but a refusal.
async fn expect_refusal(db: &dyn Database, input: &PostInput, why: &str) {
    let err = post(db, input, RetryPolicy::default(), no_sleep)
        .await
        .expect_err(why);
    assert!(
        matches!(err, GuardError::Database(_)),
        "the refusal is a fired guard, not a conflict: {err:?}"
    );
}

/// A balanced two-line entry in USD under `journal` and `agent`, posted
/// on the scenario fixture date.
fn balanced_input(entry_id: &str, journal: &str, agent: &str, daily_limit: u64) -> PostInput {
    PostInput {
        entry_id: entry_id.to_owned(),
        journal_id: journal.to_owned(),
        fiscal_year: 2026,
        entry_date: "2026-04-10".to_owned(),
        created_at: AUDIT_TIME.to_owned(),
        lines: vec![
            DemoLine {
                line_no: 1,
                account_number: "1010".to_owned(),
                currency: "USD".to_owned(),
                amount_minor: 100,
            },
            DemoLine {
                line_no: 2,
                account_number: "4000".to_owned(),
                currency: "USD".to_owned(),
                amount_minor: -100,
            },
        ],
        human: true,
        period_override: None,
        agent_subject: agent.to_owned(),
        daily_limit,
    }
}

/// Marks a period open for `journal` (the demo collapses the company
/// scope onto the journal id).
async fn open_period(db: &dyn Database, journal: &str, period: &str) {
    set_period_status(db, journal, period, period::STATUS_OPEN).await;
}

/// Sets a period's status for `journal` (the demo collapses the company
/// scope onto the journal id).
///
/// # Panics
///
/// Panics when the status write fails.
async fn set_period_status(db: &dyn Database, journal: &str, period: &str, status: &str) {
    db.execute(&period::set_status_statement(journal, period, status))
        .await
        .unwrap_or_else(|err| panic!("set {period} to {status} for {journal}: {err}"));
}

/// The UTC midnight instant of a `"YYYY-MM-DD"` date — the usage
/// meter's day anchor, derived from the entry so a retry re-plans the
/// same window.
fn posting_day(entry_date: &str) -> Result<OffsetDateTime, DbError> {
    let year: i32 = entry_date[0..4]
        .parse()
        .map_err(|_| DbError::Query("entry_date year is not a number".to_owned()))?;
    let month_num: u32 = entry_date[5..7]
        .parse()
        .map_err(|_| DbError::Query("entry_date month is not a number".to_owned()))?;
    let day_num: u8 = entry_date[8..10]
        .parse()
        .map_err(|_| DbError::Query("entry_date day is not a number".to_owned()))?;
    let month = Month::try_from(
        u8::try_from(month_num)
            .map_err(|_| DbError::Query("entry_date month is out of range".to_owned()))?,
    )
    .map_err(|_| DbError::Query("entry_date month is not 1..=12".to_owned()))?;
    let date = Date::from_calendar_date(year, month, day_num)
        .map_err(|_| DbError::Query("entry_date is not a calendar date".to_owned()))?;
    Ok(PrimitiveDateTime::new(date, Time::MIDNIGHT).assume_utc())
}

/// The demo's stand-in hash: opaque text, distinct per seq and entry
/// (the real hash is `ledgers-audit`'s job; here only linkage matters).
fn demo_hash(seq: i64, entry_id: &str) -> String {
    format!("demo:{seq}:{entry_id}")
}

/// `demo_entry.number` for one journal, ascending.
async fn entry_numbers(db: &dyn Database, journal: &str) -> Vec<i64> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT number FROM demo_entry WHERE journal_id = ? ORDER BY number".to_owned(),
            vec![SeaValue::from(journal)],
        ))
        .await
        .unwrap_or_else(|err| panic!("read entry numbers for {journal}: {err}"));
    rows.rows
        .iter()
        .map(|row| row.get::<i64>("number").expect("number is an integer"))
        .collect()
}

/// `COUNT(*)` of `demo_entry` for one journal.
async fn entry_count(db: &dyn Database, journal: &str) -> i64 {
    count_where(db, "demo_entry", "journal_id", journal).await
}

/// `COUNT(*)` of `audit_event` for one company scope.
async fn audit_count(db: &dyn Database, company_id: &str) -> i64 {
    count_where(db, "audit_event", "company_id", company_id).await
}

/// `COUNT(*)` over `table` filtered by one TEXT column.
async fn count_where(db: &dyn Database, table: &str, column: &str, value: &str) -> i64 {
    let rows = db
        .query(&Statement::with_values(
            format!("SELECT COUNT(*) AS n FROM {table} WHERE {column} = ?"),
            vec![SeaValue::from(value)],
        ))
        .await
        .unwrap_or_else(|err| panic!("count {table} by {column}: {err}"));
    rows.first()
        .and_then(|row| row.get::<i64>("n"))
        .expect("a count row with an integer n")
}

/// The counter's `next_number` for `(journal, fiscal_year)`, or `None`
/// when the row does not exist.
async fn counter_value(db: &dyn Database, journal: &str, fiscal_year: i64) -> Option<i64> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT next_number FROM numbering_counter\n\
             WHERE journal_id = ? AND fiscal_year = ?"
                .to_owned(),
            vec![SeaValue::from(journal), SeaValue::BigInt(Some(fiscal_year))],
        ))
        .await
        .expect("read the numbering counter");
    rows.first().and_then(|row| row.get::<i64>("next_number"))
}

/// The `used` value of `(subject, "post")`, or `None` when the meter
/// has no row for the subject.
async fn used_for(db: &dyn Database, subject: &str) -> Option<i64> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT used FROM agent_usage WHERE subject = ? AND meter = ?".to_owned(),
            vec![SeaValue::from(subject), SeaValue::from(METER_POST)],
        ))
        .await
        .expect("read the usage meter");
    rows.first().and_then(|row| row.get::<i64>("used"))
}

/// Walks the chain in `seq` order and asserts it is linked: seqs run
/// from 1, the first `prev_hash` is the genesis anchor, and every later
/// `prev_hash` is the previous row's `hash`.
async fn assert_linked(db: &dyn Database, company_id: &str) {
    let rows = db
        .query(&Statement::with_values(
            "SELECT seq, prev_hash, hash FROM audit_event WHERE company_id = ? ORDER BY seq"
                .to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await
        .expect("read the chain back");
    let mut previous_hash = GENESIS_HASH.to_owned();
    for (index, row) in rows.rows.iter().enumerate() {
        let seq = row.get::<i64>("seq").expect("seq is an integer");
        let prev_hash = row.get::<String>("prev_hash").expect("prev_hash is text");
        let hash = row.get::<String>("hash").expect("hash is text");
        let expected_seq = i64::try_from(index + 1).expect("a small chain");
        assert_eq!(seq, expected_seq, "seqs run 1..=n with no gaps");
        assert_eq!(
            prev_hash, previous_hash,
            "row {seq} links to the hash before it"
        );
        previous_hash = hash;
    }
}
