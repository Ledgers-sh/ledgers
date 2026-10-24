//! Polar into the books, end to end over the server's real stack (issue #142, payments):
//! orders and refunds post to the Polar balance; a person enables the suggested payout rule;
//! a bank statement line that pays the balance out is booked through the bank's own posting
//! path; the Polar balance is then at zero. The Polar API is a stub; the ledger, the bank and
//! the rules are the real modules.

mod common;

use bytes::Bytes;
use common::http::{APP_ORIGIN, Auth, Reply, call};
use common::{COMPANY, Fixture, OWNER_SESSION, fixture_with_options, owner_session};
use cratefield_core::axum::body::Body;
use cratefield_core::axum::http::{HeaderValue, Method, Request, StatusCode, header};
use cratefield_core::{HttpClient, HttpError, Statement};
use ledgers_commands::auth::{Principal, Role};
use ledgers_connect::secret::ConnectKey;
use ledgers_server::dispatch::ServerDispatch;
use ledgers_server::routes::{Dispatch, Presented};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const KEY: &str = "0123456789abcdef0123456789abcdef-polar-test-key";
const TOKEN: &str = "polar_oat_POLAR_SERVER_TEST_NEVER_LEAVES_THE_SERVER";
const CLEARING: &str = "1045";
const REVENUE: &str = "4010";
const FEES: &str = "5100";
const BANK: &str = "1020";
const HALDEN: &str = "DE64100200300400500600";

/// One order of 100.00 EUR with a 4.00 platform fee, and a refund of 40.00 EUR.
const ORDERS: &str = r#"{"items":[{"id":"ord_1","total_amount":10000,"tax_amount":0,"platform_fee_amount":400,
 "currency":"eur","created_at":"2026-08-01T10:00:00Z","product_id":"prod_pro"}],
 "pagination":{"total_count":1,"max_page":1}}"#;
const REFUNDS: &str = r#"{"items":[{"id":"ref_1","amount":4000,"tax_amount":0,"currency":"eur",
 "created_at":"2026-08-02T10:00:00Z","order_id":"ord_1"}],"pagination":{"total_count":1,"max_page":1}}"#;

/// The bank statement: the payout of the balance (100.00 - 4.00 - 40.00 = 56.00) on 6 August.
const STATEMENT: &str =
    "Buchungstag;Valuta;Auftraggeber/Empfaenger;Buchungstext;Verwendungszweck;Betrag;Waehrung
06.08.2026;06.08.2026;Polar Payouts;GUTSCHRIFT;POLAR PAYOUT po_test;56,00;EUR
";

fn csv_mapping() -> Value {
    json!({
        "delimiter": ";",
        "skip_rows": 0,
        "has_header": true,
        "decimal": ",",
        "date_format": "DD.MM.YYYY",
        "booking_date": "Buchungstag",
        "value_date": "Valuta",
        "amount": "Betrag",
        "debit": null,
        "credit": null,
        "currency": "Waehrung",
        "counterparty_name": "Auftraggeber/Empfaenger",
        "counterparty_iban": null,
        "remittance": ["Verwendungszweck"],
        "booking_text": "Buchungstext"
    })
}

/// Serves the Polar lists by URL, and checks the token is sent to Polar only.
struct PolarStub;

#[async_trait::async_trait]
impl HttpClient for PolarStub {
    async fn send(
        &self,
        request: http::Request<Bytes>,
    ) -> Result<http::Response<Bytes>, HttpError> {
        let auth = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(auth, format!("Bearer {TOKEN}"));
        let page = if request.uri().path().contains("/refunds/") {
            REFUNDS
        } else {
            ORDERS
        };
        Ok(http::Response::builder()
            .status(200)
            .body(Bytes::from_static(page.as_bytes()))
            .expect("response"))
    }
}

fn person_fx() -> Fixture {
    let fx = fixture_with_options(Vec::new(), None, vec![APP_ORIGIN.to_owned()]);
    owner_session(&fx);
    fx
}

fn owner() -> Presented {
    Presented::Principal(Principal::Person {
        person_id: "person-owner".to_owned(),
        role: Role::Owner,
    })
}

fn person(fx: &Fixture, method: Method, path: &str, body: Option<Value>) -> Reply {
    call(fx, method, path, Auth::Session(OWNER_SESSION), true, body)
}

fn as_key(fx: &Fixture, key: &str, method: Method, path: &str, body: Option<Value>) -> Reply {
    call(fx, method, path, Auth::Bearer(key), false, body)
}

fn upload(fx: &Fixture, key: &str, bytes: &[u8], content_type: &str, name: &str) -> Reply {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/documents/upload")
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_str(content_type).expect("type"),
        )
        .header("x-file-name", name)
        .body(Body::from(bytes.to_vec()))
        .expect("request");
    let response = pollster::block_on(fx.router().clone().oneshot(request)).expect("answers");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = pollster::block_on(cratefield_core::axum::body::to_bytes(
        response.into_body(),
        usize::MAX,
    ))
    .expect("body");
    Reply {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// The unit the connection defaults to (as the connections tests seed it).
fn seed_unit(fx: &Fixture) {
    pollster::block_on(fx.deps.db.execute(&Statement::with_values(
        "INSERT INTO dimension_value (company_id, kind, code, name, own_books, created_at) VALUES (?, 'unit', 'FZ', 'Factory Zero', 1, ?)".to_owned(),
        vec![
            sea_query::Value::from(COMPANY),
            sea_query::Value::from(common::NOW),
        ],
    )))
    .expect("the unit is seeded");
}

fn open_period(fx: &Fixture, period: &str) {
    pollster::block_on(
        fx.deps
            .db
            .execute(&ledgers_db::period::set_status_statement(
                COMPANY, period, "open",
            )),
    )
    .expect("the period opens");
}

/// The balance of an account in minor units: debits positive, credits negative.
fn balance(fx: &Fixture, account: &str) -> i64 {
    let rows = pollster::block_on(fx.deps.db.query(&Statement::with_values(
        "SELECT CAST(COALESCE(SUM(l.base_minor), 0) AS BIGINT) AS total FROM journal_line l JOIN journal_entry e ON e.id = l.entry_id WHERE e.company_id = ? AND l.account_number = ?".to_owned(),
        vec![sea_query::Value::from(COMPANY), sea_query::Value::from(account)],
    )))
    .expect("the balance reads");
    rows.rows
        .first()
        .and_then(|row| row.get::<i64>("total"))
        .unwrap_or(0)
}

#[test]
#[allow(clippy::too_many_lines)]
fn polar_orders_and_refunds_clear_through_the_bank_rule_to_zero() {
    let fx = person_fx();
    seed_unit(&fx);
    open_period(&fx, "2026-08");
    let accounts = ServerDispatch::new(COMPANY);
    pollster::block_on(accounts.dispatch(
        &fx.deps,
        "accounts.create",
        json!({"number": CLEARING, "name": "Polar balance", "type": "asset"}),
        "polar-account",
        Some(owner()),
    ))
    .expect("the Polar balance account is created");
    let mut deps = fx.deps.clone();
    deps.http = Some(Arc::new(PolarStub) as Arc<dyn HttpClient>);
    let dispatch =
        ServerDispatch::new(COMPANY).with_connect_key(Some(ConnectKey::from_secret(KEY).unwrap()));
    let created = pollster::block_on(dispatch.dispatch(
        &deps,
        "connections.create",
        json!({
            "connector": "polar",
            "name": "Polar (test)",
            "config": {"expense_account": FEES, "clearing_account": CLEARING, "revenue_account": REVENUE},
            "default_unit": "FZ",
            "token": TOKEN,
        }),
        "polar-create",
        Some(owner()),
    ))
    .expect("the Polar connection is created");
    let connection = created["id"].as_str().expect("id").to_owned();

    let rules = as_key(&fx, &fx.keys.read, Method::GET, "/v1/bank/rules", None);
    let suggested = rules.json()["rules"]
        .as_array()
        .and_then(|list| {
            list.iter()
                .find(|rule| rule["name"].as_str().unwrap_or("").contains("payouts"))
        })
        .cloned()
        .expect("the Polar payout rule is suggested");
    assert_eq!(suggested["enabled"], false);
    assert_eq!(suggested["account_number"], CLEARING);

    let run = pollster::block_on(dispatch.dispatch(
        &deps,
        "connections.sync_now",
        json!({"connection_id": connection, "since": "2026-08-01", "until": "2026-08-31"}),
        "polar-sync",
        Some(owner()),
    ))
    .expect("the sync runs");
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(
        balance(&fx, CLEARING),
        5600,
        "the order nets 96.00 and the refund takes 40.00 back"
    );

    let enabled = person(
        &fx,
        Method::POST,
        &format!(
            "/v1/bank/rules/{}/enable",
            suggested["id"].as_str().unwrap()
        ),
        Some(json!({})),
    );
    assert_eq!(enabled.status, StatusCode::OK, "{}", enabled.text);

    let account = person(
        &fx,
        Method::POST,
        "/v1/bank/accounts",
        Some(json!({
            "name": "Halden Works Girokonto",
            "iban": HALDEN,
            "bic": "HALDDEFFXXX",
            "currency": "EUR",
            "ledger_account_number": BANK,
            "opening_amount": "12500.00",
            "opening_date": "2026-08-01",
            "csv_mapping": csv_mapping(),
        })),
    );
    assert_eq!(account.status, StatusCode::OK, "{}", account.text);
    let account_id = account.json()["id"].as_str().unwrap().to_owned();
    let stored = upload(
        &fx,
        &fx.keys.draft,
        STATEMENT.as_bytes(),
        "text/csv",
        "august-polar.csv",
    );
    assert_eq!(stored.status, StatusCode::CREATED, "{}", stored.text);
    let document = stored.json()["document"]["id"].as_str().unwrap().to_owned();
    let imported = as_key(
        &fx,
        &fx.keys.draft,
        Method::POST,
        "/v1/bank/statements/import",
        Some(json!({"account_id": account_id, "document_id": document, "format": "csv"})),
    );
    assert_eq!(imported.status, StatusCode::OK, "{}", imported.text);

    let lines = as_key(
        &fx,
        &fx.keys.read,
        Method::GET,
        &format!("/v1/bank/transactions?account_id={account_id}&limit=50"),
        None,
    );
    let line = lines.json()["transactions"]
        .as_array()
        .and_then(|list| list.first())
        .cloned()
        .expect("the payout line");
    assert_eq!(line["amount"]["minor"], "5600", "{line}");
    let suggests_clearing = line["suggestions"].as_array().is_some_and(|list| {
        list.iter()
            .any(|s| s["kind"] == "rule" && s["account_number"] == CLEARING)
    });
    assert!(
        suggests_clearing,
        "the rule suggests the Polar balance: {line}"
    );

    let posted = person(
        &fx,
        Method::POST,
        &format!(
            "/v1/bank/transactions/{}/post",
            line["id"].as_str().unwrap()
        ),
        Some(json!({"counterparty_account": suggested["account_number"]})),
    );
    assert_eq!(posted.status, StatusCode::OK, "{}", posted.text);
    assert_eq!(
        balance(&fx, CLEARING),
        0,
        "orders, refund and payout leave the Polar balance at zero"
    );
    assert_eq!(balance(&fx, BANK), 5600);
}
