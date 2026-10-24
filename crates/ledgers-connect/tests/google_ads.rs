//! The Google Ads connector against recorded answers (issue #142). No test reaches
//! Google: every answer is a fixture, and every request is recorded.

mod support;

use ledgers_connect::connector::{Connector, SyncContext, by_id};
use ledgers_connect::google_ads::{API_VERSION, GoogleAds};
use ledgers_connect::http::VendorError;
use ledgers_connect::model::SyncOutput;
use serde_json::{Value, json};
use std::sync::Arc;
use support::{Recorded, Route, vendor};

const NOW: &str = "2026-10-25T10:00:00Z";
const CUSTOMER: &str = "1234567890";
const MANAGER: &str = "9876543210";
const DEVELOPER_TOKEN: &str = "developer_token_test_not_real";
const CLIENT_SECRET: &str = "client_secret_test_not_real";
const REFRESH: &str = "refresh_token_test_not_real";
const TOKEN_ANSWER: &str = include_str!("../fixtures/google_ads_token.json");
const SEARCH: &str = include_str!("../fixtures/google_ads_search_stream_2026_07_09.json");

fn credential() -> String {
    json!({
        "developer_token": DEVELOPER_TOKEN,
        "client_id": "client-id-test.apps.example",
        "client_secret": CLIENT_SECRET,
        "refresh_token": REFRESH
    })
    .to_string()
}

fn config() -> Value {
    json!({ "customer_id": "123-456-7890", "expense_account": "6200" })
}

fn routes() -> Vec<Route> {
    vec![
        Route {
            url: "oauth2.googleapis.com/token",
            body: "",
            status: 200,
            answer: TOKEN_ANSWER.to_owned(),
        },
        Route {
            url: "googleAds:searchStream",
            body: "",
            status: 200,
            answer: SEARCH.to_owned(),
        },
    ]
}

async fn sync_with(client: Arc<Recorded>, config: &Value) -> Result<SyncOutput, VendorError> {
    let (http, _pause) = vendor(client);
    GoogleAds
        .sync(&SyncContext {
            http: &http,
            credential: &credential(),
            now: NOW,
            config,
            since: "2026-07-01",
            until: "2026-10-25",
        })
        .await
}

#[test]
fn google_ads_is_registered_with_its_fields_and_pinned_version() {
    let connector = by_id("google_ads").expect("registered");
    assert_eq!(
        connector.allowed_hosts(),
        &["googleads.googleapis.com", "oauth2.googleapis.com"]
    );
    let names: Vec<&str> = connector
        .credential_fields()
        .iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(
        names,
        [
            "developer_token",
            "client_id",
            "client_secret",
            "refresh_token"
        ]
    );
    assert_eq!(API_VERSION, "v26");
}

#[tokio::test]
async fn exchanges_the_refresh_token_then_searches_with_the_headers() {
    let client = Recorded::new(routes());
    sync_with(client.clone(), &config()).await.expect("sync");
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests[0].1, "https://oauth2.googleapis.com/token");
    assert_eq!(requests[0].0, "POST");
    let bodies = client.bodies.lock().unwrap();
    assert!(bodies[0].contains("grant_type=refresh_token"));
    assert!(!requests[0].1.contains(REFRESH), "no secret in the URL");
    assert_eq!(
        requests[1].1,
        format!(
            "https://googleads.googleapis.com/{API_VERSION}/customers/{CUSTOMER}/googleAds:searchStream"
        )
    );
    let log = client.header_log.lock().unwrap();
    let header = |log: &Vec<(String, String)>, name: &str| {
        log.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    };
    assert_eq!(
        header(&log[1], "authorization").as_deref(),
        Some("Bearer access_token_test_not_real")
    );
    assert_eq!(
        header(&log[1], "developer-token").as_deref(),
        Some(DEVELOPER_TOKEN)
    );
    assert_eq!(header(&log[1], "login-customer-id"), None);
    assert!(bodies[1].contains("segments.month"));
    assert!(bodies[1].contains("metrics.cost_micros"));
    assert!(bodies[1].contains("BETWEEN '2026-07-01' AND '2026-10-25'"));
}

#[tokio::test]
async fn a_manager_account_sends_login_customer_id() {
    let client = Recorded::new(routes());
    let mut config = config();
    config["login_customer_id"] = json!(MANAGER);
    sync_with(client.clone(), &config).await.expect("sync");
    let log = client.header_log.lock().unwrap();
    let login = log[1]
        .iter()
        .find(|(n, _)| n == "login-customer-id")
        .map(|(_, v)| v.clone());
    assert_eq!(login.as_deref(), Some(MANAGER));
}

#[tokio::test]
async fn cost_micros_are_converted_with_exact_integer_maths() {
    let out = sync_with(Recorded::new(routes()), &config())
        .await
        .expect("sync");
    let july = out
        .invoices
        .iter()
        .find(|i| i.period == "2026-07")
        .expect("July invoice");
    assert_eq!(july.currency, "EUR");
    assert_eq!(july.external_id, "google-ads-cost:2026-07:EUR");
    // 1,234,567 + 2,000,000 micros = 3.234567 EUR = 323 cents (half up).
    assert_eq!(july.total_minor, 323);
    let brand = out
        .usage
        .iter()
        .find(|u| u.period == "2026-07" && u.resource_id == "2001")
        .expect("brand campaign line");
    assert_eq!(brand.quantity, 123, "1,234,567 micros = 1.23 EUR");
    // 7 micros in August is 0.00000 EUR: it rounds to zero and is not booked.
    assert!(out.invoices.iter().all(|i| i.period != "2026-08"));
}

#[tokio::test]
async fn the_current_month_is_skipped_and_a_repeat_import_is_the_same() {
    let answer = r#"[{"results":[{"campaign":{"id":"2001","name":"SQZ"},"customer":{"currencyCode":"EUR"},"segments":{"month":"2026-10-01"},"metrics":{"costMicros":"5000000"}}]}]"#;
    let routes_now = vec![
        Route {
            url: "oauth2.googleapis.com/token",
            body: "",
            status: 200,
            answer: TOKEN_ANSWER.to_owned(),
        },
        Route {
            url: "googleAds:searchStream",
            body: "",
            status: 200,
            answer: answer.to_owned(),
        },
    ];
    let out = sync_with(Recorded::new(routes_now), &config())
        .await
        .expect("sync");
    assert!(out.invoices.is_empty(), "October is still moving");
    let first = sync_with(Recorded::new(routes()), &config())
        .await
        .expect("first");
    let second = sync_with(Recorded::new(routes()), &config())
        .await
        .expect("second");
    assert_eq!(first.invoices, second.invoices);
    assert_eq!(first.usage, second.usage);
}

#[tokio::test]
async fn a_refused_refresh_token_is_named_and_not_retried() {
    for status in [400_u16, 401] {
        let client = Recorded::script(vec![(status, "{}")]);
        let out = sync_with(client.clone(), &config()).await;
        assert!(matches!(out, Err(VendorError::Refused { status: s, .. }) if s == status));
        assert_eq!(client.calls(), 1);
    }
}

#[tokio::test]
async fn a_refused_developer_token_is_named_and_not_retried() {
    let client = Recorded::script(vec![(200, TOKEN_ANSWER), (403, "{}")]);
    let out = sync_with(client.clone(), &config()).await;
    assert!(matches!(out, Err(VendorError::Refused { status: 403, .. })));
    assert_eq!(client.calls(), 2);
}

#[tokio::test]
async fn throttling_is_retried_with_backoff_then_reported() {
    let client = Recorded::script(vec![
        (200, TOKEN_ANSWER),
        (429, "{}"),
        (429, "{}"),
        (429, "{}"),
    ]);
    let (http, pause) = vendor(client.clone());
    let out = GoogleAds
        .sync(&SyncContext {
            http: &http,
            credential: &credential(),
            now: NOW,
            config: &config(),
            since: "2026-07-01",
            until: "2026-10-25",
        })
        .await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(
        client.calls(),
        4,
        "one token call and three search attempts"
    );
    assert_eq!(pause.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn server_errors_are_retried_then_reported() {
    let client = Recorded::script(vec![
        (200, TOKEN_ANSWER),
        (500, "{}"),
        (500, "{}"),
        (503, "{}"),
    ]);
    let out = sync_with(client.clone(), &config()).await;
    assert!(matches!(out, Err(VendorError::Status { status: 503, .. })));
}

#[test]
fn the_customer_id_is_ten_digits_and_checked_before_a_url() {
    assert!(GoogleAds.validate_config(&config()).is_ok());
    assert!(
        GoogleAds
            .validate_config(&json!({"customer_id": "12345", "expense_account": "6200"}))
            .is_err()
    );
    assert!(
        GoogleAds
            .validate_config(&json!({"customer_id": "12345678a0", "expense_account": "6200"}))
            .is_err()
    );
    assert!(GoogleAds.validate_config(&json!({"customer_id": CUSTOMER, "login_customer_id": "x", "expense_account": "6200"})).is_err());
}
