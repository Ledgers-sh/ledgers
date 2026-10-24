//! Test AP open items creation when posting bills and allocations when recording payments.
//! Tests the integration between bills and ledgers-subledger.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_bills::BillsModule;
use ledgers_bills::commands::{BillsPost, BillsRecordPayment, PostInput, RecordPaymentInput};
use ledgers_commands::auth::{Authenticator, Credential, Principal, Role};
use ledgers_commands::{
    Command, CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
};
use ledgers_db::DbModule;
use ledgers_ledger::LedgerModule;
use ledgers_storage::StorageModule;
use ledgers_subledger::SubledgerModule;
use ledgers_subledger::reconcile::reconcile;
use sea_query::Value;
use std::collections::BTreeMap;

const COMPANY_ID: &str = "01TESTCOMPANY0000000000000A";
const NOW: &str = "2026-10-24T09:30:00Z";
const OWNER_ID: &str = "owner-test";
const OWNER_TOKEN: &str = "bearer-owner";

// Simple test authenticator
struct TestAuthenticator {
    credentials: BTreeMap<String, Principal>,
}

impl TestAuthenticator {
    fn new() -> Self {
        let mut credentials = BTreeMap::new();
        credentials.insert(
            OWNER_TOKEN.to_string(),
            Principal::Person {
                person_id: OWNER_ID.to_string(),
                role: Role::Owner,
            },
        );
        TestAuthenticator { credentials }
    }
}

#[async_trait::async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        self.credentials
            .get(credential)
            .cloned()
            .ok_or(CommandError::Unauthenticated)
    }
}

fn modules() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![
        Box::new(DbModule),
        Box::new(StorageModule),
        Box::new(LedgerModule),
        Box::new(SubledgerModule),
        Box::new(BillsModule),
    ]
}

fn context_in(company_id: &str, request_id: &str) -> CommandContext {
    CommandContext {
        company_id: company_id.to_string(),
        request_id: request_id.to_string(),
        event_id: format!("{request_id}-event"),
        now: NOW.to_string(),
        is_person: true,
    }
}

async fn run_command<C: Command>(
    db: &dyn Database,
    cmd: &C,
    input: &C::Input,
    request_id: &str,
) -> Result<C::Output, ledgers_commands::CommandError> {
    use ledgers_commands::CommandError;
    let executor = Executor::new(db, TestAuthenticator::new(), DefaultPolicy);
    match executor
        .run(
            cmd,
            OWNER_TOKEN.to_string(),
            context_in(COMPANY_ID, request_id),
            input,
            Execute::Run,
        )
        .await?
    {
        Executed::Done(output) => Ok(output),
        _ => Err(CommandError::InvalidInput(
            "unexpected executor state".to_string(),
        )),
    }
}

async fn exec(db: &dyn Database, sql: &str, values: Vec<Value>) {
    db.execute(&Statement::with_values(sql.to_string(), values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}\n{sql}"));
}

async fn seed_chart(db: &dyn Database, company_id: &str) {
    // Create accounts with appropriate tags using correct schema
    // account(company_id, number, name_key, account_type, normal_balance, tags, archived, created_at, updated_at)
    exec(
        db,
        "INSERT INTO account (company_id, number, name_key, account_type, normal_balance, tags, archived, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)",
        vec![
            Value::from(company_id),
            Value::from("2000"),
            Value::from("ap_control"),
            Value::from("liability"),
            Value::from("credit"),
            Value::from("ap"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;

    exec(
        db,
        "INSERT INTO account (company_id, number, name_key, account_type, normal_balance, tags, archived, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)",
        vec![
            Value::from(company_id),
            Value::from("6000"),
            Value::from("expense"),
            Value::from("expense"),
            Value::from("debit"),
            Value::from(""),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;

    exec(
        db,
        "INSERT INTO account (company_id, number, name_key, account_type, normal_balance, tags, archived, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)",
        vec![
            Value::from(company_id),
            Value::from("1010"),
            Value::from("bank"),
            Value::from("asset"),
            Value::from("debit"),
            Value::from("bank"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;

    exec(
        db,
        "INSERT INTO account (company_id, number, name_key, account_type, normal_balance, tags, archived, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)",
        vec![
            Value::from(company_id),
            Value::from("8100"),
            Value::from("fx_gain"),
            Value::from("income"),
            Value::from("credit"),
            Value::from("fx-gain"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;

    exec(
        db,
        "INSERT INTO account (company_id, number, name_key, account_type, normal_balance, tags, archived, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)",
        vec![
            Value::from(company_id),
            Value::from("8200"),
            Value::from("fx_loss"),
            Value::from("expense"),
            Value::from("debit"),
            Value::from("fx-loss"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;

    exec(
        db,
        "INSERT INTO account (company_id, number, name_key, account_type, normal_balance, tags, archived, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)",
        vec![
            Value::from(company_id),
            Value::from("5000"),
            Value::from("input_vat"),
            Value::from("expense"),
            Value::from("debit"),
            Value::from("vat-input"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;
}

async fn seed_company(db: &dyn Database) {
    exec(
        db,
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        vec![
            Value::from(COMPANY_ID),
            Value::from("Test Company"),
            Value::from("EUR"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;

    // Open the 2026-10 period
    exec(
        db,
        "INSERT INTO accounting_period (company_id, period, status) VALUES (?, ?, ?)",
        vec![
            Value::from(COMPANY_ID),
            Value::from("2026-10"),
            Value::from("open"),
        ],
    )
    .await;

    seed_chart(db, COMPANY_ID).await;

    // Create general journal
    exec(
        db,
        "INSERT INTO journal (company_id, id, code, kind, name, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
        vec![
            Value::from(COMPANY_ID),
            Value::from("GN"),
            Value::from("GN"),
            Value::from("general"),
            Value::from("General Journal"),
            Value::from(NOW),
        ],
    )
    .await;

    // Create vendor
    exec(
        db,
        "INSERT INTO vendor (id, company_id, name, vat_id, iban, source, needs_review, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)",
        vec![
            Value::from("vendor-1"),
            Value::from(COMPANY_ID),
            Value::from("Acme GmbH"),
            Value::from("DE123456789"),
            Value::from(None::<String>),
            Value::from("manual"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;
}

async fn test_bill_posting_and_payment_impl(db: &dyn Database) {
    seed_company(db).await;

    // Create a bill manually
    let bill_id = "bill-001";
    let invoice_number = "INV-2026-001";
    let due_date = "2026-11-24";
    let gross_minor = 10000i64; // EUR 100.00
    let vat_minor = 1500i64; // EUR 15.00
    let net_minor = 8500i64; // EUR 85.00

    exec(
        db,
        "INSERT INTO bill (id, company_id, document_id, vendor_id, invoice_number, invoice_date, \
         due_date, currency, net_minor, vat_minor, gross_minor, lines_json, checks_json, \
         instruction_flags, iban_changed, source, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)",
        vec![
            Value::from(bill_id),
            Value::from(COMPANY_ID),
            Value::from("doc-001"),
            Value::from("vendor-1"),
            Value::from(invoice_number),
            Value::from("2026-10-01"),
            Value::from(due_date),
            Value::from("EUR"),
            Value::from(net_minor),
            Value::from(vat_minor),
            Value::from(gross_minor),
            Value::from(
                r#"[{"sequence":1,"expense_account":"6000","description":"Supplies","net_minor":8500}]"#
            ),
            Value::from("[]"),
            Value::from("[]"),
            Value::from("manual"),
            Value::from("test"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;

    // Post the bill
    let post_input = PostInput {
        id: bill_id.to_string(),
        journal_id: Some("GN".to_string()),
        fiscal_year: Some(2026),
    };
    let _post_output = run_command(db, &BillsPost, &post_input, "post-001")
        .await
        .expect("post-entry should succeed");

    // Verify the open_item was created
    let item_rows = db
        .query(&Statement::with_values(
            "SELECT id, company_id, ledger, kind, party_id, source_id, tx_original_minor, \
             tx_remaining_minor, base_original_minor, base_remaining_minor, due_date \
             FROM open_item WHERE company_id = ? AND kind = 'bill' AND source_id = ?"
                .to_string(),
            vec![Value::from(COMPANY_ID), Value::from(bill_id)],
        ))
        .await
        .expect("query should succeed");

    assert_eq!(item_rows.rows.len(), 1, "should have one open item");
    let item = &item_rows.rows[0];
    assert_eq!(
        item.get::<String>("ledger").unwrap_or_default(),
        "ap",
        "ledger should be ap"
    );
    assert_eq!(
        item.get::<i64>("tx_original_minor").unwrap_or(0),
        gross_minor,
        "original tx amount should be gross"
    );
    assert_eq!(
        item.get::<i64>("tx_remaining_minor").unwrap_or(0),
        gross_minor,
        "remaining tx should equal original after posting"
    );
    let item_id = item.get::<String>("id").unwrap_or_default();

    // Record a partial payment
    let payment_amount = 6000i64; // EUR 60.00
    let payment_input = RecordPaymentInput {
        id: bill_id.to_string(),
        amount: payment_amount,
        date: "2026-10-20".to_string(),
        from_account: "1010".to_string(),
        reference: Some("payment ref".to_string()),
    };

    let _payment_output = run_command(db, &BillsRecordPayment, &payment_input, "payment-001")
        .await
        .expect("record-payment should succeed");

    // Verify the allocation row was created
    let alloc_rows = db
        .query(&Statement::with_values(
            "SELECT id, open_item_id, tx_minor, base_minor FROM allocation \
             WHERE company_id = ? AND open_item_id = ?"
                .to_string(),
            vec![Value::from(COMPANY_ID), Value::from(&item_id)],
        ))
        .await
        .expect("query should succeed");

    assert_eq!(alloc_rows.rows.len(), 1, "should have one allocation");
    let alloc = &alloc_rows.rows[0];
    assert_eq!(
        alloc.get::<i64>("tx_minor").unwrap_or(0),
        payment_amount,
        "allocation tx amount should match payment"
    );

    // Verify the open_item remaining was decremented
    let updated_item_rows = db
        .query(&Statement::with_values(
            "SELECT tx_remaining_minor, base_remaining_minor FROM open_item WHERE id = ?"
                .to_string(),
            vec![Value::from(&item_id)],
        ))
        .await
        .expect("query should succeed");

    assert_eq!(updated_item_rows.rows.len(), 1);
    let updated_item = &updated_item_rows.rows[0];
    let remaining = updated_item.get::<i64>("tx_remaining_minor").unwrap_or(0);
    assert_eq!(
        remaining,
        gross_minor - payment_amount,
        "remaining should be decremented by payment"
    );

    // Verify reconciliation shows no AP difference
    let as_of = "2026-10-24";
    let result = reconcile(db, COMPANY_ID, as_of)
        .await
        .expect("reconcile should succeed");

    let ap_recon = result
        .iter()
        .find(|r| r.ledger == "ap")
        .expect("should have AP ledger");
    assert_eq!(
        ap_recon.difference, 0,
        "AP reconciliation difference should be 0"
    );
}

#[test]
fn post_bill_creates_ap_open_item_and_record_payment_allocates() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            test_bill_posting_and_payment_impl(db).await;
        });
    }
}
