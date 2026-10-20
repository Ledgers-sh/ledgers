//! Tests for [`post_entry`] outcome contracts: idempotent resubmits,
//! recovery from ambiguous outcomes, and deterministic number assignment.

mod common;

use common::{COMPANY_ID, FY, NOW, POSTED_AT, counter_value, create_chart, open_period};
use cratefield_core::{Database, DbError, Rows, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::{DbModule, RetryPolicy};
use ledgers_ledger::journal::{self, JournalDraft, JournalKind};
use ledgers_ledger::posting::{PostContext, TOPIC_ENTRY_POSTED, post_entry};
use ledgers_ledger::{Entry, EntryDraft, JournalError, LedgerModule, Line, Side};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use time::{Date, Month};

/// Two test journals for rejecting cross-journal reuse.
const JOURNAL_1: &str = "01JRN00000000000000000000000001";
const JOURNAL_2: &str = "01JRN00000000000000000000000002";

#[test]
fn idempotent_resubmit_returns_same_entry_and_posts_nothing_new() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(LedgerModule), Box::new(DbModule)]) {
        pollster::block_on(async {
            set_up(&kit.db, JOURNAL_1).await;
            let entry = the_entry(JOURNAL_1);
            let ctx = test_ctx(
                "01ENTRYIDEM0000000000000000000",
                "01OUTBOX00000000000000000000",
            );
            let policy = RetryPolicy::default();

            let first = post_entry(kit.db.as_ref(), &entry, &ctx, policy, |_| {})
                .await
                .expect("post");

            for _ in 0..3 {
                let result = post_entry(kit.db.as_ref(), &entry, &ctx, policy, |_| {})
                    .await
                    .expect("resubmit");
                assert_eq!(result, first);
            }

            assert_eq!(
                pollster::block_on(counter_value(kit.db.as_ref(), JOURNAL_1, FY)),
                Some(1)
            );
            assert_eq!(
                pollster::block_on(count_entries(kit.db.as_ref(), JOURNAL_1)),
                1
            );
            assert_eq!(
                pollster::block_on(count_chain_events(kit.db.as_ref(), COMPANY_ID)),
                1
            );
            assert_eq!(pollster::block_on(count_outbox(kit.db.as_ref())), 1);
        });
    }
}

#[test]
fn entry_id_already_used_in_different_journal_is_rejected() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(LedgerModule), Box::new(DbModule)]) {
        pollster::block_on(async {
            set_up(&kit.db, JOURNAL_1).await;
            set_up_journal(&kit.db, JOURNAL_2).await;
            let eid = "01ENTRYCROSSJOURNAL000000000";
            let ctx1 = test_ctx(eid, "01OUTBOX1111111111111111111111");
            let ctx2 = test_ctx(eid, "01OUTBOX2222222222222222222222");
            let policy = RetryPolicy::default();

            post_entry(
                kit.db.as_ref(),
                &the_entry(JOURNAL_1),
                &ctx1,
                policy,
                |_| {},
            )
            .await
            .expect("post");

            let err = post_entry(
                kit.db.as_ref(),
                &the_entry(JOURNAL_2),
                &ctx2,
                policy,
                |_| {},
            )
            .await
            .expect_err("cross-journal reuse fails");
            assert!(matches!(err, JournalError::EntryIdAlreadyUsed { .. }));

            assert_eq!(
                pollster::block_on(count_entries(kit.db.as_ref(), JOURNAL_1)),
                1
            );
            assert_eq!(
                pollster::block_on(count_entries(kit.db.as_ref(), JOURNAL_2)),
                0
            );
        });
    }
}

#[test]
fn a_failure_after_commit_still_reports_the_number() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(LedgerModule), Box::new(DbModule)]) {
        pollster::block_on(async {
            set_up(&kit.db, JOURNAL_1).await;
            let entry = the_entry(JOURNAL_1);
            let policy = RetryPolicy::default();

            let w1 = FailureWrapper::new(kit.db.clone(), FailMode::AfterCommit);
            let ctx1 = test_ctx(
                "01ENTRY_FAIL_AFTER_COMMIT_0001",
                "01OUTBOX_FAIL_AFTER_COMMIT_001",
            );
            let r1 = post_entry(&w1, &entry, &ctx1, policy, |_| {})
                .await
                .expect("post");
            assert_eq!(r1.number, 1);
            assert!(w1.committed.load(Ordering::SeqCst));

            let w2 = FailureWrapper::new(kit.db.clone(), FailMode::AfterCommit);
            let ctx2 = test_ctx(
                "01ENTRY_FAIL_AFTER_COMMIT_0002",
                "01OUTBOX_FAIL_AFTER_COMMIT_002",
            );
            let r2 = post_entry(&w2, &entry, &ctx2, policy, |_| {})
                .await
                .expect("second");
            assert_eq!(r2.number, 2);
        });
    }
}

#[test]
fn an_ambiguous_commit_resubmitted_with_the_same_id_returns_the_original() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(LedgerModule), Box::new(DbModule)]) {
        pollster::block_on(async {
            set_up(&kit.db, JOURNAL_1).await;
            let entry = the_entry(JOURNAL_1);
            let ctx = test_ctx(
                "01ENTRY_AMBIGUOUS_COMMIT_000001",
                "01OUTBOX_AMBIGUOUS_COMMIT_001",
            );
            let policy = RetryPolicy::default();

            let wrapper = FailureWrapper::new(kit.db.clone(), FailMode::AfterBatch);
            let err = post_entry(&wrapper, &entry, &ctx, policy, |_| {})
                .await
                .expect_err("fails");
            assert!(matches!(err, JournalError::Database(_)));

            let result = post_entry(kit.db.as_ref(), &entry, &ctx, policy, |_| {})
                .await
                .expect("resubmit");
            assert_eq!(result.number, 1);
            assert_eq!(result.entry_id, ctx.entry_id);

            assert_eq!(
                pollster::block_on(count_entries(kit.db.as_ref(), JOURNAL_1)),
                1
            );
            assert_eq!(
                pollster::block_on(counter_value(kit.db.as_ref(), JOURNAL_1, FY)),
                Some(1)
            );
            assert_eq!(
                pollster::block_on(count_chain_events(kit.db.as_ref(), COMPANY_ID)),
                1
            );
            assert_eq!(pollster::block_on(count_outbox(kit.db.as_ref())), 1);
        });
    }
}

/// Test wrapper that injects failures at specific points.
/// `FailMode::AfterCommit`: fails all queries after batch succeeds.
/// `FailMode::AfterBatch`: fails immediately after `batch_atomic` succeeds.
enum FailMode {
    AfterCommit,
    AfterBatch,
}

struct FailureWrapper {
    inner: Arc<dyn Database>,
    mode: FailMode,
    committed: Arc<AtomicBool>,
}

impl FailureWrapper {
    fn new(inner: Arc<dyn Database>, mode: FailMode) -> Self {
        Self {
            inner,
            mode,
            committed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Database for FailureWrapper {
    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        stmt: &'life1 Statement,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<u64, DbError>> + Send + 'async_trait>,
    >
    where
        Self: 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
    {
        let committed = self.committed.clone();
        let inner = self.inner.clone();
        let stmt = stmt.clone();
        let is_after_commit = matches!(self.mode, FailMode::AfterCommit);
        Box::pin(async move {
            if is_after_commit && committed.load(Ordering::SeqCst) {
                Err(DbError::Query("injected: pool timed out".into()))
            } else {
                inner.execute(&stmt).await
            }
        })
    }

    fn query<'life0, 'life1, 'async_trait>(
        &'life0 self,
        stmt: &'life1 Statement,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Rows, DbError>> + Send + 'async_trait>,
    >
    where
        Self: 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
    {
        let committed = self.committed.clone();
        let inner = self.inner.clone();
        let stmt = stmt.clone();
        let is_after_commit = matches!(self.mode, FailMode::AfterCommit);
        Box::pin(async move {
            if is_after_commit && committed.load(Ordering::SeqCst) {
                Err(DbError::Query("injected: pool timed out".into()))
            } else {
                inner.query(&stmt).await
            }
        })
    }

    fn batch_atomic<'life0, 'life1, 'async_trait>(
        &'life0 self,
        stmts: &'life1 [Statement],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), DbError>> + Send + 'async_trait>,
    >
    where
        Self: 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
    {
        let committed = self.committed.clone();
        let inner = self.inner.clone();
        let stmts = stmts.to_vec();
        let is_after_commit = matches!(self.mode, FailMode::AfterCommit);
        let is_after_batch = matches!(self.mode, FailMode::AfterBatch);
        Box::pin(async move {
            let result = inner.batch_atomic(&stmts).await;
            match (result, is_after_commit, is_after_batch) {
                (Ok(()), _, true) => Err(DbError::Batch("injected: connection reset".into())),
                (Ok(()), true, false) => {
                    committed.store(true, Ordering::SeqCst);
                    Ok(())
                }
                (Ok(()), false, false) => Ok(()),
                (Err(e), _, _) => Err(e),
            }
        })
    }
}

async fn set_up(db: &Arc<dyn Database>, journal_id: &str) {
    create_chart(db.as_ref(), COMPANY_ID).await;
    set_up_journal(db, journal_id).await;
}

async fn set_up_journal(db: &Arc<dyn Database>, journal_id: &str) {
    let code = if journal_id == JOURNAL_1 { "J1" } else { "J2" };
    journal::create_journal(
        db.as_ref(),
        COMPANY_ID,
        JournalDraft {
            code: code.to_owned(),
            kind: JournalKind::General,
            name: format!("Test {code}"),
        },
        journal_id,
        NOW,
    )
    .await
    .expect("create journal");
    open_period(db.as_ref(), COMPANY_ID, "2026-04").await;
}

fn the_entry(journal_id: &str) -> Entry {
    let line = |account: &str, side: Side| Line {
        account_number: account.to_owned(),
        side,
        base: Amount::from_major(1, Currency::from_code("EUR").expect("EUR")).expect("amount"),
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    };
    Entry::from_draft(
        EntryDraft {
            journal_id: journal_id.to_owned(),
            document_date: Date::from_calendar_date(2026, Month::April, 10).expect("date"),
            posting_date: Date::from_calendar_date(2026, Month::April, 10).expect("date"),
            tax_point_date: None,
            base_currency: Currency::from_code("EUR").expect("EUR"),
            description: "Test".to_owned(),
            source_documents: Vec::new(),
            reverses: None,
            lines: vec![line("1010", Side::Debit), line("4000", Side::Credit)],
        },
        COMPANY_ID,
    )
    .expect("entry validates")
}

fn test_ctx(entry_id: &str, outbox_id: &str) -> PostContext {
    PostContext {
        fiscal_year: FY,
        entry_id: entry_id.to_owned(),
        recorded_at: POSTED_AT.to_owned(),
        outbox_id: outbox_id.to_owned(),
    }
}

async fn count_rows(db: &dyn Database, sql: &str, values: Vec<SeaValue>) -> i64 {
    db.query(&Statement::with_values(
        format!("SELECT COUNT(*) AS n {sql}"),
        values,
    ))
    .await
    .ok()
    .and_then(|rows| rows.first().and_then(|row| row.get::<i64>("n")))
    .unwrap_or(0)
}

async fn count_entries(db: &dyn Database, journal_id: &str) -> i64 {
    count_rows(
        db,
        "FROM journal_entry WHERE journal_id = ?",
        vec![SeaValue::from(journal_id)],
    )
    .await
}

async fn count_outbox(db: &dyn Database) -> i64 {
    count_rows(
        db,
        "FROM outbox WHERE topic = ?",
        vec![SeaValue::from(TOPIC_ENTRY_POSTED)],
    )
    .await
}

async fn count_chain_events(db: &dyn Database, company_id: &str) -> i64 {
    count_rows(
        db,
        "FROM audit_event WHERE company_id = ?",
        vec![SeaValue::from(company_id)],
    )
    .await
}
