//! Control account posting guard: manual posting to ar/ap is rejected,
//! privileged paths are allowed, rejection leaves no trace.

mod common;
use common::*;
use cratefield_core::Database;
use ledgers_db::retry::RetryPolicy;
use ledgers_ledger::account::AccountDraft;
use ledgers_ledger::commands;
use ledgers_ledger::{Entry, EntryDraft, JournalError, Line, PostContext, PostedEntry};
use ledgers_money::{Amount, Currency};
use time::Month;

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

/// Creates an entry draft with custom lines.
fn entry_draft(journal_id: &str, description: &str, lines: Vec<Line>) -> EntryDraft {
    EntryDraft {
        journal_id: journal_id.to_owned(),
        document_date: time::Date::from_calendar_date(2026, Month::February, 1).unwrap(),
        posting_date: time::Date::from_calendar_date(2026, Month::February, 1).unwrap(),
        tax_point_date: None,
        base_currency: Currency::from_code("EUR").unwrap(),
        description: description.to_owned(),
        source_documents: vec![],
        external_id: None,
        source_app: None,
        reverses: None,
        lines,
    }
}

/// Posts an entry with standard `PostContext` and default options.
async fn post_test_entry(
    db: &dyn Database,
    entry: &Entry,
    entry_id: &str,
    outbox_id: &str,
) -> Result<PostedEntry, JournalError> {
    let ctx = PostContext {
        fiscal_year: FY,
        entry_id: entry_id.to_owned(),
        recorded_at: POSTED_AT.to_owned(),
        outbox_id: outbox_id.to_owned(),
    };
    ledgers_ledger::post_entry(db, entry, &ctx, RetryPolicy::default(), |_| {}, &agent()).await
}

/// Posts an entry with control permissions allowed.
async fn post_test_entry_with_control(
    db: &dyn Database,
    entry: &Entry,
    entry_id: &str,
    outbox_id: &str,
) -> Result<PostedEntry, JournalError> {
    let ctx = PostContext {
        fiscal_year: FY,
        entry_id: entry_id.to_owned(),
        recorded_at: POSTED_AT.to_owned(),
        outbox_id: outbox_id.to_owned(),
    };
    ledgers_ledger::posting::post_entry_with_control(
        db,
        entry,
        &ctx,
        RetryPolicy::default(),
        |_| {},
        &agent(),
    )
    .await
}

#[test]
fn manual_post_to_ar_account_is_rejected() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();

        mount(db, COMPANY_ID).await;
        create_test_journal(db, COMPANY_ID, "jn-01").await;
        open_period(db, COMPANY_ID, "2026-02").await;

        // Create an ar-tagged account
        let ar_account = AccountDraft {
            number: "1200".to_owned(),
            name_key: "account.inline.ar".to_owned(),
            account_type: ledgers_ledger::account::AccountType::Asset,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: vec!["ar".to_owned()],
            display_name: None,
        };
        commands::create_account(db, COMPANY_ID, ar_account, NOW.to_owned(), &person())
            .await
            .unwrap();

        // Prepare entry: bank debit, ar account credit
        let ed = entry_draft(
            "jn-01",
            "Test ar posting",
            vec![
                Line {
                    account_number: "1010".to_owned(),
                    side: ledgers_ledger::Side::Debit,
                    base: Amount::new(5_000, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
                Line {
                    account_number: "1200".to_owned(),
                    side: ledgers_ledger::Side::Credit,
                    base: Amount::new(5_000, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
            ],
        );

        let entry = Entry::from_draft(ed, COMPANY_ID).unwrap();
        let result = post_test_entry(db, &entry, "entry_ar_01", "outbox_ar_01").await;

        // Should be rejected with ControlAccountPosting error
        assert!(matches!(
            result,
            Err(JournalError::ControlAccountPosting { accounts, .. })
            if accounts.contains("1200")
        ));

        // Verify no entry was written
        let entry_count = count(db, "journal_entry", "id", "entry_ar_01").await;
        assert_eq!(
            entry_count, 0,
            "Entry should not be persisted when rejected"
        );

        // Verify no lines were written
        let lines_count = count(db, "journal_line", "entry_id", "entry_ar_01").await;
        assert_eq!(
            lines_count, 0,
            "Lines should not be persisted when rejected"
        );
    });
}

#[test]
fn manual_post_to_ap_account_is_rejected() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();

        mount(db, COMPANY_ID).await;
        create_test_journal(db, COMPANY_ID, "jn-02").await;
        open_period(db, COMPANY_ID, "2026-02").await;

        // Create an ap-tagged account
        let ap_account = AccountDraft {
            number: "2100".to_owned(),
            name_key: "account.inline.ap".to_owned(),
            account_type: ledgers_ledger::account::AccountType::Liability,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: vec!["ap".to_owned()],
            display_name: None,
        };
        commands::create_account(db, COMPANY_ID, ap_account, NOW.to_owned(), &person())
            .await
            .unwrap();

        // Prepare entry: ap account debit, bank account credit
        let ed = entry_draft(
            "jn-02",
            "Test ap posting",
            vec![
                Line {
                    account_number: "2100".to_owned(),
                    side: ledgers_ledger::Side::Debit,
                    base: Amount::new(3_000, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
                Line {
                    account_number: "1010".to_owned(),
                    side: ledgers_ledger::Side::Credit,
                    base: Amount::new(3_000, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
            ],
        );

        let entry = Entry::from_draft(ed, COMPANY_ID).unwrap();
        let result = post_test_entry(db, &entry, "entry_ap_01", "outbox_ap_01").await;

        // Should be rejected with ControlAccountPosting error
        assert!(matches!(
            result,
            Err(JournalError::ControlAccountPosting { accounts, .. })
            if accounts.contains("2100")
        ));

        // Verify no entry was written
        let entry_count = count(db, "journal_entry", "id", "entry_ap_01").await;
        assert_eq!(
            entry_count, 0,
            "Entry should not be persisted when rejected"
        );

        // Verify no lines were written
        let lines_count = count(db, "journal_line", "entry_id", "entry_ap_01").await;
        assert_eq!(
            lines_count, 0,
            "Lines should not be persisted when rejected"
        );
    });
}

#[test]
fn manual_post_to_other_tagged_accounts_succeeds() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();

        mount(db, COMPANY_ID).await;
        create_test_journal(db, COMPANY_ID, "jn-03").await;
        open_period(db, COMPANY_ID, "2026-02").await;

        // Create accounts with other tags (retained-earnings, vat-input, bank)
        let re_account = AccountDraft {
            number: "3100".to_owned(),
            name_key: "account.inline.retained".to_owned(),
            account_type: ledgers_ledger::account::AccountType::Equity,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: vec!["retained-earnings".to_owned()],
            display_name: None,
        };
        commands::create_account(db, COMPANY_ID, re_account, NOW.to_owned(), &person())
            .await
            .unwrap();

        let vat_account = AccountDraft {
            number: "1750".to_owned(),
            name_key: "account.inline.vat-input".to_owned(),
            account_type: ledgers_ledger::account::AccountType::Asset,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: vec!["vat-input".to_owned()],
            display_name: None,
        };
        commands::create_account(db, COMPANY_ID, vat_account, NOW.to_owned(), &person())
            .await
            .unwrap();

        // Create entry: bank to retained-earnings
        let ed1 = entry_draft(
            "jn-03",
            "RE posting",
            vec![
                Line {
                    account_number: "1010".to_owned(),
                    side: ledgers_ledger::Side::Debit,
                    base: Amount::new(2_000, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
                Line {
                    account_number: "3100".to_owned(),
                    side: ledgers_ledger::Side::Credit,
                    base: Amount::new(2_000, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
            ],
        );

        let entry_1 = Entry::from_draft(ed1, COMPANY_ID).unwrap();
        let result_1 = post_test_entry(db, &entry_1, "entry_re_01", "outbox_re_01").await;
        assert!(
            result_1.is_ok(),
            "Posting to retained-earnings should succeed"
        );
        let entry_count = count(db, "journal_entry", "id", "entry_re_01").await;
        assert_eq!(entry_count, 1, "Entry should be persisted");

        // Create entry: bank to vat-input
        let ed2 = entry_draft(
            "jn-03",
            "VAT posting",
            vec![
                Line {
                    account_number: "1010".to_owned(),
                    side: ledgers_ledger::Side::Debit,
                    base: Amount::new(1_500, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
                Line {
                    account_number: "1750".to_owned(),
                    side: ledgers_ledger::Side::Credit,
                    base: Amount::new(1_500, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
            ],
        );

        let entry_2 = Entry::from_draft(ed2, COMPANY_ID).unwrap();
        let result_2 = post_test_entry(db, &entry_2, "entry_vat_01", "outbox_vat_01").await;
        assert!(result_2.is_ok(), "Posting to vat-input should succeed");
        let entry_count = count(db, "journal_entry", "id", "entry_vat_01").await;
        assert_eq!(entry_count, 1, "Entry should be persisted");
    });
}

#[test]
fn privileged_post_to_ar_account_succeeds() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();

        mount(db, COMPANY_ID).await;
        create_test_journal(db, COMPANY_ID, "jn-04").await;
        open_period(db, COMPANY_ID, "2026-02").await;

        // Create an ar-tagged account
        let ar_account = AccountDraft {
            number: "1201".to_owned(),
            name_key: "account.inline.ar2".to_owned(),
            account_type: ledgers_ledger::account::AccountType::Asset,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: vec!["ar".to_owned()],
            display_name: None,
        };
        commands::create_account(db, COMPANY_ID, ar_account, NOW.to_owned(), &person())
            .await
            .unwrap();

        // Prepare entry: bank debit, ar account credit
        let ed = entry_draft(
            "jn-04",
            "Privileged ar posting",
            vec![
                Line {
                    account_number: "1010".to_owned(),
                    side: ledgers_ledger::Side::Debit,
                    base: Amount::new(7_000, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
                Line {
                    account_number: "1201".to_owned(),
                    side: ledgers_ledger::Side::Credit,
                    base: Amount::new(7_000, Currency::from_code("EUR").unwrap()),
                    fx: None,
                    tax_code: None,
                    dimensions: Default::default(),
                    memo: None,
                },
            ],
        );

        let entry = Entry::from_draft(ed, COMPANY_ID).unwrap();

        // Use the privileged posting path (post_entry_with_control)
        let result =
            post_test_entry_with_control(db, &entry, "entry_priv_ar_01", "outbox_priv_ar_01").await;

        assert!(
            result.is_ok(),
            "Privileged posting to ar account should succeed"
        );

        // Verify entry was posted
        let entry_count = count(db, "journal_entry", "id", "entry_priv_ar_01").await;
        assert_eq!(entry_count, 1, "Entry should be persisted");
    });
}
