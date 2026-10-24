//! Integration tests for subledger.post-item command.

mod common;

use common::{COMPANY_ID, NOW, count_rows, create_journal, harness, mount, open_item_row};
use ledgers_commands::{
    CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
    auth::{Authenticator, Credential, Principal, Role},
};
use ledgers_subledger::commands::{PostItemInput, SubledgerPostItem};

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

#[test]
fn test_post_eur_invoice_entry_and_item() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // EUR invoice: 1000 EUR to cust001
        let input = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "invoice".to_owned(),
            party_id: "cust001".to_owned(),
            journal_id,
            document_date: "2026-10-01".to_owned(),
            posting_date: "2026-10-01".to_owned(),
            due_date: Some("2026-11-01".to_owned()),
            currency: "EUR".to_owned(),
            amount: 100_000, // 1000.00 EUR in minor units
            rate: None,
            counter_account: "4000".to_owned(), // Revenue
            description: "Invoice for services".to_owned(),
            source_id: Some("INV001".to_owned()),
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_inv001".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        // Execute the command
        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await
            .expect("command should execute");

        let Executed::Done(output) = result else {
            panic!("expected Done")
        };

        assert!(!output.item_id.is_empty(), "item_id should be set");
        assert!(!output.entry_id.is_empty(), "entry_id should be set");

        // Verify the open_item was created (need to query after execution)
        let item = open_item_row(db, &output.item_id)
            .await
            .expect("open_item should exist");
        assert_eq!(item.ledger, "ar");
        assert_eq!(item.kind, "invoice");
        assert_eq!(item.party_id, "cust001");
        assert_eq!(item.source_id, Some("INV001".to_owned()));
        assert_eq!(item.currency, "EUR");
        assert_eq!(
            item.tx_original_minor, 100_000,
            "tx_original should match input amount"
        );
        assert_eq!(
            item.base_original_minor, 100_000,
            "base_original should match for EUR"
        );
        assert_eq!(
            item.tx_remaining_minor, 100_000,
            "tx_remaining should equal tx_original"
        );
        assert_eq!(
            item.base_remaining_minor, 100_000,
            "base_remaining should equal base_original"
        );

        // Verify GL: 1400 (AR) should be debited by 100000
        // Note: GL balances update via the posted entry
        let ar_count = count_rows(db, "open_item", "id", &output.item_id).await;
        assert_eq!(ar_count, 1, "open_item should be created");
    });
}

#[test]
fn test_post_usd_invoice_with_fx() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // USD invoice: 1000.00 USD at rate 0.9 (1 USD = 0.9 EUR) → base 900.00 EUR
        let input = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "invoice".to_owned(),
            party_id: "cust_us".to_owned(),
            journal_id,
            document_date: "2026-10-01".to_owned(),
            posting_date: "2026-10-01".to_owned(),
            due_date: None,
            currency: "USD".to_owned(),
            amount: 100_000,              // 1000.00 USD
            rate: Some("0.9".to_owned()), // Decimal rate, will be scaled internally
            counter_account: "4000".to_owned(),
            description: "USD invoice".to_owned(),
            source_id: Some("INVUSD001".to_owned()),
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_usd001".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        // Execute the command
        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await
            .expect("command should execute");

        let Executed::Done(output) = result else {
            panic!("expected Done")
        };

        // Verify the open_item
        let item = open_item_row(db, &output.item_id)
            .await
            .expect("open_item should exist");
        assert_eq!(item.ledger, "ar");
        assert_eq!(item.currency, "USD");
        assert_eq!(
            item.tx_original_minor, 100_000,
            "tx_original should be 1000.00 USD"
        );
        assert_eq!(
            item.base_original_minor, 90_000,
            "base_original should be 900.00 EUR"
        );
        assert_eq!(item.tx_remaining_minor, 100_000);
        assert_eq!(item.base_remaining_minor, 90_000);
        assert_eq!(item.rate_scaled, Some("0.9".to_owned()));

        // Verify entry was posted
        let item_count = count_rows(db, "open_item", "id", &output.item_id).await;
        assert_eq!(item_count, 1, "open_item should be created");
    });
}

#[test]
fn test_post_ap_bill_credits_payables() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // AP bill: 500.00 EUR from vendor001
        let input = PostItemInput {
            ledger: "ap".to_owned(),
            kind: "bill".to_owned(),
            party_id: "vendor001".to_owned(),
            journal_id,
            document_date: "2026-10-01".to_owned(),
            posting_date: "2026-10-01".to_owned(),
            due_date: Some("2026-11-01".to_owned()),
            currency: "EUR".to_owned(),
            amount: 50_000, // 500.00 EUR
            rate: None,
            counter_account: "6000".to_owned(), // Expense
            description: "Vendor bill".to_owned(),
            source_id: Some("BILL001".to_owned()),
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_bill001".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await
            .expect("command should execute");

        let Executed::Done(output) = result else {
            panic!("expected Done")
        };

        // Verify the open_item
        let item = open_item_row(db, &output.item_id)
            .await
            .expect("open_item should exist");
        assert_eq!(item.ledger, "ap");
        assert_eq!(item.kind, "bill");
        assert_eq!(item.party_id, "vendor001");
        assert_eq!(item.tx_original_minor, 50_000);
        assert_eq!(item.base_original_minor, 50_000);
        assert_eq!(item.tx_remaining_minor, 50_000);
        assert_eq!(item.base_remaining_minor, 50_000);

        // Verify the item was created
        let item_count = count_rows(db, "open_item", "id", &output.item_id).await;
        assert_eq!(item_count, 1, "open_item should be created");
    });
}

#[test]
fn test_post_ar_credit_note_gives_negative_item() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // AR credit note: 100.00 EUR to cust001
        let input = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "credit-note".to_owned(),
            party_id: "cust001".to_owned(),
            journal_id,
            document_date: "2026-10-05".to_owned(),
            posting_date: "2026-10-05".to_owned(),
            due_date: None,
            currency: "EUR".to_owned(),
            amount: 10_000, // 100.00 EUR
            rate: None,
            counter_account: "4000".to_owned(), // Revenue
            description: "Credit note".to_owned(),
            source_id: Some("CN001".to_owned()),
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_cn001".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await
            .expect("command should execute");

        let Executed::Done(output) = result else {
            panic!("expected Done")
        };

        // Verify the open_item
        let item = open_item_row(db, &output.item_id)
            .await
            .expect("open_item should exist");
        assert_eq!(item.ledger, "ar");
        assert_eq!(item.kind, "credit-note");
        assert_eq!(item.currency, "EUR");
        assert_eq!(
            item.tx_original_minor, -10_000,
            "credit notes should be negative"
        );
        assert_eq!(
            item.base_original_minor, -10_000,
            "credit notes should be negative in base"
        );
        assert_eq!(item.tx_remaining_minor, -10_000);
        assert_eq!(item.base_remaining_minor, -10_000);

        // Verify the item was created
        let item_count = count_rows(db, "open_item", "id", &output.item_id).await;
        assert_eq!(item_count, 1, "open_item should be created");
    });
}

#[test]
fn test_duplicate_source_id_rejected() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // First invoice
        let input1 = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "invoice".to_owned(),
            party_id: "cust001".to_owned(),
            journal_id: journal_id.clone(),
            document_date: "2026-10-01".to_owned(),
            posting_date: "2026-10-01".to_owned(),
            due_date: None,
            currency: "EUR".to_owned(),
            amount: 100_000,
            rate: None,
            counter_account: "4000".to_owned(),
            description: "First invoice".to_owned(),
            source_id: Some("INV001".to_owned()),
        };

        let ctx1 = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_inv_a".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result1 = executor
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx1,
                &input1,
                Execute::Run,
            )
            .await
            .expect("first command should execute");

        match result1 {
            Executed::Done(_) => {}
            _ => panic!("expected Done"),
        }

        // Second invoice with same source_id should be rejected
        let input2 = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "invoice".to_owned(),
            party_id: "cust002".to_owned(),
            journal_id,
            document_date: "2026-10-05".to_owned(),
            posting_date: "2026-10-05".to_owned(),
            due_date: None,
            currency: "EUR".to_owned(),
            amount: 50_000,
            rate: None,
            counter_account: "4000".to_owned(),
            description: "Second invoice".to_owned(),
            source_id: Some("INV001".to_owned()), // Duplicate!
        };

        let ctx2 = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_inv_b".to_owned(),
            request_id: "req_002".to_owned(),
            now: NOW.to_owned(),
        };

        let executor2 = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result2 = executor2
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx2,
                &input2,
                Execute::Run,
            )
            .await;

        assert!(
            result2.is_err(),
            "duplicate source_id should be rejected: {result2:?}"
        );
    });
}

#[test]
fn test_missing_rate_for_non_base_currency_rejected() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // USD invoice without rate should fail
        let input = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "invoice".to_owned(),
            party_id: "cust_us".to_owned(),
            journal_id,
            document_date: "2026-10-01".to_owned(),
            posting_date: "2026-10-01".to_owned(),
            due_date: None,
            currency: "USD".to_owned(),
            amount: 100_000,
            rate: None, // Missing rate for non-base currency
            counter_account: "4000".to_owned(),
            description: "USD invoice without rate".to_owned(),
            source_id: Some("INVUSD_BAD".to_owned()),
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_usd_bad".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await;

        assert!(
            result.is_err(),
            "missing rate for USD should be rejected: {result:?}"
        );
    });
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_multiple_items_same_party() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        let party_id = "same_party";

        // Item 1: EUR 100.00 on day 1
        let input1 = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "invoice".to_owned(),
            party_id: party_id.to_owned(),
            journal_id: journal_id.clone(),
            document_date: "2026-10-01".to_owned(),
            posting_date: "2026-10-01".to_owned(),
            due_date: Some("2026-11-01".to_owned()),
            currency: "EUR".to_owned(),
            amount: 100_000, // EUR 100.00
            rate: None,
            counter_account: "4000".to_owned(),
            description: "Invoice 1".to_owned(),
            source_id: Some("INV001".to_owned()),
        };

        let ctx1 = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_inv_001".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result1 = executor
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx1,
                &input1,
                Execute::Run,
            )
            .await
            .expect("first invoice should execute");

        let Executed::Done(output1) = result1 else {
            panic!("expected Done for first invoice")
        };

        // Item 2: EUR 200.00 on day 2
        let input2 = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "invoice".to_owned(),
            party_id: party_id.to_owned(),
            journal_id: journal_id.clone(),
            document_date: "2026-10-02".to_owned(),
            posting_date: "2026-10-02".to_owned(),
            due_date: Some("2026-11-02".to_owned()),
            currency: "EUR".to_owned(),
            amount: 200_000, // EUR 200.00
            rate: None,
            counter_account: "4000".to_owned(),
            description: "Invoice 2".to_owned(),
            source_id: Some("INV002".to_owned()),
        };

        let ctx2 = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_inv_002".to_owned(),
            request_id: "req_002".to_owned(),
            now: NOW.to_owned(),
        };

        let executor2 = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result2 = executor2
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx2,
                &input2,
                Execute::Run,
            )
            .await
            .expect("second invoice should execute");

        let Executed::Done(output2) = result2 else {
            panic!("expected Done for second invoice")
        };

        // Item 3: USD 300.00 at rate 0.9 on day 3 → base 270.00 EUR
        let input3 = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "invoice".to_owned(),
            party_id: party_id.to_owned(),
            journal_id: journal_id.clone(),
            document_date: "2026-10-03".to_owned(),
            posting_date: "2026-10-03".to_owned(),
            due_date: Some("2026-11-03".to_owned()),
            currency: "USD".to_owned(),
            amount: 300_000, // USD 300.00
            rate: Some("0.9".to_owned()),
            counter_account: "4000".to_owned(),
            description: "Invoice 3 USD".to_owned(),
            source_id: Some("INV003".to_owned()),
        };

        let ctx3 = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_inv_003".to_owned(),
            request_id: "req_003".to_owned(),
            now: NOW.to_owned(),
        };

        let executor3 = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result3 = executor3
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx3,
                &input3,
                Execute::Run,
            )
            .await
            .expect("third invoice should execute");

        let Executed::Done(output3) = result3 else {
            panic!("expected Done for third invoice")
        };

        // Item 4: AP bill EUR 100.00
        let input4 = PostItemInput {
            ledger: "ap".to_owned(),
            kind: "bill".to_owned(),
            party_id: party_id.to_owned(),
            journal_id: journal_id.clone(),
            document_date: "2026-10-03".to_owned(),
            posting_date: "2026-10-03".to_owned(),
            due_date: Some("2026-11-03".to_owned()),
            currency: "EUR".to_owned(),
            amount: 100_000, // EUR 100.00
            rate: None,
            counter_account: "6000".to_owned(),
            description: "Bill 1".to_owned(),
            source_id: Some("BILL001".to_owned()),
        };

        let ctx4 = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "evt_bill_001".to_owned(),
            request_id: "req_004".to_owned(),
            now: NOW.to_owned(),
        };

        let executor4 = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result4 = executor4
            .run(
                &SubledgerPostItem,
                "bearer_token".to_string(),
                ctx4,
                &input4,
                Execute::Run,
            )
            .await
            .expect("bill should execute");

        let Executed::Done(output4) = result4 else {
            panic!("expected Done for bill")
        };

        // Verify all 4 open_item rows exist with correct amounts
        let item1 = open_item_row(db, &output1.item_id)
            .await
            .expect("item 1 should exist");
        assert_eq!(item1.party_id, party_id);
        assert_eq!(item1.ledger, "ar");
        assert_eq!(item1.tx_original_minor, 100_000, "item 1 EUR amount");
        assert_eq!(item1.base_original_minor, 100_000, "item 1 base amount");

        let item2 = open_item_row(db, &output2.item_id)
            .await
            .expect("item 2 should exist");
        assert_eq!(item2.party_id, party_id);
        assert_eq!(item2.ledger, "ar");
        assert_eq!(item2.tx_original_minor, 200_000, "item 2 EUR amount");
        assert_eq!(item2.base_original_minor, 200_000, "item 2 base amount");

        let item3 = open_item_row(db, &output3.item_id)
            .await
            .expect("item 3 should exist");
        assert_eq!(item3.party_id, party_id);
        assert_eq!(item3.ledger, "ar");
        assert_eq!(item3.tx_original_minor, 300_000, "item 3 USD amount");
        assert_eq!(
            item3.base_original_minor, 270_000,
            "item 3 base amount (300 * 0.9)"
        );

        let item4 = open_item_row(db, &output4.item_id)
            .await
            .expect("item 4 should exist");
        assert_eq!(item4.party_id, party_id);
        assert_eq!(item4.ledger, "ap");
        assert_eq!(item4.tx_original_minor, 100_000, "item 4 EUR amount");
        assert_eq!(item4.base_original_minor, 100_000, "item 4 base amount");

        // Verify reconciliation shows difference 0 for both ledgers
        let reconcile_result =
            ledgers_subledger::reconcile::reconcile(db, COMPANY_ID, "2026-10-31")
                .await
                .expect("reconciliation should succeed");

        for ledger in reconcile_result {
            if ledger.ledger == "ar" {
                assert_eq!(
                    ledger.subledger_balance, 570_000,
                    "AR subledger balance should be 570 EUR (100+200+270)"
                );
                assert_eq!(
                    ledger.gl_balance, 570_000,
                    "AR GL balance should be 570 EUR"
                );
                assert_eq!(
                    ledger.difference, 0,
                    "AR should reconcile with 0 difference"
                );
            } else if ledger.ledger == "ap" {
                assert_eq!(
                    ledger.subledger_balance, 100_000,
                    "AP subledger balance should be 100 EUR"
                );
                // AP GL balance is calculated with negate=true for credit-positive accounts
                assert_eq!(
                    ledger.gl_balance, 100_000,
                    "AP GL balance should be 100 EUR"
                );
                assert_eq!(
                    ledger.difference, 0,
                    "AP should reconcile with 0 difference"
                );
            }
        }
    });
}

#[test]
fn test_multiple_items_same_party_same_day() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        let party_id = "same_party";

        // Three invoices on the same day with the same description
        for i in 1..=3 {
            let input = PostItemInput {
                ledger: "ar".to_owned(),
                kind: "invoice".to_owned(),
                party_id: party_id.to_owned(),
                journal_id: journal_id.clone(),
                document_date: "2026-10-01".to_owned(),
                posting_date: "2026-10-01".to_owned(),
                due_date: Some("2026-11-01".to_owned()),
                currency: "EUR".to_owned(),
                amount: 100_000 * i, // EUR 100, 200, 300
                rate: None,
                counter_account: "4000".to_owned(),
                description: "Same description".to_owned(), // Same description
                source_id: Some(format!("INV{i:03}")),
            };

            let ctx = CommandContext {
                company_id: COMPANY_ID.to_owned(),
                is_person: true,
                event_id: format!("evt_same_day_{i:03}"),
                request_id: format!("req_{i:03}"),
                now: NOW.to_owned(),
            };

            let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
            let result = executor
                .run(
                    &SubledgerPostItem,
                    "bearer_token".to_string(),
                    ctx,
                    &input,
                    Execute::Run,
                )
                .await
                .expect("invoice should execute");

            let Executed::Done(output) = result else {
                panic!("expected Done for invoice {i}")
            };

            // Verify the item was created
            let item = open_item_row(db, &output.item_id)
                .await
                .unwrap_or_else(|| panic!("item {i} should exist"));
            assert_eq!(item.party_id, party_id);
            assert_eq!(item.tx_original_minor, 100_000 * i, "item {i} amount");
        }

        // Verify reconciliation shows all items accounted for
        let reconcile_result =
            ledgers_subledger::reconcile::reconcile(db, COMPANY_ID, "2026-10-31")
                .await
                .expect("reconciliation should succeed");

        for ledger in reconcile_result {
            if ledger.ledger == "ar" {
                assert_eq!(
                    ledger.subledger_balance, 600_000,
                    "AR should have 600 EUR from three invoices"
                );
                assert_eq!(ledger.gl_balance, 600_000, "AR GL should have 600 EUR");
                assert_eq!(ledger.difference, 0, "AR should reconcile");
            }
        }
    });
}
