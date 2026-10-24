//! Customer receipts on a bank line: a line to the receivables account settles
//! the customer's open items through the subledger, oldest first, and any excess
//! is the customer's on-account item. The March statement's credits are the
//! receipts; the invoices are posted through the subledger executor.

mod common;

use common::http::{APP_ORIGIN, Auth, Reply, call};
use common::{COMPANY, Fixture, OWNER_SESSION, fixture_with_options, owner_session};
use cratefield_core::axum::body::Body;
use cratefield_core::axum::http::{HeaderValue, Method, Request, StatusCode, header};
use cratefield_core::{Database, Statement};
use ledgers_commands::auth::{Authenticator, Credential, Principal, Role};
use ledgers_commands::{CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor};
use ledgers_subledger::commands::{PostItemInput, SubledgerPostItem};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tower::ServiceExt;

const HALDEN: &str = "DE64100200300400500600";
const CAMT_MARCH: &[u8] =
    include_bytes!("../../ledgers-bank/tests/fixtures/camt053_bank_a_march_full.xml");
const NOW: &str = "2026-10-24T09:00:00Z";

struct Owner;

#[async_trait::async_trait]
impl Authenticator for Owner {
    async fn authenticate(&self, _credential: &Credential) -> Result<Principal, CommandError> {
        Ok(Principal::Person {
            person_id: "owner".to_string(),
            role: Role::Owner,
        })
    }
}

fn person_fx() -> Fixture {
    let fx = fixture_with_options(Vec::new(), None, vec![APP_ORIGIN.to_owned()]);
    owner_session(&fx);
    fx
}

fn person(fx: &Fixture, method: Method, path: &str, body: Option<Value>) -> Reply {
    call(fx, method, path, Auth::Session(OWNER_SESSION), true, body)
}

fn upload(fx: &Fixture, bytes: &[u8]) -> String {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/documents/upload")
        .header(header::AUTHORIZATION, format!("Bearer {}", fx.keys.draft))
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/xml"),
        )
        .header("x-file-name", "march.xml")
        .body(Body::from(bytes.to_vec()))
        .expect("request");
    let response = pollster::block_on(fx.router().clone().oneshot(request)).expect("answers");
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = pollster::block_on(cratefield_core::axum::body::to_bytes(
        response.into_body(),
        usize::MAX,
    ))
    .expect("body");
    let stored: Value = serde_json::from_slice(&bytes).expect("json");
    stored["document"]["id"]
        .as_str()
        .expect("document id")
        .to_owned()
}

/// The March account, with the March statement imported. Returns the bank
/// account id and the statement's lines by amount.
fn bank_with_march(fx: &Fixture) -> String {
    let account = person(
        fx,
        Method::POST,
        "/v1/bank/accounts",
        Some(json!({
            "name": "Halden Works Girokonto",
            "iban": HALDEN,
            "bic": "HALDDEFFXXX",
            "currency": "EUR",
            "ledger_account_number": "1020",
            "opening_amount": "12500.00",
            "opening_date": "2026-03-01",
            "csv_mapping": null,
        })),
    );
    assert_eq!(account.status, StatusCode::OK, "{}", account.text);
    let account_id = account.json()["id"]
        .as_str()
        .expect("account id")
        .to_owned();
    let document = upload(fx, CAMT_MARCH);
    let imported = call(
        fx,
        Method::POST,
        "/v1/bank/statements/import",
        Auth::Bearer(&fx.keys.draft),
        false,
        Some(json!({"account_id": account_id, "document_id": document})),
    );
    assert_eq!(imported.status, StatusCode::OK, "{}", imported.text);
    account_id
}

/// The id of the unmatched March line of `minor` (signed, minor units).
fn line_of(fx: &Fixture, minor: &str) -> String {
    let lines = person(
        fx,
        Method::GET,
        "/v1/bank/transactions?status=unmatched&limit=200",
        None,
    );
    assert_eq!(lines.status, StatusCode::OK, "{}", lines.text);
    lines.json()["transactions"]
        .as_array()
        .expect("lines")
        .iter()
        .find(|line| line["amount"]["minor"] == minor)
        .unwrap_or_else(|| panic!("the {minor} line"))["id"]
        .as_str()
        .expect("line id")
        .to_owned()
}

fn journal_id(db: &dyn Database) -> String {
    let rows = pollster::block_on(db.query(&Statement::with_values(
        "SELECT id FROM journal WHERE company_id = ? AND code = 'GN'".to_owned(),
        vec![sea_query::Value::from(COMPANY)],
    )))
    .expect("journal");
    rows.rows[0].get::<String>("id").expect("journal id")
}

/// Posts an AR invoice for `party` of `minor` cents, through the subledger.
fn invoice(fx: &Fixture, party: &str, minor: i64, source: &str) -> String {
    let db = fx.deps.db.as_ref();
    let input = PostItemInput {
        ledger: "ar".to_owned(),
        kind: "invoice".to_owned(),
        party_id: party.to_owned(),
        journal_id: journal_id(db),
        document_date: "2026-03-01".to_owned(),
        posting_date: "2026-03-01".to_owned(),
        due_date: None,
        currency: "EUR".to_owned(),
        amount: minor,
        rate: None,
        counter_account: "4010".to_owned(),
        dimensions: BTreeMap::new(),
        counter_lines: Vec::new(),
        description: format!("Invoice {source}"),
        source_id: Some(source.to_owned()),
    };
    let ctx = CommandContext {
        company_id: COMPANY.to_owned(),
        is_person: true,
        event_id: format!("inv-{source}"),
        request_id: format!("req-inv-{source}"),
        now: NOW.to_owned(),
    };
    let executor = Executor::new(db, Owner, DefaultPolicy);
    match pollster::block_on(executor.run(
        &SubledgerPostItem,
        "bearer".to_owned(),
        ctx,
        &input,
        Execute::Run,
    ))
    .expect("the invoice posts")
    {
        Executed::Done(output) => output.item_id,
        _ => panic!("expected the invoice to post"),
    }
}

fn remaining(db: &dyn Database, item: &str) -> i64 {
    let rows = pollster::block_on(db.query(&Statement::with_values(
        "SELECT tx_remaining_minor FROM open_item WHERE id = ?".to_owned(),
        vec![sea_query::Value::from(item)],
    )))
    .expect("item");
    rows.rows[0]
        .get::<i64>("tx_remaining_minor")
        .expect("remaining")
}

fn receipt(fx: &Fixture, line: &str, body: Value) -> Reply {
    person(
        fx,
        Method::POST,
        &format!("/v1/bank/transactions/{line}/post"),
        Some(body),
    )
}

#[test]
fn a_receipt_clears_the_one_invoice_it_pays() {
    let fx = person_fx();
    bank_with_march(&fx);
    let db = fx.deps.db.as_ref();
    let invoice_id = invoice(&fx, "nordlicht", 420_000, "RE-NL-1");
    let line = line_of(&fx, "420000");

    let posted = receipt(
        &fx,
        &line,
        json!({"id": line, "counterparty_account": "1030", "party_id": "nordlicht"}),
    );
    assert_eq!(posted.status, StatusCode::OK, "{}", posted.text);
    assert_eq!(posted.json()["status"], "matched", "{}", posted.text);
    assert_eq!(remaining(db, &invoice_id), 0, "the invoice is settled");

    let rows = pollster::block_on(ledgers_subledger::reconcile::reconcile(
        db,
        COMPANY,
        "2026-12-31",
    ))
    .expect("reconcile");
    let ar = rows.iter().find(|row| row.ledger == "ar").expect("ar");
    assert_eq!(ar.difference, 0, "{ar:?}");
}

#[test]
fn a_receipt_across_two_invoices_settles_the_older_and_part_pays_the_newer() {
    let fx = person_fx();
    bank_with_march(&fx);
    let db = fx.deps.db.as_ref();
    let older = invoice(&fx, "acme", 100_000, "RE-AC-1");
    let newer = invoice(&fx, "acme", 100_000, "RE-AC-2");

    // The customer's open items, oldest first.
    let open = person(
        &fx,
        Method::GET,
        "/v1/bank/receivables/open-items?party_id=acme",
        None,
    );
    assert_eq!(open.status, StatusCode::OK, "{}", open.text);
    let body = open.json();
    let ids: Vec<&str> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec![older.as_str(), newer.as_str()]);

    // 1500.00 from Acme: the older invoice (1000.00) and 500.00 of the newer.
    let line = line_of(&fx, "150000");
    let posted = receipt(
        &fx,
        &line,
        json!({"id": line, "counterparty_account": "1030", "party_id": "acme"}),
    );
    assert_eq!(posted.status, StatusCode::OK, "{}", posted.text);
    assert_eq!(remaining(db, &older), 0);
    assert_eq!(
        remaining(db, &newer),
        50_000,
        "500.00 of the newer invoice is left"
    );
}

#[test]
fn an_overpayment_settles_the_invoice_and_the_excess_is_on_account() {
    let fx = person_fx();
    bank_with_march(&fx);
    let db = fx.deps.db.as_ref();
    let invoice_id = invoice(&fx, "nordlicht", 100_000, "RE-NL-OP");
    let line = line_of(&fx, "420000");

    let posted = receipt(
        &fx,
        &line,
        json!({"id": line, "counterparty_account": "1030", "party_id": "nordlicht"}),
    );
    assert_eq!(posted.status, StatusCode::OK, "{}", posted.text);
    assert_eq!(remaining(db, &invoice_id), 0);

    // The 3200.00 excess is the customer's on-account item, and the control still reconciles.
    let rows = pollster::block_on(db.query(&Statement::with_values(
        "SELECT tx_remaining_minor FROM open_item WHERE company_id = ? AND party_id = ? AND kind = 'on-account'".to_owned(),
        vec![sea_query::Value::from(COMPANY), sea_query::Value::from("nordlicht")],
    )))
    .expect("on-account");
    assert_eq!(rows.rows.len(), 1, "one on-account item");
    assert_eq!(
        rows.rows[0].get::<i64>("tx_remaining_minor"),
        Some(-320_000)
    );
    let recon = pollster::block_on(ledgers_subledger::reconcile::reconcile(
        db,
        COMPANY,
        "2026-12-31",
    ))
    .expect("reconcile");
    let ar = recon.iter().find(|row| row.ledger == "ar").expect("ar");
    assert_eq!(ar.difference, 0, "{ar:?}");
}

#[test]
fn a_receivables_line_without_a_customer_is_refused_and_nothing_is_settled() {
    let fx = person_fx();
    bank_with_march(&fx);
    let db = fx.deps.db.as_ref();
    let invoice_id = invoice(&fx, "nordlicht", 420_000, "RE-NL-REFUSED");
    let line = line_of(&fx, "420000");

    let refused = receipt(
        &fx,
        &line,
        json!({"id": line, "counterparty_account": "1030"}),
    );
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert_eq!(
        remaining(db, &invoice_id),
        420_000,
        "the invoice is still open"
    );
    let still = person(
        &fx,
        Method::GET,
        "/v1/bank/transactions?status=unmatched&limit=200",
        None,
    );
    assert!(still.text.contains(&line), "the line stays unmatched");
}
