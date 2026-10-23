//! E2E test for `FlaggedExtractionCloseCheck::blockers`.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_bills::BillsModule;
use ledgers_bills::close::FlaggedExtractionCloseCheck;
use ledgers_db::DbModule;
use ledgers_ledger::close::{CloseCheck, CloseCheckContext};
use ledgers_ledger::error::AccountsError;
use ledgers_ledger::journal_port::{AccountBalance, BoxFuture, Journal, NewEntry, PostedEntry};
use ledgers_money::Currency;
use sea_query::Value;

const COMPANY: &str = "01TESTCOMPANY0000000000000A";
const NOW: &str = "2026-03-15T09:30:00Z";

struct TestJournal;

impl Journal for TestJournal {
    fn base_currency<'a>(
        &'a self,
        _db: &'a dyn Database,
        _company_id: &'a str,
    ) -> BoxFuture<'a, Result<Currency, AccountsError>> {
        Box::pin(async { Ok(Currency::from_code("EUR").unwrap()) })
    }

    fn balances<'a>(
        &'a self,
        _db: &'a dyn Database,
        _company_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<AccountBalance>, AccountsError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn draft_count<'a>(
        &'a self,
        _db: &'a dyn Database,
        _company_id: &'a str,
        _from: &'a str,
        _to: &'a str,
    ) -> BoxFuture<'a, Result<i64, AccountsError>> {
        Box::pin(async { Ok(0) })
    }

    fn post<'a>(
        &'a self,
        _db: &'a dyn Database,
        _entry: &'a NewEntry,
        _audit: &'a ledgers_audit::AuditContext,
    ) -> BoxFuture<'a, Result<PostedEntry, AccountsError>> {
        Box::pin(async { unreachable!() })
    }

    fn find_entry<'a>(
        &'a self,
        _db: &'a dyn Database,
        _company_id: &'a str,
        _entry_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<PostedEntry>, AccountsError>> {
        Box::pin(async { Ok(None) })
    }

    fn ensure_journal<'a>(
        &'a self,
        _db: &'a dyn Database,
        _company_id: &'a str,
        _journal_id: &'a str,
        _code: &'a str,
        _name: &'a str,
        _created_at: &'a str,
        _audit: &'a ledgers_audit::AuditContext,
    ) -> BoxFuture<'a, Result<(), AccountsError>> {
        Box::pin(async { Ok(()) })
    }
}

async fn exec(db: &dyn Database, sql: &str, vals: Vec<Value>) {
    db.execute(&Statement::with_values(sql.to_string(), vals))
        .await
        .unwrap_or_else(|e| panic!("exec: {e}"));
}

struct BillData {
    id: String,
    invoice_date: Option<String>,
    checks_json: String,
    confidence_json: String,
    instruction_flags: String,
    posted_entry_id: Option<String>,
    discarded_at: Option<String>,
}

async fn seed_bill(db: &dyn Database, bill: &BillData) {
    let date_val = bill
        .invoice_date
        .as_ref()
        .map_or_else(|| "NULL".to_string(), |d| format!("'{d}'"));
    let posted_val = bill
        .posted_entry_id
        .as_ref()
        .map_or_else(|| "NULL".to_string(), |p| format!("'{p}'"));
    let discard_val = bill
        .discarded_at
        .as_ref()
        .map_or_else(|| "NULL".to_string(), |d| format!("'{d}'"));

    let sql = format!(
        "INSERT INTO bill (id, company_id, document_id, vendor_id, invoice_number, invoice_date, \
         currency, lines_json, confidence_json, checks_json, instruction_flags, iban_changed, \
         source, posted_entry_id, discarded_at, created_by, created_at, updated_at) \
         VALUES ('{id}', '{company}', 'doc-{id}', 'v1', 'inv-{id}', {date_val}, 'EUR', '[]', '{conf}', '{checks}', '{flags}', 0, \
         'manual', {posted}, {discard}, 'test', '{now}', '{now}')",
        id = bill.id,
        company = COMPANY,
        date_val = date_val,
        conf = bill.confidence_json,
        checks = bill.checks_json,
        flags = bill.instruction_flags,
        posted = posted_val,
        discard = discard_val,
        now = NOW
    );
    db.execute(&Statement::new(sql)).await.expect("insert");
}

async fn seed_test_bills(db: &dyn Database) {
    // A: flagged (failed check), unreviewed, in period → COUNT
    seed_bill(
        db,
        &BillData {
            id: "bill-a".into(),
            invoice_date: Some("2026-03-15".into()),
            checks_json: r#"[{"id":"check-1","ok":false,"detail":"test"}]"#.into(),
            confidence_json: "{}".into(),
            instruction_flags: "[]".into(),
            posted_entry_id: None,
            discarded_at: None,
        },
    )
    .await;

    // B: low confidence, unreviewed, in period → COUNT
    seed_bill(
        db,
        &BillData {
            id: "bill-b".into(),
            invoice_date: Some("2026-03-16".into()),
            checks_json: "[]".into(),
            confidence_json: r#"{"vendor_name": 45}"#.into(),
            instruction_flags: "[]".into(),
            posted_entry_id: None,
            discarded_at: None,
        },
    )
    .await;

    // C: clean, in period → no count
    seed_bill(
        db,
        &BillData {
            id: "bill-c".into(),
            invoice_date: Some("2026-03-17".into()),
            checks_json: "[]".into(),
            confidence_json: "{}".into(),
            instruction_flags: "[]".into(),
            posted_entry_id: None,
            discarded_at: None,
        },
    )
    .await;

    // D: flagged, outside period → no count
    seed_bill(
        db,
        &BillData {
            id: "bill-d".into(),
            invoice_date: Some("2026-02-01".into()),
            checks_json: r#"[{"id":"check-2","ok":false,"detail":"test"}]"#.into(),
            confidence_json: "{}".into(),
            instruction_flags: "[]".into(),
            posted_entry_id: None,
            discarded_at: None,
        },
    )
    .await;

    // E: flagged but posted → no count
    seed_bill(
        db,
        &BillData {
            id: "bill-e".into(),
            invoice_date: Some("2026-03-18".into()),
            checks_json: r#"[{"id":"check-3","ok":false,"detail":"test"}]"#.into(),
            confidence_json: "{}".into(),
            instruction_flags: "[]".into(),
            posted_entry_id: Some("entry-posted".into()),
            discarded_at: None,
        },
    )
    .await;

    // F: instruction flags, no invoice_date (uses created_at), in period → COUNT
    seed_bill(
        db,
        &BillData {
            id: "bill-f".into(),
            invoice_date: None,
            checks_json: "[]".into(),
            confidence_json: "{}".into(),
            instruction_flags: r#"["review_needed"]"#.into(),
            posted_entry_id: None,
            discarded_at: None,
        },
    )
    .await;

    // G: flagged but discarded → no count
    seed_bill(
        db,
        &BillData {
            id: "bill-g".into(),
            invoice_date: Some("2026-03-19".into()),
            checks_json: r#"[{"id":"check-4","ok":false,"detail":"test"}]"#.into(),
            confidence_json: "{}".into(),
            instruction_flags: "[]".into(),
            posted_entry_id: None,
            discarded_at: Some("2026-03-19T10:00:00Z".into()),
        },
    )
    .await;
}

async fn run_test_scenario(db: &dyn Database) {
    // Create vendor.
    exec(
        db,
        "INSERT INTO vendor (id, company_id, name, source, needs_review, created_at, updated_at) \
         VALUES (?, ?, ?, 'manual', 0, ?, ?)",
        vec![
            Value::from("v1"),
            Value::from(COMPANY),
            Value::from("Test"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;

    // Seed test bills (7 cases: A-G).
    seed_test_bills(db).await;
}

#[test]
fn flagged_extraction_close_check_blockers() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule), Box::new(BillsModule)]) {
        pollster::block_on(async {
            run_test_scenario(&*kit.db).await;

            let check = FlaggedExtractionCloseCheck;
            let journal = TestJournal;

            // Test period 2026-03: expect 3 flagged bills (A, B, F).
            let ctx = CloseCheckContext {
                db: &*kit.db,
                journal: &journal,
                company_id: COMPANY,
                period: "2026-03",
                starts_on: "2026-03-01",
                ends_on: "2026-03-31",
            };

            let blockers = check.blockers(ctx).await.expect("blockers query");
            assert_eq!(blockers.len(), 1, "Expected one blocker type");
            assert_eq!(blockers[0].check_id, "bills.flagged-extraction");
            assert_eq!(blockers[0].kind, "flagged-extraction");
            assert_eq!(blockers[0].count, 3, "Expected 3 flagged bills (A, B, F)");

            // Empty period: no blockers.
            let empty_ctx = CloseCheckContext {
                db: &*kit.db,
                journal: &journal,
                company_id: COMPANY,
                period: "2026-01",
                starts_on: "2026-01-01",
                ends_on: "2026-01-31",
            };
            let empty = check.blockers(empty_ctx).await.expect("empty");
            assert_eq!(empty.len(), 0);

            // Different company: no blockers.
            let other_ctx = CloseCheckContext {
                db: &*kit.db,
                journal: &journal,
                company_id: "01OTHERCOMPANY0000000000000B",
                period: "2026-03",
                starts_on: "2026-03-01",
                ends_on: "2026-03-31",
            };
            let other = check.blockers(other_ctx).await.expect("other");
            assert_eq!(other.len(), 0);
        });
    }
}
