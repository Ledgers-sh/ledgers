//! Integration tests for subledger.allocate command.

mod common;

use common::{COMPANY_ID, NOW, create_journal, harness, mount, open_item_row};
use ledgers_commands::{
    CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
    auth::{Authenticator, Credential, Principal, Role},
};
use ledgers_subledger::commands::{AllocateInput, AllocateItem, SubledgerAllocate};

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
async fn post_invoice(
    db: &dyn cratefield_core::Database,
    journal_id: &str,
    party_id: &str,
    amount: i64,
    currency: &str,
    rate: Option<&str>,
    event_id: &str,
) -> String {
    use ledgers_subledger::commands::PostItemInput;

    let input = PostItemInput {
        ledger: "ar".to_owned(),
        kind: "invoice".to_owned(),
        party_id: party_id.to_owned(),
        journal_id: journal_id.to_owned(),
        document_date: "2026-10-01".to_owned(),
        posting_date: "2026-10-01".to_owned(),
        due_date: None,
        currency: currency.to_owned(),
        amount,
        rate: rate.map(std::borrow::ToOwned::to_owned),
        counter_account: "4000".to_owned(),
        dimensions: Default::default(),
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
            &ledgers_subledger::commands::SubledgerPostItem,
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
async fn post_bill(
    db: &dyn cratefield_core::Database,
    journal_id: &str,
    party_id: &str,
    amount: i64,
    currency: &str,
    rate: Option<&str>,
    event_id: &str,
) -> String {
    use ledgers_subledger::commands::PostItemInput;

    let input = PostItemInput {
        ledger: "ap".to_owned(),
        kind: "bill".to_owned(),
        party_id: party_id.to_owned(),
        journal_id: journal_id.to_owned(),
        document_date: "2026-10-01".to_owned(),
        posting_date: "2026-10-01".to_owned(),
        due_date: None,
        currency: currency.to_owned(),
        amount,
        rate: rate.map(std::borrow::ToOwned::to_owned),
        counter_account: "6000".to_owned(),
        dimensions: Default::default(),
        description: "Test bill".to_owned(),
        source_id: Some(format!("BILL-{event_id}")),
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
            &ledgers_subledger::commands::SubledgerPostItem,
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
async fn allocate_payment(
    db: &dyn cratefield_core::Database,
    input: AllocateInput,
    event_id: &str,
) -> (String, Vec<(String, i64, i64)>, Option<String>, i64) {
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
            &SubledgerAllocate,
            "bearer".to_string(),
            ctx,
            &input,
            Execute::Run,
        )
        .await
        .expect("allocate should succeed");

    match result {
        Executed::Done(output) => {
            let items_remaining: Vec<_> = output
                .items_remaining
                .iter()
                .map(|ir| (ir.open_item_id.clone(), ir.tx_remaining, ir.base_remaining))
                .collect();
            (
                output.entry_id,
                items_remaining,
                output.on_account_item_id,
                output.fx_difference_total,
            )
        }
        _ => panic!("expected Done"),
    }
}

#[test]
fn test_usd_invoice_receipt_with_fx_gain() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post USD 1000.00 invoice at rate 0.9 (base 900.00 EUR)
        let item_id = post_invoice(
            db,
            &journal_id,
            "cust001",
            100_000,
            "USD",
            Some("0.9"),
            "inv_usd",
        )
        .await;

        // Receipt: paid 920.00 EUR for 100000 USD (amount = 100000, paid_base = 92000)
        // Calculation: 100000 USD at 0.9 = 90000 base. Paid 92000. FX gain = 92000 - 90000 = 2000.
        let input = AllocateInput {
            ledger: "ar".to_owned(),
            party_id: "cust001".to_owned(),
            journal_id,
            date: "2026-10-02".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 92_000,
            items: vec![AllocateItem {
                open_item_id: item_id.clone(),
                amount: 100_000,
                paid_base: Some(92_000),
            }],
            description: None,
            reference: None,
        };

        let (_entry_id, items_remaining, oa_item, fx_total) =
            allocate_payment(db, input, "alloc_usd_gain").await;

        // Item should be fully allocated
        assert_eq!(items_remaining.len(), 1);
        let (rid, tx_rem, base_rem) = &items_remaining[0];
        assert_eq!(rid, &item_id);
        assert_eq!(*tx_rem, 0, "tx_remaining should be 0 after full allocation");
        assert_eq!(
            *base_rem, 0,
            "base_remaining should be 0 after full allocation"
        );

        // No on-account item (exact payment)
        assert!(oa_item.is_none(), "no on-account item for exact payment");

        // FX gain: 92000 - 90000 = 2000
        assert_eq!(fx_total, 2_000, "FX gain should be 2000");

        // Verify item row: tx_remaining and base_remaining should be 0
        let item = open_item_row(db, &item_id)
            .await
            .expect("item should exist");
        assert_eq!(item.tx_remaining_minor, 0);
        assert_eq!(item.base_remaining_minor, 0);
    });
}

#[test]
fn test_usd_invoice_receipt_with_fx_loss() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post USD 1000.00 invoice at rate 0.9 (base 900.00 EUR)
        let item_id = post_invoice(
            db,
            &journal_id,
            "cust002",
            100_000,
            "USD",
            Some("0.9"),
            "inv_usd2",
        )
        .await;

        // Receipt: paid 880.00 EUR for 100000 USD (less than base 900.00)
        // FX loss = base_cleared - paid_base = 90000 - 88000 = 2000 (loss, so negative)
        let input = AllocateInput {
            ledger: "ar".to_owned(),
            party_id: "cust002".to_owned(),
            journal_id,
            date: "2026-10-02".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 88_000,
            items: vec![AllocateItem {
                open_item_id: item_id.clone(),
                amount: 100_000,
                paid_base: Some(88_000),
            }],
            description: None,
            reference: None,
        };

        let (_entry_id, items_remaining, oa_item, fx_total) =
            allocate_payment(db, input, "alloc_usd_loss").await;

        // Item should be fully allocated
        assert_eq!(items_remaining.len(), 1);
        let (rid, tx_rem, base_rem) = &items_remaining[0];
        assert_eq!(rid, &item_id);
        assert_eq!(*tx_rem, 0);
        assert_eq!(*base_rem, 0);

        // No on-account item
        assert!(oa_item.is_none());

        // FX loss (AR): paid_base - base_cleared = 88000 - 90000 = -2000
        assert_eq!(fx_total, -2_000, "FX loss should be -2000");
    });
}

#[test]
fn test_partial_eur_payment_then_final() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post 1000.00 EUR invoice
        let item_id =
            post_invoice(db, &journal_id, "cust003", 100_000, "EUR", None, "inv_eur").await;

        // First payment: 600.00 EUR
        let input1 = AllocateInput {
            ledger: "ar".to_owned(),
            party_id: "cust003".to_owned(),
            journal_id: journal_id.clone(),
            date: "2026-10-02".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 60_000,
            items: vec![AllocateItem {
                open_item_id: item_id.clone(),
                amount: 60_000,
                paid_base: None,
            }],
            description: None,
            reference: None,
        };

        let (_entry_id, items_remaining, _oa_item, _fx_total) =
            allocate_payment(db, input1, "alloc_eur_p1").await;

        // Item should have 400.00 EUR remaining
        assert_eq!(items_remaining.len(), 1);
        let (rid, tx_rem, base_rem) = &items_remaining[0];
        assert_eq!(rid, &item_id);
        assert_eq!(*tx_rem, 40_000, "tx_remaining should be 400.00 EUR");
        assert_eq!(*base_rem, 40_000, "base_remaining should be 400.00 EUR");

        // Second payment: remaining 400.00 EUR
        let input2 = AllocateInput {
            ledger: "ar".to_owned(),
            party_id: "cust003".to_owned(),
            journal_id,
            date: "2026-10-03".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 40_000,
            items: vec![AllocateItem {
                open_item_id: item_id.clone(),
                amount: 40_000,
                paid_base: None,
            }],
            description: None,
            reference: None,
        };

        let (_entry_id, items_remaining2, _oa_item, _fx_total) =
            allocate_payment(db, input2, "alloc_eur_p2").await;

        // Item should be fully paid
        assert_eq!(items_remaining2.len(), 1);
        let (rid, tx_rem, base_rem) = &items_remaining2[0];
        assert_eq!(rid, &item_id);
        assert_eq!(*tx_rem, 0, "tx_remaining should be 0");
        assert_eq!(*base_rem, 0, "base_remaining should be 0");
    });
}

#[test]
fn test_combined_two_invoices_one_receipt() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post two EUR invoices: 600 and 400
        let item1_id = post_invoice(db, &journal_id, "cust004", 60_000, "EUR", None, "inv1").await;
        let item2_id = post_invoice(db, &journal_id, "cust004", 40_000, "EUR", None, "inv2").await;

        // Combined payment: 1000.00 EUR for both items
        let input = AllocateInput {
            ledger: "ar".to_owned(),
            party_id: "cust004".to_owned(),
            journal_id,
            date: "2026-10-02".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 100_000,
            items: vec![
                AllocateItem {
                    open_item_id: item1_id.clone(),
                    amount: 60_000,
                    paid_base: None,
                },
                AllocateItem {
                    open_item_id: item2_id.clone(),
                    amount: 40_000,
                    paid_base: None,
                },
            ],
            description: None,
            reference: None,
        };

        let (_entry_id, items_remaining, oa_item, _fx_total) =
            allocate_payment(db, input, "alloc_two_inv").await;

        // Both items should be fully paid
        assert_eq!(items_remaining.len(), 2);
        for (rid, tx_rem, base_rem) in items_remaining {
            assert!(rid == item1_id || rid == item2_id, "unknown item: {rid}");
            assert_eq!(tx_rem, 0, "item {rid} should be paid");
            assert_eq!(base_rem, 0);
        }

        // No on-account item
        assert!(oa_item.is_none(), "no on-account for exact payment");
    });
}

#[test]
fn test_overpayment_creates_on_account() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post 600.00 EUR invoice
        let item_id =
            post_invoice(db, &journal_id, "cust005", 60_000, "EUR", None, "inv_over").await;

        // Overpayment: 700.00 EUR for 600.00 EUR invoice
        let input = AllocateInput {
            ledger: "ar".to_owned(),
            party_id: "cust005".to_owned(),
            journal_id,
            date: "2026-10-02".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 70_000,
            items: vec![AllocateItem {
                open_item_id: item_id.clone(),
                amount: 60_000,
                paid_base: None,
            }],
            description: None,
            reference: None,
        };

        let (_entry_id, items_remaining, oa_item, _fx_total) =
            allocate_payment(db, input, "alloc_over").await;

        // Original item paid
        assert_eq!(items_remaining.len(), 1);
        let (rid, tx_rem, base_rem) = &items_remaining[0];
        assert_eq!(rid, &item_id);
        assert_eq!(*tx_rem, 0);
        assert_eq!(*base_rem, 0);

        // On-account item created for excess 100.00 EUR
        let oa_id = oa_item.expect("on-account item should be created");
        let oa_item_row = open_item_row(db, &oa_id)
            .await
            .expect("on-account item should exist");
        assert_eq!(oa_item_row.kind, "on-account");
        assert_eq!(oa_item_row.party_id, "cust005");
        assert_eq!(
            oa_item_row.tx_original_minor, -10_000,
            "on-account should be negative (prepayment)"
        );
        assert_eq!(oa_item_row.tx_remaining_minor, -10_000);
    });
}

#[test]
fn test_prepayment_with_no_items() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Prepayment: no items, just a receipt
        let input = AllocateInput {
            ledger: "ar".to_owned(),
            party_id: "cust_prepay".to_owned(),
            journal_id,
            date: "2026-10-02".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 50_000,
            items: vec![],
            description: None,
            reference: None,
        };

        let (_entry_id, items_remaining, oa_item, _fx_total) =
            allocate_payment(db, input, "alloc_prepay").await;

        // No items
        assert_eq!(items_remaining.len(), 0);

        // On-account item created for the full prepayment
        let oa_id = oa_item.expect("on-account item should be created");
        let oa_item_row = open_item_row(db, &oa_id)
            .await
            .expect("on-account item should exist");
        assert_eq!(oa_item_row.kind, "on-account");
        assert_eq!(oa_item_row.party_id, "cust_prepay");
        assert_eq!(
            oa_item_row.tx_original_minor, -50_000,
            "prepayment should be negative"
        );
        assert_eq!(oa_item_row.tx_remaining_minor, -50_000);
    });
}

#[test]
fn test_over_allocation_refused() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post 600.00 EUR invoice
        let item_id = post_invoice(
            db,
            &journal_id,
            "cust006",
            60_000,
            "EUR",
            None,
            "inv_exceed",
        )
        .await;

        // Try to allocate 700.00 EUR to an invoice of 600.00 EUR (exceeds remaining)
        // This should fail because amount (700000 in tx minor) > remaining (600000)
        let input = AllocateInput {
            ledger: "ar".to_owned(),
            party_id: "cust006".to_owned(),
            journal_id,
            date: "2026-10-02".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 60_000,
            items: vec![AllocateItem {
                open_item_id: item_id.clone(),
                amount: 70_000, // More than the 60000 originally posted
                paid_base: None,
            }],
            description: None,
            reference: None,
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "alloc_exceed".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerAllocate,
                "bearer".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await;

        // Should fail
        assert!(result.is_err(), "over-allocation should be rejected");

        // Check error type
        let err_str = format!("{result:?}");
        assert!(
            err_str.contains("allocation") || err_str.contains("exceed"),
            "error should mention allocation/exceed: {err_str}"
        );

        // Verify item row is unchanged
        let item = open_item_row(db, &item_id)
            .await
            .expect("item should exist");
        assert_eq!(
            item.tx_remaining_minor, 60_000,
            "remaining should be unchanged after failed allocation"
        );
        assert_eq!(
            item.base_remaining_minor, 60_000,
            "base_remaining should be unchanged after failed allocation"
        );

        // Verify no allocations were created
        let alloc_rows = db
            .query(&cratefield_core::Statement::with_values(
                "SELECT COUNT(*) as cnt FROM allocation WHERE open_item_id = ?".to_owned(),
                vec![sea_query::Value::from(item_id.as_str())],
            ))
            .await
            .expect("query should succeed");
        assert_eq!(alloc_rows.len(), 1, "should have one row");
        let cnt: i64 = alloc_rows
            .first()
            .and_then(|row| row.get("cnt"))
            .expect("should have cnt column");
        assert_eq!(cnt, 0, "no allocations should be created on failure");
    });
}

#[test]
fn test_ap_bill_payment_reduces_bill_and_credits_bank() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Post 500.00 EUR bill from vendor
        let bill_id = post_bill(
            db,
            &journal_id,
            "vendor001",
            50_000,
            "EUR",
            None,
            "bill_pay",
        )
        .await;

        // Payment: 500.00 EUR
        let input = AllocateInput {
            ledger: "ap".to_owned(),
            party_id: "vendor001".to_owned(),
            journal_id,
            date: "2026-10-02".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 50_000,
            items: vec![AllocateItem {
                open_item_id: bill_id.clone(),
                amount: 50_000,
                paid_base: None,
            }],
            description: None,
            reference: None,
        };

        let (_entry_id, items_remaining, oa_item, _fx_total) =
            allocate_payment(db, input, "alloc_ap").await;

        // Bill should be paid
        assert_eq!(items_remaining.len(), 1);
        let (rid, tx_rem, base_rem) = &items_remaining[0];
        assert_eq!(rid, &bill_id);
        assert_eq!(*tx_rem, 0, "bill should be paid");
        assert_eq!(*base_rem, 0);

        // No on-account item
        assert!(oa_item.is_none());

        // Verify bill row: tx_remaining and base_remaining should be 0
        let bill = open_item_row(db, &bill_id)
            .await
            .expect("bill should exist");
        assert_eq!(bill.tx_remaining_minor, 0);
        assert_eq!(bill.base_remaining_minor, 0);
    });
}
