//! The Twilio connector against recorded answers (issue #142). No test reaches
//! Twilio. Each answer is a fixture or an inline script, and every request and
//! header is recorded.

mod support;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ledgers_connect::connector::{Connector, SyncContext};
use ledgers_connect::http::VendorError;
use ledgers_connect::model::SyncOutput;
use ledgers_connect::twilio::Twilio;
use serde_json::{Value, json};
use std::sync::Arc;
use support::{Recorded, Route, vendor};

const NOW: &str = "2026-10-25T10:00:00Z";
const ACCOUNT: &str = concat!("AC", "0123456789abcdef", "0123456789abcdef");
const SUB: &str = concat!("AC", "89abcdef01234567", "89abcdef01234567");
const PAGE1: &str = include_str!("../fixtures/twilio_usage_monthly_page1.json");
const PAGE2: &str = include_str!("../fixtures/twilio_usage_monthly_page2.json");
const BALANCE: &str = include_str!("../fixtures/twilio_balance.json");
const KEY_SID: &str = concat!("SK", "0123456789abcdef", "0123456789abcdef");
const KEY_SECRET: &str = "twilio_secret_test_not_real";

fn credential() -> String {
    json!({"api_key_sid": KEY_SID, "api_key_secret": KEY_SECRET}).to_string()
}

fn config() -> Value {
    json!({"account_sid": ACCOUNT, "expense_account": "6100"})
}

async fn sync_with(
    client: Arc<Recorded>,
    credential: &str,
    config: &Value,
    since: &str,
    until: &str,
) -> Result<SyncOutput, VendorError> {
    let (http, _pause) = vendor(client);
    Twilio
        .sync(&SyncContext {
            http: &http,
            credential,
            now: NOW,
            config,
            since,
            until,
        })
        .await
}

fn routes() -> Vec<Route> {
    vec![
        Route {
            url: "Usage/Records/Monthly.json?StartDate",
            body: "",
            status: 200,
            answer: PAGE1.to_owned(),
        },
        Route {
            url: "PageToken=PA-test-token",
            body: "",
            status: 200,
            answer: PAGE2.to_owned(),
        },
        Route {
            url: "/Balance.json",
            body: "",
            status: 200,
            answer: BALANCE.to_owned(),
        },
    ]
}

fn header(log: &[(String, String)], name: &str) -> Option<String> {
    log.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
}

#[tokio::test]
async fn sends_basic_auth_with_the_key_sid_and_secret_and_the_monthly_query() {
    let client = Recorded::new(routes());
    sync_with(
        client.clone(),
        &credential(),
        &config(),
        "2026-08-01",
        "2026-10-25",
    )
    .await
    .expect("the sync answers");
    let log = client.header_log.lock().unwrap();
    let expected = format!(
        "Basic {}",
        STANDARD.encode(format!("{KEY_SID}:{KEY_SECRET}"))
    );
    assert_eq!(
        header(&log[0], "authorization").as_deref(),
        Some(expected.as_str())
    );
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests[0].0, "GET");
    assert_eq!(
        requests[0].1,
        format!(
            "https://api.twilio.com/2010-04-01/Accounts/{ACCOUNT}/Usage/Records/Monthly.json?StartDate=2026-08-01&EndDate=2026-10-25&PageSize=1000&IncludeSubaccounts=false"
        )
    );
}

#[tokio::test]
async fn follows_next_page_uri_and_books_one_invoice_per_month_and_currency() {
    let client = Recorded::new(routes());
    let out = sync_with(
        client.clone(),
        &credential(),
        &config(),
        "2026-09-01",
        "2026-10-25",
    )
    .await
    .expect("both pages read");
    assert_eq!(client.calls(), 3, "two usage pages and the balance");
    assert_eq!(out.invoices.len(), 1);
    let invoice = &out.invoices[0];
    assert_eq!(invoice.external_id, "twilio-cost:2026-09:USD");
    // 12.04 + 2.00 + 3.10 = 17.14 USD; the totalprice row is skipped, not added twice.
    assert_eq!(invoice.total_minor, 1714);
    assert!(
        invoice.description.contains("sms-outbound 1204"),
        "{}",
        invoice.description
    );
    assert!(
        invoice.description.contains("phonenumbers 200"),
        "{}",
        invoice.description
    );
    assert!(
        invoice.description.contains("calls 310"),
        "{}",
        invoice.description
    );
    assert_eq!(out.usage.len(), 1, "one account resource in the month");
    assert_eq!(out.usage[0].quantity, 1714);
    assert_eq!(out.usage[0].resource_id, ACCOUNT);
}

#[tokio::test]
async fn a_next_page_uri_that_leaves_twilio_is_refused() {
    let client = Recorded::script(vec![(
        200,
        r#"{"usage_records": [], "next_page_uri": "https://evil.example/steal"}"#,
    )]);
    let out = sync_with(client, &credential(), &config(), "2026-09-01", "2026-09-30").await;
    assert!(matches!(out, Err(VendorError::Malformed { .. })), "{out:?}");
}

#[tokio::test]
async fn the_current_month_is_not_booked_and_a_repeat_sync_is_the_same() {
    let client = Recorded::new(routes());
    let first = sync_with(
        client.clone(),
        &credential(),
        &config(),
        "2026-08-01",
        "2026-10-25",
    )
    .await
    .expect("first sync");
    assert!(
        first.invoices.iter().all(|i| i.period != "2026-10"),
        "October is still moving and is skipped"
    );
    let second = sync_with(
        Recorded::new(routes()),
        &credential(),
        &config(),
        "2026-08-01",
        "2026-10-25",
    )
    .await
    .expect("second sync");
    assert_eq!(
        first.invoices, second.invoices,
        "re-import gives the same ids and totals"
    );
    assert_eq!(first.usage, second.usage);
}

#[tokio::test]
async fn a_subaccount_connection_reads_its_own_scope_and_the_parent_balance() {
    let client = Recorded::new(routes());
    let config = json!({"account_sid": ACCOUNT, "subaccount_sid": SUB, "expense_account": "6100"});
    let out = sync_with(
        client.clone(),
        &credential(),
        &config,
        "2026-09-01",
        "2026-10-25",
    )
    .await
    .expect("sub sync");
    let requests = client.requests.lock().unwrap();
    assert!(
        requests[0].1.contains(&format!("/Accounts/{SUB}/Usage/")),
        "{}",
        requests[0].1
    );
    assert!(
        requests
            .iter()
            .any(|r| r.1.ends_with(&format!("/Accounts/{ACCOUNT}/Balance.json"))),
        "the balance is read from the parent, which holds it"
    );
    assert_eq!(out.usage[0].resource_id, SUB);
}

#[tokio::test]
async fn the_balance_is_a_note_never_a_bill() {
    let out = sync_with(
        Recorded::new(routes()),
        &credential(),
        &config(),
        "2026-09-01",
        "2026-10-25",
    )
    .await
    .expect("sync");
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("42.17 USD") && n.contains("not booked")),
        "{:?}",
        out.notes
    );
    assert_eq!(out.invoices.len(), 1, "the balance adds no invoice");
}

#[tokio::test]
async fn a_refused_balance_does_not_fail_the_sync() {
    let client = Recorded::script(vec![(200, PAGE2), (403, "{}")]);
    let out = sync_with(client, &credential(), &config(), "2026-09-01", "2026-09-30").await;
    let out = out.expect("the usage still books");
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("account balance not read")),
        "{:?}",
        out.notes
    );
}

#[tokio::test]
async fn refusals_are_named_and_not_retried() {
    let client = Recorded::script(vec![(401, "{}")]);
    let out = sync_with(
        client.clone(),
        &credential(),
        &config(),
        "2026-09-01",
        "2026-09-30",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 401, .. })));
    assert_eq!(client.calls(), 1);

    let client = Recorded::script(vec![(403, "{}")]);
    let out = sync_with(
        client.clone(),
        &credential(),
        &config(),
        "2026-09-01",
        "2026-09-30",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 403, .. })));
    assert_eq!(client.calls(), 1);
}

#[tokio::test]
async fn throttling_and_server_errors_are_retried_three_times_then_reported() {
    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let out = sync_with(
        client.clone(),
        &credential(),
        &config(),
        "2026-09-01",
        "2026-09-30",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(client.calls(), 3);

    let client = Recorded::script(vec![(503, "{}"), (503, "{}"), (503, "{}")]);
    let out = sync_with(
        client.clone(),
        &credential(),
        &config(),
        "2026-09-01",
        "2026-09-30",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 503, .. })));
    assert_eq!(client.calls(), 3);
}

#[tokio::test]
async fn a_missing_credential_field_is_named_and_no_request_is_sent() {
    let client = Recorded::new(routes());
    let only_sid = json!({"api_key_sid": KEY_SID}).to_string();
    let out = sync_with(
        client.clone(),
        &only_sid,
        &config(),
        "2026-09-01",
        "2026-09-30",
    )
    .await;
    match out {
        Err(VendorError::Malformed { detail, .. }) => {
            assert!(detail.contains("api_key_secret"), "{detail}");
        }
        other => panic!("expected the field to be named, got {other:?}"),
    }
    assert_eq!(client.calls(), 0);
}

#[test]
fn account_sids_are_checked_before_they_are_used_in_a_url() {
    assert!(Twilio.validate_config(&config()).is_ok());
    assert!(
        Twilio
            .validate_config(&json!({"account_sid": "AC../../x"}))
            .is_err()
    );
    assert!(
        Twilio
            .validate_config(&json!({"account_sid": "XX0123456789abcdef0123456789abcdef"}))
            .is_err()
    );
    assert!(
        Twilio
            .validate_config(&json!({"account_sid": ACCOUNT, "subaccount_sid": "nope"}))
            .is_err()
    );
    assert!(Twilio.validate_config(&json!({})).is_err());
    assert_eq!(Twilio.allowed_hosts(), &["api.twilio.com"]);
}
