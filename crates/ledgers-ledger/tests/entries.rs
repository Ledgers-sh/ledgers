//! Entry lifecycle: idempotency, reversals, dry run, drafts, recurring.
//! Minimal acceptance tests following journal.rs pattern.

mod common;
use common::*;
use cratefield_core::Database;
use ledgers_db::retry::RetryPolicy;
use ledgers_ledger::recurring::{CreateRecurringContext, Schedule};
use ledgers_ledger::{Entry, EntryDraft, Line, PostContext, PostedEntry};
use ledgers_ledger::{ReverseContext, reverse_entry};
use time::{Date, Month};

/// Creates a test journal with standard properties.
async fn create_test_journal(db: &dyn Database, company_id: &str, journal_id: &str) {
    db.execute(&cratefield_core::Statement::with_values(
        "INSERT INTO journal (id, company_id, code, kind, name, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            sea_query::Value::from(journal_id),
            sea_query::Value::from(company_id),
            sea_query::Value::from("GN"),
            sea_query::Value::from("general"),
            sea_query::Value::from("Gen"),
            sea_query::Value::from(NOW),
        ],
    ))
    .await
    .unwrap();
}

/// Creates a standard entry draft: 1010 debit, 1400 credit, 10,000 EUR each.
fn standard_entry_draft(journal_id: &str, description: &str) -> EntryDraft {
    EntryDraft {
        journal_id: journal_id.to_owned(),
        document_date: time::Date::from_calendar_date(2026, Month::February, 1).unwrap(),
        posting_date: time::Date::from_calendar_date(2026, Month::February, 1).unwrap(),
        tax_point_date: None,
        base_currency: ledgers_money::Currency::from_code("EUR").unwrap(),
        description: description.to_owned(),
        source_documents: vec![],
        external_id: None,
        source_app: None,
        reverses: None,
        lines: vec![
            Line {
                account_number: "1010".to_owned(),
                side: ledgers_ledger::Side::Debit,
                base: ledgers_money::Amount::new(
                    10_000,
                    ledgers_money::Currency::from_code("EUR").unwrap(),
                ),
                fx: None,
                tax_code: None,
                dimensions: std::collections::BTreeMap::new(),
                memo: None,
            },
            Line {
                account_number: "1400".to_owned(),
                side: ledgers_ledger::Side::Credit,
                base: ledgers_money::Amount::new(
                    10_000,
                    ledgers_money::Currency::from_code("EUR").unwrap(),
                ),
                fx: None,
                tax_code: None,
                dimensions: std::collections::BTreeMap::new(),
                memo: None,
            },
        ],
    }
}

/// Posts an entry with standard `PostContext`.
async fn post_test_entry(
    db: &dyn Database,
    entry: &Entry,
    entry_id: &str,
    outbox_id: &str,
) -> Result<PostedEntry, ledgers_ledger::JournalError> {
    let ctx = PostContext {
        fiscal_year: FY,
        entry_id: entry_id.to_owned(),
        recorded_at: POSTED_AT.to_owned(),
        outbox_id: outbox_id.to_owned(),
    };
    ledgers_ledger::post_entry(db, entry, &ctx, RetryPolicy::default(), |_| {}, &agent()).await
}

/// Creates reversal lines from an entry (flipped sides).
fn reversal_lines(entry: &Entry) -> Vec<Line> {
    entry
        .lines
        .iter()
        .map(|line| Line {
            side: line.side.flip(),
            ..line.clone()
        })
        .collect()
}

#[test]
fn same_idempotency_key_posts_once() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;

        db.execute(&cratefield_core::Statement::with_values(
            "INSERT INTO journal (id, company_id, code, kind, name, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)"
                .to_owned(),
            vec![
                sea_query::Value::from("j1"),
                sea_query::Value::from(COMPANY_ID),
                sea_query::Value::from("GN"),
                sea_query::Value::from("general"),
                sea_query::Value::from("Gen"),
                sea_query::Value::from(NOW),
            ],
        ))
        .await
        .unwrap();

        let draft = ledgers_ledger::EntryDraft {
            journal_id: "j1".to_owned(),
            document_date: time::Date::from_calendar_date(2026, time::Month::February, 1).unwrap(),
            posting_date: time::Date::from_calendar_date(2026, time::Month::February, 1).unwrap(),
            tax_point_date: None,
            base_currency: ledgers_money::Currency::from_code("EUR").unwrap(),
            description: "test".to_owned(),
            source_documents: vec![],
            external_id: None,
            source_app: None,
            reverses: None,
            lines: vec![
                ledgers_ledger::Line {
                    account_number: "1010".to_owned(),
                    side: ledgers_ledger::Side::Debit,
                    base: ledgers_money::Amount::new(
                        10_000,
                        ledgers_money::Currency::from_code("EUR").unwrap(),
                    ),
                    fx: None,
                    tax_code: None,
                    dimensions: std::collections::BTreeMap::new(),
                    memo: None,
                },
                ledgers_ledger::Line {
                    account_number: "1400".to_owned(),
                    side: ledgers_ledger::Side::Credit,
                    base: ledgers_money::Amount::new(
                        10_000,
                        ledgers_money::Currency::from_code("EUR").unwrap(),
                    ),
                    fx: None,
                    tax_code: None,
                    dimensions: std::collections::BTreeMap::new(),
                    memo: None,
                },
            ],
        };

        let entry = ledgers_ledger::Entry::from_draft(draft, COMPANY_ID).unwrap();
        let entry_id = "e1";
        let ctx = ledgers_ledger::PostContext {
            fiscal_year: FY,
            entry_id: entry_id.to_owned(),
            recorded_at: POSTED_AT.to_owned(),
            outbox_id: "o1".to_owned(),
        };

        let r1 =
            ledgers_ledger::post_entry(db, &entry, &ctx, RetryPolicy::default(), |_| {}, &agent())
                .await
                .unwrap();

        let r2 =
            ledgers_ledger::post_entry(db, &entry, &ctx, RetryPolicy::default(), |_| {}, &agent())
                .await
                .unwrap();

        assert_eq!(r1.number, r2.number);
        let entry_count = count(db, "journal_entry", "id", entry_id).await;
        assert_eq!(entry_count, 1);
    });
}

#[test]
fn dry_run_returns_provisional_number() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;

        db.execute(&cratefield_core::Statement::with_values(
            "INSERT INTO journal (id, company_id, code, kind, name, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)"
                .to_owned(),
            vec![
                sea_query::Value::from("j1"),
                sea_query::Value::from(COMPANY_ID),
                sea_query::Value::from("GN"),
                sea_query::Value::from("general"),
                sea_query::Value::from("Gen"),
                sea_query::Value::from(NOW),
            ],
        ))
        .await
        .unwrap();

        let draft = ledgers_ledger::EntryDraft {
            journal_id: "j1".to_owned(),
            document_date: time::Date::from_calendar_date(2026, time::Month::February, 1).unwrap(),
            posting_date: time::Date::from_calendar_date(2026, time::Month::February, 1).unwrap(),
            tax_point_date: None,
            base_currency: ledgers_money::Currency::from_code("EUR").unwrap(),
            description: "test".to_owned(),
            source_documents: vec![],
            external_id: None,
            source_app: None,
            reverses: None,
            lines: vec![
                ledgers_ledger::Line {
                    account_number: "1010".to_owned(),
                    side: ledgers_ledger::Side::Debit,
                    base: ledgers_money::Amount::new(
                        10_000,
                        ledgers_money::Currency::from_code("EUR").unwrap(),
                    ),
                    fx: None,
                    tax_code: None,
                    dimensions: std::collections::BTreeMap::new(),
                    memo: None,
                },
                ledgers_ledger::Line {
                    account_number: "1400".to_owned(),
                    side: ledgers_ledger::Side::Credit,
                    base: ledgers_money::Amount::new(
                        10_000,
                        ledgers_money::Currency::from_code("EUR").unwrap(),
                    ),
                    fx: None,
                    tax_code: None,
                    dimensions: std::collections::BTreeMap::new(),
                    memo: None,
                },
            ],
        };

        let entry = ledgers_ledger::Entry::from_draft(draft, COMPANY_ID).unwrap();
        let ctx = ledgers_ledger::PostContext {
            fiscal_year: FY,
            entry_id: "dry1".to_owned(),
            recorded_at: POSTED_AT.to_owned(),
            outbox_id: "o1".to_owned(),
        };

        let (_returned_entry, provisional) =
            ledgers_ledger::posting::dry_run_entry(db, &entry, &ctx)
                .await
                .unwrap();

        assert!(provisional >= 1);
        let entry_count = count(db, "journal_entry", "id", "dry1").await;
        assert_eq!(entry_count, 0);
    });
}

#[test]
fn closed_period_entry_reverses_into_first_open_period() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;
        open_period(db, COMPANY_ID, "2026-03").await;

        create_test_journal(db, COMPANY_ID, "j1").await;

        let draft = standard_entry_draft("j1", "test");
        let entry = Entry::from_draft(draft, COMPANY_ID).unwrap();
        let posted = post_test_entry(db, &entry, "e1", "o1").await.unwrap();

        // Close period 2026-02
        db.execute(&ledgers_db::period::set_status_statement(
            COMPANY_ID, "2026-02", "closed",
        ))
        .await
        .unwrap();

        // Try to reverse the entry; it should post to March (first open period)
        let reverse_ctx = ReverseContext {
            company_id: COMPANY_ID.to_owned(),
            entry_id: posted.entry_id.clone(),
            requested_posting_date: time::Date::from_calendar_date(2026, Month::February, 1)
                .unwrap(),
            human: true,
        };

        let post_ctx = PostContext {
            fiscal_year: FY,
            entry_id: "e2".to_owned(),
            recorded_at: POSTED_AT.to_owned(),
            outbox_id: "o2".to_owned(),
        };

        let reversal_posted = reverse_entry(
            db,
            &reverse_ctx,
            &post_ctx,
            RetryPolicy::default(),
            |_| {},
            &agent(),
        )
        .await
        .expect("reversal posts to next open period");

        // Check that reversal's posting_date is in March
        let rows = db
            .query(&cratefield_core::Statement::with_values(
                "SELECT posting_date FROM journal_entry WHERE id = ?".to_owned(),
                vec![sea_query::Value::from(reversal_posted.entry_id.as_str())],
            ))
            .await
            .unwrap();

        let posting_date_str = rows
            .first()
            .and_then(|r| r.get::<String>("posting_date"))
            .unwrap();

        assert!(
            posting_date_str.starts_with("2026-03"),
            "reversal posting_date should be in March, got {posting_date_str}"
        );
    });
}

#[test]
fn reversal_of_reversal_is_rejected() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;

        create_test_journal(db, COMPANY_ID, "j1").await;

        // Post original
        let original_draft = standard_entry_draft("j1", "orig");
        let original = Entry::from_draft(original_draft, COMPANY_ID).unwrap();
        let original_posted = post_test_entry(db, &original, "e1", "o1").await.unwrap();

        // Post reversal
        let reversal = Entry::from_draft(
            EntryDraft {
                reverses: Some(original_posted.entry_id.clone()),
                journal_id: "j1".to_owned(),
                document_date: original.document_date,
                posting_date: original.posting_date,
                tax_point_date: Some(original.tax_point_date),
                base_currency: original.base_currency,
                description: "reversal".to_owned(),
                source_documents: vec![],
                external_id: None,
                source_app: None,
                lines: reversal_lines(&original),
            },
            COMPANY_ID,
        )
        .unwrap();
        let _ = post_test_entry(db, &reversal, "e2", "o2").await.unwrap();

        // Try to reverse the reversal - should fail
        let reversal_of_reversal = Entry::from_draft(
            EntryDraft {
                external_id: None,
                reverses: Some("e2".to_owned()),
                journal_id: "j1".to_owned(),
                document_date: original.document_date,
                posting_date: original.posting_date,
                tax_point_date: Some(original.tax_point_date),
                base_currency: original.base_currency,
                description: "reversal of reversal".to_owned(),
                source_documents: vec![],
                source_app: None,
                lines: original.lines.clone(),
            },
            COMPANY_ID,
        )
        .unwrap();

        let err = post_test_entry(db, &reversal_of_reversal, "e3", "o3")
            .await
            .expect_err("reversal of reversal should fail");

        // Check for the specific error variant
        assert!(
            matches!(err, ledgers_ledger::JournalError::ReversalOfReversal { .. }),
            "expected ReversalOfReversal error, got {err:?}"
        );
    });
}

#[test]
fn double_reversal_is_rejected() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;

        create_test_journal(db, COMPANY_ID, "j1").await;

        // Post original
        let original_draft = standard_entry_draft("j1", "orig");
        let original = Entry::from_draft(original_draft, COMPANY_ID).unwrap();
        let original_posted = post_test_entry(db, &original, "e1", "o1").await.unwrap();

        // Post first reversal
        let reversal = Entry::from_draft(
            EntryDraft {
                reverses: Some(original_posted.entry_id.clone()),
                journal_id: "j1".to_owned(),
                document_date: original.document_date,
                posting_date: original.posting_date,
                tax_point_date: Some(original.tax_point_date),
                base_currency: original.base_currency,
                description: "first reversal".to_owned(),
                source_documents: vec![],
                external_id: None,
                source_app: None,
                lines: reversal_lines(&original),
            },
            COMPANY_ID,
        )
        .unwrap();
        let _ = post_test_entry(db, &reversal, "e2", "o2").await.unwrap();

        // Try to reverse again - should fail (guard prevents double reversals)
        let second_reversal = Entry::from_draft(
            EntryDraft {
                external_id: None,
                reverses: Some(original_posted.entry_id.clone()),
                journal_id: "j1".to_owned(),
                document_date: original.document_date,
                posting_date: original.posting_date,
                tax_point_date: Some(original.tax_point_date),
                base_currency: original.base_currency,
                description: "second reversal".to_owned(),
                source_documents: vec![],
                source_app: None,
                lines: reversal_lines(&original),
            },
            COMPANY_ID,
        )
        .unwrap();

        let err = post_test_entry(db, &second_reversal, "e3", "o3")
            .await
            .expect_err("double reversal should fail");

        // Check that it failed with AlreadyReversed
        assert!(
            matches!(err, ledgers_ledger::JournalError::AlreadyReversed { .. }),
            "expected AlreadyReversed error, got {err:?}"
        );
    });
}

#[test]
fn agent_cannot_reverse_human_posted_entry() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;

        create_test_journal(db, COMPANY_ID, "j1").await;

        // Post as person (human)
        let original_draft = standard_entry_draft("j1", "orig");
        let original = Entry::from_draft(original_draft, COMPANY_ID).unwrap();
        let ctx = PostContext {
            fiscal_year: FY,
            entry_id: "e1".to_owned(),
            recorded_at: POSTED_AT.to_owned(),
            outbox_id: "o1".to_owned(),
        };
        let original_posted = ledgers_ledger::post_entry(
            db,
            &original,
            &ctx,
            RetryPolicy::default(),
            |_| {},
            &person(),
        )
        .await
        .unwrap();

        // Verify the original entry was stored with actor='human'
        let original_rows = db
            .query(&cratefield_core::Statement::with_values(
                "SELECT actor FROM journal_entry WHERE id = ?".to_owned(),
                vec![sea_query::Value::from(&original_posted.entry_id)],
            ))
            .await
            .expect("query should succeed");
        let original_actor = original_rows
            .first()
            .and_then(|row| row.get::<String>("actor"))
            .expect("original entry should exist");
        assert_eq!(
            original_actor, "human",
            "original entry should be stored with actor='human'"
        );

        // Try to reverse as agent - should fail (human-only guard prevents it)
        let reverse_ctx = ReverseContext {
            company_id: COMPANY_ID.to_owned(),
            entry_id: original_posted.entry_id.clone(),
            requested_posting_date: time::Date::from_calendar_date(2026, Month::February, 1)
                .unwrap(),
            human: false,
        };

        let post_ctx = PostContext {
            fiscal_year: FY,
            entry_id: "e2".to_owned(),
            recorded_at: POSTED_AT.to_owned(),
            outbox_id: "o2".to_owned(),
        };

        let err = ledgers_ledger::reverse_entry(
            db,
            &reverse_ctx,
            &post_ctx,
            RetryPolicy::default(),
            |_| {},
            &agent(),
        )
        .await
        .expect_err("agent cannot reverse human entry");

        // Check that it failed with HumanOnlyReversal
        assert!(
            matches!(err, ledgers_ledger::JournalError::HumanOnlyReversal),
            "expected HumanOnlyReversal error, got {err:?}"
        );

        // Now try as person - should succeed
        let reverse_ctx_person = ReverseContext {
            company_id: COMPANY_ID.to_owned(),
            entry_id: original_posted.entry_id.clone(),
            requested_posting_date: time::Date::from_calendar_date(2026, Month::February, 1)
                .unwrap(),
            human: true,
        };

        let post_ctx_person = PostContext {
            fiscal_year: FY,
            entry_id: "e3".to_owned(),
            recorded_at: POSTED_AT.to_owned(),
            outbox_id: "o3".to_owned(),
        };

        let reversal_posted = ledgers_ledger::reverse_entry(
            db,
            &reverse_ctx_person,
            &post_ctx_person,
            RetryPolicy::default(),
            |_| {},
            &person(),
        )
        .await
        .expect("person can reverse person entry");

        assert!(reversal_posted.number > 0);
    });
}

#[test]
fn drafts_lifecycle() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;

        create_test_journal(db, COMPANY_ID, "j1").await;

        let draft = standard_entry_draft("j1", "test draft");

        // Save draft
        ledgers_ledger::draft::save_draft(db, COMPANY_ID, "draft1", &draft, NOW)
            .await
            .expect("save draft");

        // Edit draft
        let mut edited = draft.clone();
        edited.description = "edited draft".to_owned();
        ledgers_ledger::draft::edit_draft(db, COMPANY_ID, "draft1", &edited, POSTED_AT)
            .await
            .expect("edit draft");

        // Post draft
        let entry = Entry::from_draft(edited.clone(), COMPANY_ID).unwrap();
        let ctx = PostContext {
            fiscal_year: FY,
            entry_id: "draft1".to_owned(),
            recorded_at: POSTED_AT.to_owned(),
            outbox_id: "o1".to_owned(),
        };
        let posted =
            ledgers_ledger::post_entry(db, &entry, &ctx, RetryPolicy::default(), |_| {}, &agent())
                .await
                .expect("post draft");

        // Try to edit after posting - should fail
        let result =
            ledgers_ledger::draft::edit_draft(db, COMPANY_ID, "draft1", &edited, POSTED_AT).await;

        assert!(result.is_err(), "edit after post should fail");

        // Post draft again with same id - should return same number (idempotent)
        let ctx2 = PostContext {
            fiscal_year: FY,
            entry_id: "draft1".to_owned(),
            recorded_at: POSTED_AT.to_owned(),
            outbox_id: "o2".to_owned(),
        };
        let posted2 =
            ledgers_ledger::post_entry(db, &entry, &ctx2, RetryPolicy::default(), |_| {}, &agent())
                .await
                .expect("repost draft");

        assert_eq!(
            posted.number, posted2.number,
            "idempotent post should return same number"
        );

        // Check there is exactly one journal_entry row for this draft
        let entry_count = count(db, "journal_entry", "id", "draft1").await;
        assert_eq!(
            entry_count, 1,
            "posting same draft twice should leave one row"
        );
    });
}

#[test]
fn recurring_templates_create_drafts_only() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;

        create_test_journal(db, COMPANY_ID, "j1").await;

        let template_draft = standard_entry_draft("j1", "recurring invoice");
        let next_due = Date::from_calendar_date(2026, Month::February, 15).unwrap();

        // Create recurring template
        let ctx = CreateRecurringContext {
            template_id: "t1".to_owned(),
            company_id: COMPANY_ID.to_owned(),
            journal_id: "j1".to_owned(),
            name: "Monthly invoice".to_owned(),
            schedule: Schedule::Monthly,
            next_due,
            recorded_at: NOW.to_owned(),
        };

        ledgers_ledger::recurring::create_recurring(db, &ctx, &template_draft)
            .await
            .expect("create recurring");

        // Run due recurring as of the due date
        let draft_ids =
            ledgers_ledger::recurring::run_due_recurring(db, COMPANY_ID, next_due, POSTED_AT)
                .await
                .expect("run recurring");

        // Should create one draft
        assert_eq!(draft_ids.len(), 1, "should create one due draft");

        // Check that no journal_entry rows were created (drafts are unposted)
        let entry_count = count(db, "journal_entry", "id", &draft_ids[0]).await;
        assert_eq!(
            entry_count, 0,
            "recurring should create draft only, no journal_entry"
        );

        // Check that entry_draft was created
        let draft_count = count(db, "entry_draft", "id", &draft_ids[0]).await;
        assert_eq!(draft_count, 1, "recurring should create entry_draft");

        // Rerun on the same as_of - should not create anything new
        let draft_ids2 =
            ledgers_ledger::recurring::run_due_recurring(db, COMPANY_ID, next_due, POSTED_AT)
                .await
                .expect("rerun recurring");

        assert_eq!(
            draft_ids2.len(),
            0,
            "rerun on same as_of should create nothing new"
        );

        // Pause the template
        ledgers_ledger::recurring::pause_recurring(db, "t1", true)
            .await
            .expect("pause recurring");

        // Set next_due back to trigger
        db.execute(&cratefield_core::Statement::with_values(
            "UPDATE recurring_template SET next_due = ? WHERE id = ?".to_owned(),
            vec![
                sea_query::Value::from(
                    format!(
                        "{:04}-{:02}-{:02}",
                        next_due.year(),
                        u8::from(next_due.month()),
                        next_due.day()
                    )
                    .as_str(),
                ),
                sea_query::Value::from("t1"),
            ],
        ))
        .await
        .expect("reset next_due");

        // Rerun with paused template - should create nothing
        let draft_ids3 =
            ledgers_ledger::recurring::run_due_recurring(db, COMPANY_ID, next_due, POSTED_AT)
                .await
                .expect("run with paused template");

        assert_eq!(draft_ids3.len(), 0, "paused template should create nothing");
    });
}
