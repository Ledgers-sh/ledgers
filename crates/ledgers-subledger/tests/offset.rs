//! Integration tests for subledger.offset command.

mod common;

use common::{COMPANY_ID, NOW, create_journal, harness, mount};
use ledgers_commands::{
    CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
    auth::{Authenticator, Credential, Principal, Role},
};
use ledgers_subledger::commands::{OffsetInput, SubledgerOffset};

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

async fn post_credit_note(
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
        kind: "credit-note".to_owned(),
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
        counter_lines: Vec::new(),
        description: "Test credit note".to_owned(),
        source_id: Some(format!("CN-{event_id}")),
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
        .expect("post credit note should succeed");

    match result {
        Executed::Done(output) => output.item_id,
        _ => panic!("expected Done"),
    }
}

#[test]
fn test_offset_link_only_same_rate() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;

        let journal_id = create_journal(db, COMPANY_ID).await;
        let party_id = "cust_1";

        let invoice_id = post_invoice(db, &journal_id, party_id, 30000, "EUR", None, "inv_1").await;
        let credit_id =
            post_credit_note(db, &journal_id, party_id, 10000, "EUR", None, "cn_1").await;

        let input = OffsetInput {
            ledger: "ar".to_owned(),
            party_id: party_id.to_owned(),
            journal_id: journal_id.clone(),
            date: "2026-10-15".to_owned(),
            credit_item_id: credit_id.clone(),
            debit_item_id: invoice_id.clone(),
            amount: 10000,
            description: None,
            reference: None,
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "offset_1".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerOffset,
                "bearer".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await;

        assert!(result.is_ok(), "offset should succeed");
        if let Ok(Executed::Done(output)) = result {
            assert!(output.entry_id.is_none(), "should be link-only");
            assert_eq!(
                output.fx_difference_total, 0,
                "no FX difference for same rate"
            );
        }

        // The credit note (-100.00) and the invoice (+300.00) each move by the
        // 100.00 cleared: the credit note to zero, the invoice to 200.00.
        let credit = common::open_item_row(db, &credit_id).await.expect("credit");
        assert_eq!(credit.tx_remaining_minor, 0);
        assert_eq!(
            credit.base_remaining_minor, 0,
            "the credit note clears in base too"
        );
        let invoice = common::open_item_row(db, &invoice_id)
            .await
            .expect("invoice");
        assert_eq!(invoice.tx_remaining_minor, 20000);
        assert_eq!(invoice.base_remaining_minor, 20000);
    });
}

#[test]
fn test_offset_with_fx_difference() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;

        let journal_id = create_journal(db, COMPANY_ID).await;
        let party_id = "cust_2";

        let invoice_id = post_invoice(
            db,
            &journal_id,
            party_id,
            10000,
            "USD",
            Some("0.90"),
            "inv_2",
        )
        .await;
        let credit_id = post_credit_note(
            db,
            &journal_id,
            party_id,
            10000,
            "USD",
            Some("0.95"),
            "cn_2",
        )
        .await;

        let input = OffsetInput {
            ledger: "ar".to_owned(),
            party_id: party_id.to_owned(),
            journal_id: journal_id.clone(),
            date: "2026-10-15".to_owned(),
            credit_item_id: credit_id,
            debit_item_id: invoice_id,
            amount: 10000,
            description: None,
            reference: None,
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "offset_2".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerOffset,
                "bearer".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await;

        assert!(result.is_ok(), "offset should succeed");
        if let Ok(Executed::Done(output)) = result {
            assert!(
                output.entry_id.is_some(),
                "should post entry for FX difference"
            );
            assert_eq!(output.fx_difference_total, 500, "FX gain should be 500");
        }
    });
}

#[test]
fn test_offset_different_parties_refused() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;

        let journal_id = create_journal(db, COMPANY_ID).await;
        let invoice_id = post_invoice(db, &journal_id, "cust_1", 30000, "EUR", None, "inv_3").await;
        let credit_id =
            post_credit_note(db, &journal_id, "cust_2", 10000, "EUR", None, "cn_3").await;

        let input = OffsetInput {
            ledger: "ar".to_owned(),
            party_id: "cust_1".to_owned(),
            journal_id: journal_id.clone(),
            date: "2026-10-15".to_owned(),
            credit_item_id: credit_id,
            debit_item_id: invoice_id,
            amount: 10000,
            description: None,
            reference: None,
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "offset_3".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerOffset,
                "bearer".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await;

        assert!(result.is_err(), "should refuse offset across parties");
    });
}

#[test]
fn test_offset_amount_exceeds_remaining_refused() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;

        let journal_id = create_journal(db, COMPANY_ID).await;
        let party_id = "cust_4";

        let invoice_id = post_invoice(db, &journal_id, party_id, 30000, "EUR", None, "inv_4").await;
        let credit_id =
            post_credit_note(db, &journal_id, party_id, 10000, "EUR", None, "cn_4").await;

        let input = OffsetInput {
            ledger: "ar".to_owned(),
            party_id: party_id.to_owned(),
            journal_id: journal_id.clone(),
            date: "2026-10-15".to_owned(),
            credit_item_id: credit_id,
            debit_item_id: invoice_id,
            amount: 15000,
            description: None,
            reference: None,
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "offset_4".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerOffset,
                "bearer".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await;

        assert!(
            result.is_err(),
            "should refuse offset that exceeds remaining"
        );
    });
}
