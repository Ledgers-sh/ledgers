//! Integration tests for subledger reconciliation.

mod common;

use common::{COMPANY_ID, NOW, create_journal, harness, mount};
use cratefield_core::Statement;
use ledgers_commands::{
    CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
    auth::{Authenticator, Credential, Principal, Role},
};
use ledgers_ledger::close::{CloseCheck, CloseCheckContext};
use ledgers_ledger::error::AccountsError;
use ledgers_ledger::journal_port::{AccountBalance, BoxFuture, Journal, NewEntry, PostedEntry};
use ledgers_money::Currency;
use ledgers_subledger::close_check::SubledgerReconciliationCheck;
use ledgers_subledger::commands::{AllocateInput, AllocateItem, PostItemInput};
use ledgers_subledger::commands::{SubledgerAllocate, SubledgerPostItem};
use ledgers_subledger::reconcile::reconcile;
use sea_query::Value;

struct TestAuthenticator;

#[async_trait::async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(&self, _credential: &Credential) -> Result<Principal, CommandError> {
        Ok(Principal::Person {
            person_id: "test_person".to_string(),
            role: Role::Owner,
        })
    }
}

/// Helper to post an AR invoice.
#[allow(clippy::too_many_arguments)]
async fn post_invoice(
    db: &dyn cratefield_core::Database,
    journal_id: &str,
    party_id: &str,
    amount: i64,
    currency: &str,
    rate: Option<&str>,
    date: &str,
    event_id: &str,
) -> String {
    let input = PostItemInput {
        ledger: "ar".to_owned(),
        kind: "invoice".to_owned(),
        party_id: party_id.to_owned(),
        journal_id: journal_id.to_owned(),
        document_date: date.to_owned(),
        posting_date: date.to_owned(),
        due_date: None,
        currency: currency.to_owned(),
        amount,
        rate: rate.map(std::borrow::ToOwned::to_owned),
        counter_account: "4000".to_owned(),
        dimensions: Default::default(),
        counter_lines: Vec::new(),
        description: "Test invoice".to_owned(),
        source_id: Some(format!("INV-{event_id}")),
    };

    let ctx = CommandContext {
        company_id: COMPANY_ID.to_owned(),
        is_person: true,
        event_id: event_id.to_owned(),
        request_id: "req_001".to_owned(),
        now: NOW.to_owned(),
    };

    let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
    let result = executor
        .run(
            &SubledgerPostItem,
            "bearer".to_string(),
            ctx,
            &input,
            Execute::Run,
        )
        .await
        .expect("post invoice should succeed");

    match result {
        Executed::Done(output) => output.item_id,
        _ => panic!("expected Done"),
    }
}

/// Helper to post an AP bill.
#[allow(clippy::too_many_arguments)]
async fn post_bill(
    db: &dyn cratefield_core::Database,
    journal_id: &str,
    party_id: &str,
    amount: i64,
    currency: &str,
    rate: Option<&str>,
    date: &str,
    event_id: &str,
) -> String {
    let input = PostItemInput {
        ledger: "ap".to_owned(),
        kind: "bill".to_owned(),
        party_id: party_id.to_owned(),
        journal_id: journal_id.to_owned(),
        document_date: date.to_owned(),
        posting_date: date.to_owned(),
        due_date: None,
        currency: currency.to_owned(),
        amount,
        rate: rate.map(std::borrow::ToOwned::to_owned),
        counter_account: "6000".to_owned(),
        dimensions: Default::default(),
        counter_lines: Vec::new(),
        description: "Test bill".to_owned(),
        source_id: Some(format!("BILL-{event_id}")),
    };

    let ctx = CommandContext {
        company_id: COMPANY_ID.to_owned(),
        is_person: true,
        event_id: event_id.to_owned(),
        request_id: "req_002".to_owned(),
        now: NOW.to_owned(),
    };

    let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
    let result = executor
        .run(
            &SubledgerPostItem,
            "bearer".to_string(),
            ctx,
            &input,
            Execute::Run,
        )
        .await
        .expect("post bill should succeed");

    match result {
        Executed::Done(output) => output.item_id,
        _ => panic!("expected Done"),
    }
}

/// Helper to allocate a payment.
#[allow(clippy::too_many_arguments)]
async fn allocate_payment(
    db: &dyn cratefield_core::Database,
    journal_id: &str,
    party_id: &str,
    ledger: &str,
    items: Vec<(String, i64, Option<i64>)>, // (item_id, amount, paid_base)
    total_paid: i64,
    date: &str,
    event_id: &str,
) {
    let allocate_items: Vec<AllocateItem> = items
        .iter()
        .map(|(item_id, amount, paid_base)| AllocateItem {
            open_item_id: item_id.clone(),
            amount: *amount,
            paid_base: *paid_base,
        })
        .collect();

    let input = AllocateInput {
        ledger: ledger.to_owned(),
        party_id: party_id.to_owned(),
        journal_id: journal_id.to_owned(),
        date: date.to_owned(),
        bank_account: "1200".to_owned(),
        paid: total_paid,
        items: allocate_items,
        description: Some("Test payment".to_owned()),
        reference: None,
    };

    let ctx = CommandContext {
        company_id: COMPANY_ID.to_owned(),
        is_person: true,
        event_id: event_id.to_owned(),
        request_id: "req_alloc".to_owned(),
        now: NOW.to_owned(),
    };

    let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
    let _result = executor
        .run(
            &SubledgerAllocate,
            "bearer".to_string(),
            ctx,
            &input,
            Execute::Run,
        )
        .await
        .expect("allocate should succeed");
}

#[test]
fn test_reconcile_empty_company_has_zero_balance() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;

        let result = reconcile(db, COMPANY_ID, "2026-10-31")
            .await
            .expect("reconcile empty should succeed");

        assert_eq!(result.len(), 2);
        for ledger in result {
            assert_eq!(ledger.subledger_balance, 0);
            assert_eq!(ledger.gl_balance, 0);
            assert_eq!(ledger.difference, 0);
        }
    });
}

#[test]
fn test_reconcile_single_ar_invoice_matches() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post AR invoice for 1000 EUR
        post_invoice(
            db,
            &journal_id,
            "cust1",
            10_0000,
            "EUR",
            None,
            "2026-10-15",
            "inv1",
        )
        .await;

        let result = reconcile(db, COMPANY_ID, "2026-10-31")
            .await
            .expect("reconcile should succeed");

        let ar = result.iter().find(|l| l.ledger == "ar").expect("AR found");
        assert_eq!(ar.subledger_balance, 10_0000);
        assert_eq!(ar.gl_balance, 10_0000);
        assert_eq!(ar.difference, 0);
    });
}

#[test]
fn test_reconcile_ar_with_partial_allocation() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post AR invoice for 1000 EUR
        let item_id = post_invoice(
            db,
            &journal_id,
            "cust1",
            10_0000,
            "EUR",
            None,
            "2026-10-15",
            "inv1",
        )
        .await;

        // Allocate 300 EUR
        allocate_payment(
            db,
            &journal_id,
            "cust1",
            "ar",
            vec![(item_id, 30_000, None)],
            30_000,
            "2026-10-20",
            "pay1",
        )
        .await;

        let result = reconcile(db, COMPANY_ID, "2026-10-31")
            .await
            .expect("reconcile should succeed");

        let ar = result.iter().find(|l| l.ledger == "ar").expect("AR found");
        // Subledger: 1000 - 300 = 700
        assert_eq!(ar.subledger_balance, 70_000);
        // GL: 1000 (invoice) - 300 (payment cr) = 700
        assert_eq!(ar.gl_balance, 70_000);
        assert_eq!(ar.difference, 0);
    });
}

#[test]
fn test_reconcile_ap_single_bill_matches() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post AP bill for 1000 EUR
        post_bill(
            db,
            &journal_id,
            "vendor1",
            10_0000,
            "EUR",
            None,
            "2026-10-15",
            "bill1",
        )
        .await;

        let result = reconcile(db, COMPANY_ID, "2026-10-31")
            .await
            .expect("reconcile should succeed");

        let ap = result.iter().find(|l| l.ledger == "ap").expect("AP found");
        assert_eq!(ap.subledger_balance, 10_0000);
        assert_eq!(ap.gl_balance, 10_0000);
        assert_eq!(ap.difference, 0);
    });
}

#[test]
fn test_reconcile_ap_full_allocation() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post AP bill for 500 EUR
        let bill_id = post_bill(
            db,
            &journal_id,
            "vendor1",
            50_000,
            "EUR",
            None,
            "2026-10-15",
            "bill1",
        )
        .await;

        // Fully allocate 500 EUR
        allocate_payment(
            db,
            &journal_id,
            "vendor1",
            "ap",
            vec![(bill_id, 50_000, None)],
            50_000,
            "2026-10-20",
            "pay1",
        )
        .await;

        let result = reconcile(db, COMPANY_ID, "2026-10-31")
            .await
            .expect("reconcile should succeed");

        let ap = result.iter().find(|l| l.ledger == "ap").expect("AP found");
        assert_eq!(
            ap.subledger_balance, 0,
            "fully allocated bill should have zero balance"
        );
        assert_eq!(ap.difference, 0, "full allocation should be balanced");
    });
}

#[test]
fn test_reconcile_as_of_date_filters_correctly() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post AR invoice on 2026-10-15
        let _item1 = post_invoice(
            db,
            &journal_id,
            "cust1",
            10_0000,
            "EUR",
            None,
            "2026-10-15",
            "inv1",
        )
        .await;

        // Post AR invoice on 2026-10-25
        let _item2 = post_invoice(
            db,
            &journal_id,
            "cust1",
            50_000,
            "EUR",
            None,
            "2026-10-25",
            "inv2",
        )
        .await;

        // Reconcile as of 2026-10-20: should only see first invoice
        let result_early = reconcile(db, COMPANY_ID, "2026-10-20")
            .await
            .expect("reconcile early should succeed");
        let ar_early = result_early
            .iter()
            .find(|l| l.ledger == "ar")
            .expect("AR found");
        assert_eq!(ar_early.subledger_balance, 10_0000);

        // Reconcile as of 2026-10-31: should see both invoices
        let result_late = reconcile(db, COMPANY_ID, "2026-10-31")
            .await
            .expect("reconcile late should succeed");
        let ar_late = result_late
            .iter()
            .find(|l| l.ledger == "ar")
            .expect("AR found");
        assert_eq!(ar_late.subledger_balance, 150_000);
    });
}

/// Fixed scenario test: reconciliation stays balanced with different posting patterns.
/// (This is a fixed test; the real property test is in tests/property.rs)
#[test]
fn test_reconcile_stays_balanced_fixed_scenario() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Test scenario 1: Single invoice and bill with unique parties
        post_invoice(
            db,
            &journal_id,
            "cust1",
            10_0000,
            "EUR",
            None,
            "2026-10-05",
            "inv-s1",
        )
        .await;

        post_bill(
            db,
            &journal_id,
            "vendor1",
            50_000,
            "EUR",
            None,
            "2026-10-06",
            "bill-s1",
        )
        .await;

        let result = reconcile(db, COMPANY_ID, "2026-10-31")
            .await
            .expect("reconcile scenario 1");

        for ledger in &result {
            assert_eq!(
                ledger.difference, 0,
                "Scenario 1: reconciliation difference must be 0 for {} at month end",
                ledger.ledger
            );
        }
    });
}

/// Test journal for close-check testing.
struct TestJournal;

impl Journal for TestJournal {
    fn base_currency<'a>(
        &'a self,
        _db: &'a dyn cratefield_core::Database,
        _company_id: &'a str,
    ) -> BoxFuture<'a, Result<Currency, AccountsError>> {
        Box::pin(async { Ok(Currency::from_code("EUR").unwrap()) })
    }

    fn balances<'a>(
        &'a self,
        _db: &'a dyn cratefield_core::Database,
        _company_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<AccountBalance>, AccountsError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn draft_count<'a>(
        &'a self,
        _db: &'a dyn cratefield_core::Database,
        _company_id: &'a str,
        _from: &'a str,
        _to: &'a str,
    ) -> BoxFuture<'a, Result<i64, AccountsError>> {
        Box::pin(async { Ok(0) })
    }

    fn post<'a>(
        &'a self,
        _db: &'a dyn cratefield_core::Database,
        _entry: &'a NewEntry,
        _audit: &'a ledgers_audit::AuditContext,
    ) -> BoxFuture<'a, Result<PostedEntry, AccountsError>> {
        Box::pin(async { unreachable!() })
    }

    fn find_entry<'a>(
        &'a self,
        _db: &'a dyn cratefield_core::Database,
        _company_id: &'a str,
        _entry_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<PostedEntry>, AccountsError>> {
        Box::pin(async { Ok(None) })
    }

    fn ensure_journal<'a>(
        &'a self,
        _db: &'a dyn cratefield_core::Database,
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

#[test]
fn subledger_reconciliation_check_balanced_company_has_no_blockers() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post matching AR and AP entries
        post_invoice(
            db,
            &journal_id,
            "cust1",
            10_0000,
            "EUR",
            None,
            "2026-10-15",
            "inv1",
        )
        .await;

        post_bill(
            db,
            &journal_id,
            "vendor1",
            10_0000,
            "EUR",
            None,
            "2026-10-15",
            "bill1",
        )
        .await;

        let check = SubledgerReconciliationCheck;
        let journal = TestJournal;

        let ctx = CloseCheckContext {
            db,
            journal: &journal,
            company_id: COMPANY_ID,
            period: "2026-10",
            starts_on: "2026-10-01",
            ends_on: "2026-10-31",
        };

        let blockers = check.blockers(ctx).await.expect("blockers query");
        assert_eq!(
            blockers.len(),
            0,
            "Balanced company should have no reconciliation blockers"
        );
    });
}

#[test]
fn subledger_reconciliation_check_detects_drift() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post AR invoice for 1000 EUR
        post_invoice(
            db,
            &journal_id,
            "cust1",
            10_0000,
            "EUR",
            None,
            "2026-10-15",
            "inv1",
        )
        .await;

        // Create drift by overwriting the posted amount the subledger balance is built
        // from (base_original_minor): the item now says 300 where the GL posted 1000.
        let drift_sql = "UPDATE open_item SET base_original_minor = 30_00 \
                        WHERE company_id = ? AND ledger = ?";
        db.execute(&Statement::with_values(
            drift_sql.to_owned(),
            vec![Value::from(COMPANY_ID), Value::from("ar")],
        ))
        .await
        .expect("drift update should succeed");

        let check = SubledgerReconciliationCheck;
        let journal = TestJournal;

        let ctx = CloseCheckContext {
            db,
            journal: &journal,
            company_id: COMPANY_ID,
            period: "2026-10",
            starts_on: "2026-10-01",
            ends_on: "2026-10-31",
        };

        let blockers = check.blockers(ctx).await.expect("blockers query");
        // The drift check should detect the mismatch between subledger (300) and GL (1000)
        assert!(
            !blockers.is_empty(),
            "Drift between subledger and GL should produce blockers"
        );
        // Verify blocker properties
        let ar_blocker = blockers
            .iter()
            .find(|b| b.kind == "subledger-mismatch" && b.link.contains("ar"));
        assert!(
            ar_blocker.is_some(),
            "Should find subledger-mismatch blocker for AR"
        );
    });
}
