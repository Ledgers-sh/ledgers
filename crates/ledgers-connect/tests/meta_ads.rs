//! The Meta Ads connector against recorded answers (issue #142). No test reaches
//! Meta: every answer is a fixture, and every request is recorded.

mod support;

use ledgers_connect::connector::{Connector, SyncContext, by_id};
use ledgers_connect::http::VendorError;
use ledgers_connect::meta_ads::{GRAPH_VERSION, MetaAds};
use ledgers_connect::model::SyncOutput;
use serde_json::{Value, json};
use std::sync::Arc;
use support::{Recorded, Route, vendor};

const NOW: &str = "2026-10-25T10:00:00Z";
const ACCOUNT_ID: &str = "act_1234567890";
const TOKEN: &str = "meta_system_user_token_test_not_real";
const PAGE1: &str = include_str!("../fixtures/meta_ads_insights_page1.json");
const PAGE2: &str = include_str!("../fixtures/meta_ads_insights_page2.json");

fn credential() -> String {
    json!({ "access_token": TOKEN }).to_string()
}

fn config() -> Value {
    json!({ "ad_account_id": ACCOUNT_ID, "expense_account": "6200", "name_prefixes": "SQZ:SQZ" })
}

fn routes() -> Vec<Route> {
    vec![
        Route {
            url: "/insights?level=campaign",
            body: "",
            status: 200,
            answer: PAGE1.to_owned(),
        },
        Route {
            url: "page=2",
            body: "",
            status: 200,
            answer: PAGE2.to_owned(),
        },
    ]
}

async fn sync_with(
    client: Arc<Recorded>,
    credential: &str,
    config: &Value,
) -> Result<SyncOutput, VendorError> {
    let (http, _pause) = vendor(client);
    MetaAds
        .sync(&SyncContext {
            http: &http,
            credential,
            now: NOW,
            config,
            since: "2026-07-01",
            until: "2026-10-25",
        })
        .await
}

#[test]
fn meta_is_registered_with_its_fields_and_pinned_version() {
    let connector = by_id("meta_ads").expect("registered");
    assert_eq!(connector.allowed_hosts(), &["graph.facebook.com"]);
    assert_eq!(connector.credential_fields().len(), 1);
    assert_eq!(connector.credential_fields()[0].name, "access_token");
    assert_eq!(GRAPH_VERSION, "v26.0");
}

#[tokio::test]
async fn reads_the_pinned_insights_edge_with_a_bearer_token_and_pages() {
    let client = Recorded::new(routes());
    sync_with(client.clone(), &credential(), &config())
        .await
        .expect("the sync answers");
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "two pages");
    assert!(requests[0].1.starts_with(&format!(
        "https://graph.facebook.com/{GRAPH_VERSION}/act_1234567890/insights?level=campaign&time_increment=monthly&fields=spend,account_currency,campaign_id,campaign_name,date_start,date_stop&time_range=%7B%22since%22%3A%222026-07-01%22"
    )));
    assert!(
        !requests[0].1.contains(TOKEN),
        "the token is never in a URL"
    );
    let log = client.header_log.lock().unwrap();
    let auth = log[0]
        .iter()
        .find(|(name, _)| name == "authorization")
        .map(|(_, value)| value.clone());
    assert_eq!(auth, Some(format!("Bearer {TOKEN}")));
}

#[tokio::test]
async fn books_one_invoice_per_month_and_currency_and_a_usage_line_per_campaign() {
    let out = sync_with(Recorded::new(routes()), &credential(), &config())
        .await
        .expect("sync");
    let mut ids: Vec<&str> = out
        .invoices
        .iter()
        .map(|i| i.external_id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        ["meta-ads-cost:2026-07:USD", "meta-ads-cost:2026-08:USD"]
    );
    let july = out
        .invoices
        .iter()
        .find(|i| i.period == "2026-07")
        .expect("July");
    assert_eq!(july.total_minor, 133_456, "1234.56 + 100.00 in cents");
    let august = out
        .invoices
        .iter()
        .find(|i| i.period == "2026-08")
        .expect("August");
    assert_eq!(august.total_minor, 5_035, "50.10 + 0.25 in cents");
    assert!(
        out.usage
            .iter()
            .any(|u| u.resource_id == "120000000000001" && u.quantity == 123_456),
        "campaign lines keep their spend in minor units"
    );
}

#[tokio::test]
async fn the_current_month_is_skipped_and_a_repeat_import_is_the_same() {
    let first = sync_with(Recorded::new(routes()), &credential(), &config())
        .await
        .expect("first");
    assert!(first.invoices.iter().all(|i| i.period != "2026-10"));
    assert!(first.usage.iter().all(|u| u.period != "2026-10"));
    let second = sync_with(Recorded::new(routes()), &credential(), &config())
        .await
        .expect("second");
    assert_eq!(first.invoices, second.invoices);
    assert_eq!(first.usage, second.usage);
}

#[tokio::test]
async fn refusals_are_named_and_not_retried() {
    for status in [401_u16, 403] {
        let client = Recorded::script(vec![(status, "{}")]);
        let out = sync_with(client.clone(), &credential(), &config()).await;
        assert!(matches!(out, Err(VendorError::Refused { status: s, .. }) if s == status));
        assert_eq!(client.calls(), 1);
    }
}

#[tokio::test]
async fn throttling_is_retried_with_backoff_then_reported() {
    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let (http, pause) = vendor(client.clone());
    let out = MetaAds
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
    assert_eq!(client.calls(), 3);
    let waits = pause.0.lock().unwrap().clone();
    assert_eq!(waits.len(), 2, "a wait between each of the three attempts");
    assert!(waits[1] > waits[0], "the backoff grows");
}

#[tokio::test]
async fn server_errors_are_retried_then_reported() {
    let client = Recorded::script(vec![(500, "{}"), (502, "{}"), (503, "{}")]);
    let out = sync_with(client.clone(), &credential(), &config()).await;
    assert!(matches!(out, Err(VendorError::Status { status: 503, .. })));
    assert_eq!(client.calls(), 3);
}

#[tokio::test]
async fn a_paging_link_that_leaves_the_pinned_path_is_refused() {
    let client = Recorded::script(vec![(
        200,
        r#"{"data":[],"paging":{"next":"https://graph.facebook.com/v1.0/other"}}"#,
    )]);
    let out = sync_with(client.clone(), &credential(), &config()).await;
    assert!(matches!(out, Err(VendorError::Malformed { .. })));
    assert_eq!(client.calls(), 1);
}

#[test]
fn the_ad_account_id_is_checked_before_it_reaches_a_url() {
    assert!(
        MetaAds
            .validate_config(&json!({"ad_account_id": "act_12345", "expense_account": "6200"}))
            .is_ok()
    );
    assert!(
        MetaAds
            .validate_config(&json!({"ad_account_id": "1234567890", "expense_account": "6200"}))
            .is_ok()
    );
    for bad in ["act_12a45", "act_", "https://evil.test/x", "act_1234 5"] {
        assert!(
            MetaAds
                .validate_config(&json!({"ad_account_id": bad}))
                .is_err(),
            "{bad}"
        );
    }
    assert!(
        MetaAds
            .validate_config(&json!({"ad_account_id": "act_12345", "name_prefixes": "SQZ"}))
            .is_err()
    );
}
