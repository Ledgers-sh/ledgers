//! Multi-line `subledger.post-item`: a VAT invoice and a VAT bill post one
//! control line plus several counter lines, and a partial allocation leaves
//! the rest of the item open.

mod common;

use common::{COMPANY_ID, NOW, create_journal, harness, mount, open_item_row, person};
use cratefield_core::{Database, Statement};
use ledgers_commands::{
    CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
    auth::{Authenticator, Credential, Principal, Role},
};
use ledgers_ledger::account::{AccountDraft, AccountType};
use ledgers_ledger::commands as ledger_commands;
use ledgers_subledger::commands::{
    AllocateInput, AllocateItem, CounterLine, PostItemInput, SubledgerAllocate, SubledgerPostItem,
};
use sea_query::Value;
use std::collections::BTreeMap;

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

fn ctx(event_id: &str) -> CommandContext {
    CommandContext {
        company_id: COMPANY_ID.to_owned(),
        is_person: true,
        event_id: event_id.to_owned(),
        request_id: format!("req_{event_id}"),
        now: NOW.to_owned(),
    }
}

/// The VAT accounts the shared chart does not have: input VAT (1050, asset)
/// and output VAT (2020, liability).
async fn add_vat_accounts(db: &dyn Database) {
    for (number, name, account_type) in [
        ("1050", "account.input-vat", AccountType::Asset),
        ("2020", "account.output-vat", AccountType::Liability),
    ] {
        ledger_commands::create_account(
            db,
            COMPANY_ID,
            AccountDraft {
                number: number.to_owned(),
                name_key: name.to_owned(),
                account_type,
                normal_balance: None,
                currency: None,
                parent: None,
                tags: Vec::new(),
                display_name: None,
            },
            NOW.to_owned(),
            &person(),
        )
        .await
        .expect("create the VAT account");
    }
}

fn counter(account: &str, amount: i64, tax_code: Option<&str>) -> CounterLine {
    CounterLine {
        account_number: account.to_owned(),
        amount,
        tax_code: tax_code.map(str::to_owned),
        dimensions: BTreeMap::new(),
    }
}

/// A gross item (AR invoice or AP bill) with its counter lines; returns the
/// open item id and the entry id.
#[allow(clippy::too_many_arguments)]
async fn post_item(
    db: &dyn Database,
    journal_id: &str,
    ledger: &str,
    kind: &str,
    party_id: &str,
    gross: i64,
    counter_lines: Vec<CounterLine>,
    event_id: &str,
) -> (String, String) {
    let input = PostItemInput {
        ledger: ledger.to_owned(),
        kind: kind.to_owned(),
        party_id: party_id.to_owned(),
        journal_id: journal_id.to_owned(),
        document_date: "2026-10-05".to_owned(),
        posting_date: "2026-10-05".to_owned(),
        due_date: None,
        currency: "EUR".to_owned(),
        amount: gross,
        rate: None,
        counter_account: String::new(),
        dimensions: BTreeMap::new(),
        counter_lines,
        description: format!("{kind} {event_id}"),
        source_id: Some(format!("{kind}-{event_id}")),
    };
    let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
    match executor
        .run(
            &SubledgerPostItem,
            "bearer".to_string(),
            ctx(event_id),
            &input,
            Execute::Run,
        )
        .await
        .expect("the item posts")
    {
        Executed::Done(output) => (output.item_id, output.entry_id),
        _ => panic!("expected the item to post"),
    }
}

/// The posted lines of an entry as (account, signed base minor, tax code).
async fn lines_of(db: &dyn Database, entry_id: &str) -> Vec<(String, i64, Option<String>)> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT account_number, base_minor, tax_code FROM journal_line \
             WHERE entry_id = ? ORDER BY line_no"
                .to_owned(),
            vec![Value::from(entry_id)],
        ))
        .await
        .expect("read the lines");
    rows.rows
        .iter()
        .map(|row| {
            (
                row.get::<String>("account_number").unwrap_or_default(),
                row.get::<i64>("base_minor").unwrap_or(0),
                row.get::<String>("tax_code"),
            )
        })
        .collect()
}

#[test]
fn vat_invoice_posts_control_revenue_and_output_vat() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        add_vat_accounts(db).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Net 100.00 at 19% VAT: 119.00 gross.
        let (item_id, entry_id) = post_item(
            db,
            &journal_id,
            "ar",
            "invoice",
            "cust001",
            11_900,
            vec![
                counter("4000", 10_000, Some("19-U")),
                counter("2020", 1_900, None),
            ],
            "vat_invoice",
        )
        .await;

        let lines = lines_of(db, &entry_id).await;
        assert_eq!(
            lines,
            vec![
                ("1400".to_owned(), 11_900, None),
                ("4000".to_owned(), -10_000, Some("19-U".to_owned())),
                ("2020".to_owned(), -1_900, None),
            ],
            "control debit, revenue and output VAT credits"
        );
        let item = open_item_row(db, &item_id).await.expect("the open item");
        assert_eq!(item.tx_original_minor, 11_900);
        assert_eq!(item.tx_remaining_minor, 11_900);
    });
}

#[test]
fn vat_bill_posts_expense_and_input_vat_then_partial_allocation_leaves_the_rest() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        add_vat_accounts(db).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        // Net 100.00 expense at 19% input VAT: 119.00 gross payable.
        let (item_id, entry_id) = post_item(
            db,
            &journal_id,
            "ap",
            "bill",
            "vendor001",
            11_900,
            vec![
                counter("6000", 10_000, Some("19-U")),
                counter("1050", 1_900, None),
            ],
            "vat_bill",
        )
        .await;

        let lines = lines_of(db, &entry_id).await;
        assert_eq!(
            lines,
            vec![
                ("6000".to_owned(), 10_000, Some("19-U".to_owned())),
                ("1050".to_owned(), 1_900, None),
                ("1600".to_owned(), -11_900, None),
            ],
            "expense and input VAT debits, payable credit"
        );

        // Pay 50.00 of the 119.00 bill: the rest stays open.
        let input = AllocateInput {
            ledger: "ap".to_owned(),
            party_id: "vendor001".to_owned(),
            journal_id,
            date: "2026-10-20".to_owned(),
            bank_account: "1200".to_owned(),
            paid: 5_000,
            items: vec![AllocateItem {
                open_item_id: item_id.clone(),
                amount: 5_000,
                paid_base: None,
            }],
            description: None,
            reference: None,
        };
        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        executor
            .run(
                &SubledgerAllocate,
                "bearer".to_string(),
                ctx("vat_bill_pay"),
                &input,
                Execute::Run,
            )
            .await
            .expect("the partial payment allocates");

        let item = open_item_row(db, &item_id).await.expect("the open item");
        assert_eq!(item.tx_remaining_minor, 6_900, "119.00 less the 50.00 paid");
        assert_eq!(item.base_remaining_minor, 6_900);
    });
}

#[test]
fn counter_lines_that_do_not_sum_to_the_amount_are_refused() {
    let h = harness();
    let db = h.db.as_ref();
    pollster::block_on(async {
        mount(db, COMPANY_ID).await;
        add_vat_accounts(db).await;
        let journal_id = create_journal(db, COMPANY_ID).await;

        let input = PostItemInput {
            ledger: "ar".to_owned(),
            kind: "invoice".to_owned(),
            party_id: "cust001".to_owned(),
            journal_id,
            document_date: "2026-10-05".to_owned(),
            posting_date: "2026-10-05".to_owned(),
            due_date: None,
            currency: "EUR".to_owned(),
            amount: 11_900,
            rate: None,
            counter_account: String::new(),
            dimensions: BTreeMap::new(),
            counter_lines: vec![
                counter("4000", 10_000, Some("19-U")),
                counter("2020", 1_000, None),
            ],
            description: "mismatch".to_owned(),
            source_id: Some("mismatch".to_owned()),
        };
        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
        let err = executor
            .run(
                &SubledgerPostItem,
                "bearer".to_string(),
                ctx("mismatch"),
                &input,
                Execute::Run,
            )
            .await
            .expect_err("counter lines of 110.00 cannot post a 119.00 item");
        assert!(
            format!("{err:?}").contains("counter lines sum to 11000"),
            "{err:?}"
        );
    });
}
