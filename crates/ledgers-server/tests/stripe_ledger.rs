//! Stripe into the books, end to end over the server's real stack (issue #142,
//! payments): a sync posts the charge and the refund as balanced journal entries,
//! the suggested payout rule is enabled by a person, a bank statement line that
//! matches it is booked through the bank's own posting path, and the Stripe clearing
//! account is then at zero. The Stripe API is a stub; the ledger, the bank and the
//! rules are the real modules.

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
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const KEY: &str = "0123456789abcdef0123456789abcdef-stripe-test-key";
const RESTRICTED_KEY: &str = "rk_test_STRIPE_SERVER_TEST_NEVER_LEAVES_THE_SERVER";
const CLEARING: &str = "1035";
const REVENUE: &str = "4010";
const FEES: &str = "5100";
const SUSPENSE: &str = "2040";
const BANK: &str = "1020";
const HALDEN: &str = "DE64100200300400500600";

/// Charge 100.00 EUR (fee 3.20), refund 40.00 EUR, and a payout of the net 56.80 EUR.
const PAGE: &str = r#"{"object":"list","has_more":false,"data":[
 {"id":"txn_charge_1","object":"balance_transaction","amount":10000,"fee":320,"net":9680,"currency":"eur",
  "type":"charge","reporting_category":"charge","created":1785542400,
  "source":{"object":"charge","id":"ch_1","amount":11000,"currency":"usd","metadata":{"product":"ledgers-pro"}}},
 {"id":"txn_refund_1","object":"balance_transaction","amount":-4000,"fee":0,"net":-4000,"currency":"eur",
  "type":"refund","reporting_category":"refund","created":1785628800,
  "source":{"object":"refund","id":"re_1","amount":4400,"currency":"usd","metadata":{}}},
 {"id":"txn_payout_1","object":"balance_transaction","amount":-5680,"fee":0,"net":-5680,"currency":"eur",
  "type":"payout","reporting_category":"payout","created":1785715200,"source":{"object":"payout","id":"po_1"}}
]}"#;

/// The bank statement: the payout arrives on 3 August 2026.
const STATEMENT: &str =
    "Buchungstag;Valuta;Auftraggeber/Empfaenger;Buchungstext;Verwendungszweck;Betrag;Waehrung
03.08.2026;03.08.2026;Stripe Payments Europe Ltd;GUTSCHRIFT;STRIPE PAYOUT po_1;56,80;EUR
";

/// The csv mapping of the statement above.
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

/// Serves the Stripe page for any balance-transaction request, and counts them.
struct StripeStub {
    requests: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl HttpClient for StripeStub {
    async fn send(
        &self,
        request: http::Request<Bytes>,
    ) -> Result<http::Response<Bytes>, HttpError> {
        self.requests
            .lock()
            .unwrap()
            .push(request.uri().to_string());
        let auth = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            auth,
            format!("Bearer {RESTRICTED_KEY}"),
            "the restricted key is sent to Stripe only"
        );
        Ok(http::Response::builder()
            .status(200)
            .body(Bytes::from_static(PAGE.as_bytes()))
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

fn count_stripe_entries(fx: &Fixture) -> i64 {
    let rows = pollster::block_on(fx.deps.db.query(&Statement::with_values(
        "SELECT CAST(COUNT(*) AS BIGINT) AS n FROM journal_entry WHERE company_id = ? AND external_id LIKE 'stripe-bt:%'".to_owned(),
        vec![sea_query::Value::from(COMPANY)],
    )))
    .expect("the count reads");
    rows.rows
        .first()
        .and_then(|row| row.get::<i64>("n"))
        .unwrap_or(0)
}

#[test]
#[allow(clippy::too_many_lines)]
fn a_stripe_charge_then_refund_then_payout_leaves_the_clearing_account_at_zero() {
    let fx = person_fx();
    seed_unit(&fx);
    open_period(&fx, "2026-08");
    // The clearing account is the company's own asset: "Stripe balance".
    let plain_deps = fx.deps.clone();
    let accounts = ServerDispatch::new(COMPANY);
    pollster::block_on(accounts.dispatch(
        &plain_deps,
        "accounts.create",
        json!({"number": CLEARING, "name": "Stripe balance", "type": "asset"}),
        "stripe-clearing-account",
        Some(owner()),
    ))
    .expect("the clearing account is created");
    let stripe = Arc::new(StripeStub {
        requests: Mutex::new(Vec::new()),
    });
    let mut deps = fx.deps.clone();
    deps.http = Some(stripe.clone() as Arc<dyn HttpClient>);
    let dispatch =
        ServerDispatch::new(COMPANY).with_connect_key(Some(ConnectKey::from_secret(KEY).unwrap()));

    let created = pollster::block_on(dispatch.dispatch(
        &deps,
        "connections.create",
        json!({
            "connector": "stripe",
            "name": "Stripe (test)",
            "config": {
                "expense_account": FEES,
                "clearing_account": CLEARING,
                "revenue_account": REVENUE,
                "suspense_account": SUSPENSE,
            },
            "default_unit": "FZ",
            "token": RESTRICTED_KEY,
        }),
        "stripe-create",
        Some(owner()),
    ))
    .expect("the Stripe connection is created");
    let connection = created["id"].as_str().expect("id").to_owned();

    // The payout rule is suggested at creation and waits for a person.
    let rules = as_key(&fx, &fx.keys.read, Method::GET, "/v1/bank/rules", None);
    assert_eq!(rules.status, StatusCode::OK, "{}", rules.text);
    let suggested = rules.json()["rules"]
        .as_array()
        .and_then(|list| {
            list.iter()
                .find(|rule| rule["name"].as_str().unwrap_or("").contains("payouts"))
        })
        .cloned()
        .expect("the Stripe payout rule is suggested");
    assert_eq!(
        suggested["enabled"], false,
        "a suggestion waits for a person"
    );
    assert_eq!(
        suggested["account_number"], CLEARING,
        "it books to the clearing account"
    );

    // The sync posts the charge and the refund (the payout is left to the bank rule).
    let run = pollster::block_on(dispatch.dispatch(
        &deps,
        "connections.sync_now",
        json!({"connection_id": connection, "since": "2026-08-01", "until": "2026-08-31"}),
        "stripe-sync-1",
        Some(owner()),
    ))
    .expect("the sync runs");
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(
        count_stripe_entries(&fx),
        2,
        "the charge and the refund are posted"
    );
    assert_eq!(
        balance(&fx, CLEARING),
        5680,
        "9680 from the charge less 4000 from the refund"
    );
    assert!(stripe.requests.lock().unwrap()[0].contains("expand[]=data.source"));

    // A repeat sync posts nothing again.
    let again = pollster::block_on(dispatch.dispatch(
        &deps,
        "connections.sync_now",
        json!({"connection_id": connection, "since": "2026-08-01", "until": "2026-08-31"}),
        "stripe-sync-2",
        Some(owner()),
    ))
    .expect("the repeat runs");
    assert_eq!(again["status"], "succeeded");
    assert_eq!(
        count_stripe_entries(&fx),
        2,
        "a repeat sync creates nothing"
    );
    assert_eq!(balance(&fx, CLEARING), 5680);

    // A person enables the suggested rule.
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

    // The payout arrives at the bank: import the statement, then match the line with the rule.
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
    let account_id = account.json()["id"]
        .as_str()
        .expect("account id")
        .to_owned();
    let stored = upload(
        &fx,
        &fx.keys.draft,
        STATEMENT.as_bytes(),
        "text/csv",
        "august-stripe.csv",
    );
    assert_eq!(stored.status, StatusCode::CREATED, "{}", stored.text);
    let document = stored.json()["document"]["id"]
        .as_str()
        .expect("document")
        .to_owned();
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
    assert_eq!(lines.status, StatusCode::OK, "{}", lines.text);
    let line = lines.json()["transactions"]
        .as_array()
        .and_then(|list| list.first())
        .cloned()
        .expect("the payout line");
    assert_eq!(line["amount"]["minor"], "5680", "{line}");
    // The bank's own suggestion engine already matches the line to the enabled rule.
    let suggests_clearing = line["suggestions"].as_array().is_some_and(|list| {
        list.iter()
            .any(|s| s["kind"] == "rule" && s["account_number"] == CLEARING)
    });
    assert!(
        suggests_clearing,
        "the rule suggests the clearing account: {line}"
    );

    // The rule's own conditions match the line: the real bank matcher, not a copy.
    let matched = ledgers_bank::rules::RuleConditions {
        contains: suggested["conditions"]["contains"]
            .as_str()
            .map(str::to_owned),
        counterparty_iban: None,
        amount_min: suggested["conditions"]["amount_min"].as_i64(),
        amount_max: None,
    }
    .matches(&ledgers_bank::rules::LineFacts {
        minor: 5680,
        counterparty_name: line["counterparty_name"].as_str(),
        counterparty_iban: None,
        remittance: line["remittance"].as_str().unwrap_or(""),
    });
    assert!(matched, "the rule matches the payout line");

    // Book the line against the rule's account, through the bank's posting path.
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
        "charge, refund and payout leave the clearing account at zero"
    );
    assert_eq!(
        balance(&fx, BANK),
        5680,
        "the bank ledger account received the payout"
    );
}

#[test]
fn a_stripe_connection_must_name_postable_accounts_of_the_chart() {
    let fx = person_fx();
    seed_unit(&fx);
    let accounts = ServerDispatch::new(COMPANY);
    pollster::block_on(accounts.dispatch(
        &fx.deps,
        "accounts.create",
        json!({"number": CLEARING, "name": "Stripe balance", "type": "asset"}),
        "stripe-check-clearing",
        Some(owner()),
    ))
    .expect("the clearing account is created");
    let dispatch =
        ServerDispatch::new(COMPANY).with_connect_key(Some(ConnectKey::from_secret(KEY).unwrap()));
    let attempt = |clearing: &str, revenue: &str| {
        pollster::block_on(dispatch.dispatch(
            &fx.deps,
            "connections.create",
            json!({
                "connector": "stripe",
                "name": "Stripe checks",
                "config": {
                    "expense_account": FEES,
                    "clearing_account": clearing,
                    "revenue_account": revenue,
                    "suspense_account": SUSPENSE,
                },
                "default_unit": "FZ",
                "token": RESTRICTED_KEY,
            }),
            "stripe-check",
            Some(owner()),
        ))
    };
    let missing = attempt("9999", REVENUE).expect_err("a missing account is refused");
    assert!(
        missing
            .detail
            .unwrap_or_default()
            .contains("clearing_account: account 9999 does not exist")
    );
    // 4000 is the income heading of the chart: postings go to its leaves.
    let heading = attempt(CLEARING, "4000").expect_err("a heading is refused");
    let detail = heading.detail.unwrap_or_default();
    assert!(
        detail.contains("revenue_account") && detail.contains("4000") && detail.contains("heading"),
        "{detail}"
    );
}
