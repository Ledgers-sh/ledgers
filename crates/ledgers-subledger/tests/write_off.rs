//! Integration tests for subledger.write-off command.

mod common;

use common::{COMPANY_ID, NOW, create_journal, harness, mount};
use ledgers_commands::{
    CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
    auth::{Authenticator, Credential, Limits, Principal, Role},
};
use ledgers_subledger::commands::{SubledgerWriteOff, WriteOffInput};

struct PersonAuthenticator;

#[async_trait::async_trait]
impl Authenticator for PersonAuthenticator {
    async fn authenticate(&self, _credential: &Credential) -> Result<Principal, CommandError> {
        Ok(Principal::Person {
            person_id: "test_person".to_string(),
            role: Role::Owner,
        })
    }
}

struct AgentAuthenticator;

#[async_trait::async_trait]
impl Authenticator for AgentAuthenticator {
    async fn authenticate(&self, _credential: &Credential) -> Result<Principal, CommandError> {
        Ok(Principal::Agent {
            key_id: "agent_key_1".to_string(),
            tier: ledgers_commands::auth::Tier::PostWithinLimits,
            limits: Limits::new(),
        })
    }
}

async fn post_invoice(
    db: &dyn cratefield_core::Database,
    journal_id: &str,
    party_id: &str,
    amount: i64,
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
        currency: "EUR".to_owned(),
        amount,
        rate: None,
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

    let executor = Executor::new(db, PersonAuthenticator, DefaultPolicy);
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

#[test]
fn test_write_off_by_person() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;

        let journal_id = create_journal(db, COMPANY_ID).await;
        let invoice_id = post_invoice(db, &journal_id, "cust_wo_1", 20000, "wo_person_1").await;

        let input = WriteOffInput {
            open_item_id: invoice_id,
            journal_id,
            date: "2026-10-15".to_owned(),
            amount: 12000,
            account: "6950".to_owned(),
            reason: "Bad debt".to_owned(),
            description: None,
            reference: None,
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: true,
            event_id: "wo_1".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, PersonAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerWriteOff,
                "bearer".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await;

        assert!(result.is_ok(), "person should be able to write off");
        if let Ok(Executed::Done(output)) = result {
            assert!(!output.entry_id.is_empty(), "entry should be posted");
            assert_eq!(output.tx_remaining, 8000, "should have 80.00 remaining");
        }
    });
}

#[test]
fn test_write_off_by_agent_within_limit() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;

        let journal_id = create_journal(db, COMPANY_ID).await;
        let invoice_id = post_invoice(db, &journal_id, "cust_wo_2", 10000, "wo_agent_1").await;

        let input = WriteOffInput {
            open_item_id: invoice_id,
            journal_id,
            date: "2026-10-15".to_owned(),
            amount: 2000,
            account: "6950".to_owned(),
            reason: "Discount".to_owned(),
            description: None,
            reference: None,
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: false,
            event_id: "wo_2".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, AgentAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerWriteOff,
                "bearer".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await;

        assert!(
            result.is_ok(),
            "agent should be able to write off within limit"
        );
        if let Ok(Executed::Done(output)) = result {
            assert_eq!(output.tx_remaining, 8000, "should have 80.00 remaining");
        }
    });
}

#[test]
fn test_write_off_by_agent_exceeds_limit() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;

        let journal_id = create_journal(db, COMPANY_ID).await;
        let invoice_id = post_invoice(db, &journal_id, "cust_wo_3", 20000, "wo_agent_2").await;

        let input = WriteOffInput {
            open_item_id: invoice_id,
            journal_id,
            date: "2026-10-15".to_owned(),
            amount: 12000,
            account: "6950".to_owned(),
            reason: "Bad debt".to_owned(),
            description: None,
            reference: None,
        };

        let ctx = CommandContext {
            company_id: COMPANY_ID.to_owned(),
            is_person: false,
            event_id: "wo_3".to_owned(),
            request_id: "req_001".to_owned(),
            now: NOW.to_owned(),
        };

        let executor = Executor::new(db, AgentAuthenticator, DefaultPolicy);
        let result = executor
            .run(
                &SubledgerWriteOff,
                "bearer".to_string(),
                ctx,
                &input,
                Execute::Run,
            )
            .await;

        assert!(
            result.is_err(),
            "agent should be refused for write-off exceeding limit"
        );
        if let Err(CommandError::Domain(problem)) = result {
            assert_eq!(problem.slug, "write-off-requires-person");
        }
    });
}
