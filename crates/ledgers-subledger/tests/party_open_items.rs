//! `parties.open_items`: a party's receivables and payables still open, read
//! from the subledger the same way the detail page reads them.

mod common;

use common::{COMPANY_ID, NOW, create_journal, harness, mount};
use ledgers_commands::{
    CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
    auth::{Authenticator, Credential, Principal, Role},
};
use ledgers_subledger::commands::{PostItemInput, SubledgerPostItem};
use ledgers_subledger::party_open_items::{OpenItemsInput, PartiesOpenItems};
use std::collections::BTreeMap;

struct Owner;

#[async_trait::async_trait]
impl Authenticator for Owner {
    async fn authenticate(&self, _credential: &Credential) -> Result<Principal, CommandError> {
        Ok(Principal::Person {
            person_id: "test_person".to_string(),
            role: Role::Owner,
        })
    }
}

fn ctx(event: &str) -> CommandContext {
    CommandContext {
        company_id: COMPANY_ID.to_owned(),
        is_person: true,
        event_id: format!("evt_{event}"),
        request_id: format!("req_{event}"),
        now: NOW.to_owned(),
    }
}

#[test]
fn a_party_sees_only_its_own_open_items_and_their_totals() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        let journal_id = create_journal(db, COMPANY_ID).await;
        let executor = Executor::new(db, Owner, DefaultPolicy);

        for (party, amount, event) in [
            ("cust001", 100_000, "inv1"),
            ("cust001", 25_000, "inv2"),
            ("cust002", 40_000, "inv3"),
        ] {
            let input = PostItemInput {
                ledger: "ar".to_owned(),
                kind: "invoice".to_owned(),
                party_id: party.to_owned(),
                journal_id: journal_id.clone(),
                document_date: "2026-10-01".to_owned(),
                posting_date: "2026-10-01".to_owned(),
                due_date: Some("2026-11-01".to_owned()),
                currency: "EUR".to_owned(),
                amount,
                rate: None,
                counter_account: "4000".to_owned(),
                dimensions: BTreeMap::default(),
                counter_lines: Vec::new(),
                description: format!("Invoice {event}"),
                source_id: Some(event.to_owned()),
            };
            let posted = executor
                .run(
                    &SubledgerPostItem,
                    "bearer_token".to_string(),
                    ctx(event),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("the invoice posts");
            assert!(matches!(posted, Executed::Done(_)));
        }

        let read = executor
            .run(
                &PartiesOpenItems,
                "bearer_token".to_string(),
                ctx("open-cust001"),
                &OpenItemsInput {
                    id: "cust001".to_owned(),
                },
                Execute::Run,
            )
            .await
            .expect("the read runs");
        let Executed::Done(out) = read else {
            panic!("expected the read to complete")
        };
        assert_eq!(
            out.open_items.len(),
            2,
            "both of cust001's invoices are open"
        );
        assert!(
            out.open_items
                .iter()
                .all(|item| item.ledger == "ar" && item.currency == "EUR")
        );
        assert_eq!(out.open_items[0].document.as_deref(), Some("inv1"));
        assert_eq!(out.balances.len(), 1);
        assert_eq!(out.balances[0].remaining_minor, 125_000);

        let other = executor
            .run(
                &PartiesOpenItems,
                "bearer_token".to_string(),
                ctx("open-nobody"),
                &OpenItemsInput {
                    id: "nobody".to_owned(),
                },
                Execute::Run,
            )
            .await
            .expect("the read runs");
        let Executed::Done(none) = other else {
            panic!("expected the read to complete")
        };
        assert!(none.open_items.is_empty(), "a party with no items has none");
        assert!(none.balances.is_empty());
    });
}
