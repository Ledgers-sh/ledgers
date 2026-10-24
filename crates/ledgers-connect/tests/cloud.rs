//! The cloud connectors (AWS, Vercel) against recorded answers (issue #142).
//! No test reaches a vendor: each answer is a fixture or an inline script, and
//! every request is recorded with its method, URL and authorization header.

mod support;

use ledgers_connect::aws::AwsCostExplorer;
use ledgers_connect::connector::{Connector, SyncContext};
use ledgers_connect::http::VendorError;
use ledgers_connect::model::SyncOutput;
use ledgers_connect::vercel::Vercel;
use serde_json::{Value, json};
use std::sync::Arc;
use support::{Recorded, RecordedPause, Route, vendor};

const NOW: &str = "2026-10-24T10:00:00Z";
/// Fake keys. The AWS example key id and secret from the AWS documentation.
const AWS_KEY_ID: &str = "AKIDEXAMPLE";
const AWS_SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
const VERCEL_TOKEN: &str = "vercel-test-token-not-real";
const AWS_FIXTURE: &str = include_str!("../fixtures/aws_cost_2026_07.json");
const VERCEL_FIXTURE: &str = include_str!("../fixtures/vercel_charges_2026_07.jsonl");

fn aws_credential() -> String {
    json!({"access_key_id": AWS_KEY_ID, "secret_access_key": AWS_SECRET}).to_string()
}

fn aws_routes() -> Vec<Route> {
    vec![Route {
        url: "ce.us-east-1.amazonaws.com",
        body: "TimePeriod",
        status: 200,
        answer: AWS_FIXTURE.to_owned(),
    }]
}

fn vercel_routes() -> Vec<Route> {
    vec![Route {
        url: "api.vercel.com/v1/billing/charges",
        body: "",
        status: 200,
        answer: VERCEL_FIXTURE.to_owned(),
    }]
}

async fn run(
    connector: &dyn Connector,
    credential: &str,
    client: Arc<Recorded>,
    config: &Value,
    since: &str,
    until: &str,
) -> (Result<SyncOutput, VendorError>, Arc<RecordedPause>) {
    let (http, pause) = vendor(client);
    let out = connector
        .sync(&SyncContext {
            http: &http,
            credential,
            now: NOW,
            config,
            since,
            until,
        })
        .await;
    (out, pause)
}

fn invoice_total(out: &SyncOutput, id: &str) -> Option<i64> {
    out.invoices
        .iter()
        .find(|invoice| invoice.external_id == id)
        .map(|invoice| invoice.total_minor)
}

// --- AWS -------------------------------------------------------------------

#[tokio::test]
async fn aws_books_one_invoice_per_month_and_currency_from_daily_service_costs() {
    let client = Recorded::new(aws_routes());
    let (out, _) = run(
        &AwsCostExplorer,
        &aws_credential(),
        client.clone(),
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-08-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    // 10.123456 + 0.5 + 20 = 30.623456 USD, rounded half up to cents.
    assert_eq!(invoice_total(&out, "aws-cost:2026-07:USD"), Some(3062));
    // 5.25 USD in August.
    assert_eq!(invoice_total(&out, "aws-cost:2026-08:USD"), Some(525));
    assert_eq!(out.invoices.len(), 2);
    let ec2 = out
        .usage
        .iter()
        .find(|row| row.period == "2026-07" && row.resource_id.contains("Elastic Compute"))
        .expect("EC2 usage");
    assert_eq!(ec2.resource_kind, "service");
    assert_eq!(
        ec2.quantity, 3012,
        "the EC2 cost in cents is its allocation weight"
    );
}

#[tokio::test]
async fn aws_signs_every_request_with_sigv4_for_the_cost_explorer_scope() {
    let client = Recorded::new(aws_routes());
    let (out, _) = run(
        &AwsCostExplorer,
        &aws_credential(),
        client.clone(),
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-08-31",
    )
    .await;
    out.expect("sync succeeds");
    let requests = client.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let (method, uri, authorization) = &requests[0];
    assert_eq!(method, "POST");
    assert_eq!(uri, "https://ce.us-east-1.amazonaws.com/");
    assert!(
        authorization.starts_with(&format!(
            "AWS4-HMAC-SHA256 Credential={AWS_KEY_ID}/20261024/us-east-1/ce/aws4_request, SignedHeaders=content-type;host;x-amz-date;x-amz-target, Signature="
        )),
        "{authorization}"
    );
}

#[tokio::test]
async fn aws_follows_next_page_tokens_and_sums_every_page() {
    let page_one = r#"{"ResultsByTime":[{"TimePeriod":{"Start":"2026-07-01","End":"2026-07-02"},"Groups":[{"Keys":["Amazon S3"],"Metrics":{"UnblendedCost":{"Amount":"1.00","Unit":"USD"}}}]}],"NextPageToken":"page-2"}"#;
    let page_two = r#"{"ResultsByTime":[{"TimePeriod":{"Start":"2026-07-02","End":"2026-07-03"},"Groups":[{"Keys":["Amazon S3"],"Metrics":{"UnblendedCost":{"Amount":"2.00","Unit":"USD"}}}]}],"NextPageToken":""}"#;
    let client = Recorded::script(vec![(200, page_one), (200, page_two)]);
    let (out, _) = run(
        &AwsCostExplorer,
        &aws_credential(),
        client.clone(),
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    assert_eq!(client.calls(), 2, "one request per page");
    assert_eq!(invoice_total(&out, "aws-cost:2026-07:USD"), Some(300));
}

#[tokio::test]
async fn aws_tag_key_allocates_by_tag_value_and_untagged_costs_are_named() {
    let answer = r#"{"ResultsByTime":[{"TimePeriod":{"Start":"2026-07-01","End":"2026-07-02"},"Groups":[
        {"Keys":["Amazon EC2","Team$ledgers"],"Metrics":{"UnblendedCost":{"Amount":"4.00","Unit":"USD"}}},
        {"Keys":["Amazon S3","Team$"],"Metrics":{"UnblendedCost":{"Amount":"1.00","Unit":"USD"}}}
    ]}],"NextPageToken":""}"#;
    let client = Recorded::script(vec![(200, answer)]);
    let (out, _) = run(
        &AwsCostExplorer,
        &aws_credential(),
        client,
        &json!({"expense_account": "6100", "tag_key": "Team"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    assert_eq!(invoice_total(&out, "aws-cost:2026-07:USD"), Some(500));
    let tags: Vec<(&str, i64)> = out
        .usage
        .iter()
        .filter(|row| row.resource_kind == "tag")
        .map(|row| (row.resource_id.as_str(), row.quantity))
        .collect();
    assert!(tags.contains(&("ledgers", 400)), "{tags:?}");
    assert!(tags.contains(&("untagged", 100)), "{tags:?}");
}

#[tokio::test]
async fn aws_refusals_are_not_retried_and_throttling_is_retried_then_reported() {
    for status in [401_u16, 403] {
        let client = Recorded::script(vec![(status, "{}")]);
        let (out, pause) = run(
            &AwsCostExplorer,
            &aws_credential(),
            client.clone(),
            &json!({"expense_account": "6100"}),
            "2026-07-01",
            "2026-07-31",
        )
        .await;
        assert!(
            matches!(out, Err(VendorError::Refused { status: s, .. }) if s == status),
            "{status}"
        );
        assert_eq!(client.calls(), 1, "a refusal is not retried");
        assert!(pause.0.lock().unwrap().is_empty());
    }
    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let (out, pause) = run(
        &AwsCostExplorer,
        &aws_credential(),
        client.clone(),
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(
        matches!(out, Err(VendorError::Status { status: 429, .. })),
        "{out:?}"
    );
    assert_eq!(
        client.calls(),
        3,
        "throttling is retried to the attempt limit"
    );
    assert_eq!(
        pause.0.lock().unwrap().len(),
        2,
        "the waits between attempts are recorded"
    );
    let client = Recorded::script(vec![(500, "{}"), (503, "{}"), (500, "{}")]);
    let (out, _) = run(
        &AwsCostExplorer,
        &aws_credential(),
        client,
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(
        matches!(out, Err(VendorError::Status { status: 500, .. })),
        "{out:?}"
    );
}

#[tokio::test]
async fn aws_refuses_a_credential_without_both_fields_and_a_bad_tag_key() {
    let client = Recorded::new(aws_routes());
    let (out, _) = run(
        &AwsCostExplorer,
        r#"{"access_key_id":"AKIDEXAMPLE"}"#,
        client.clone(),
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Malformed { .. })));
    assert_eq!(
        client.calls(),
        0,
        "nothing is sent without a complete credential"
    );
    assert!(
        AwsCostExplorer
            .validate_config(&json!({"tag_key": "Team"}))
            .is_ok()
    );
    assert!(
        AwsCostExplorer
            .validate_config(&json!({"tag_key": "a$b"}))
            .is_err()
    );
    assert!(
        AwsCostExplorer
            .validate_config(&json!({"tag_key": "  "}))
            .is_err()
    );
    assert!(
        AwsCostExplorer
            .validate_config(&json!({"tag_key": 5}))
            .is_err()
    );
    assert!(
        AwsCostExplorer.validate_config(&json!({})).is_ok(),
        "tag_key is optional"
    );
}

// --- Vercel ----------------------------------------------------------------

#[tokio::test]
async fn vercel_books_usage_and_tax_by_project_and_never_books_a_purchase() {
    let client = Recorded::new(vercel_routes());
    let (out, _) = run(
        &Vercel,
        VERCEL_TOKEN,
        client.clone(),
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-08-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    // July: 12.50 + 0.75 USD (the 100 USD purchase is not an expense).
    assert_eq!(invoice_total(&out, "vercel-cost:2026-07:USD"), Some(1325));
    assert_eq!(invoice_total(&out, "vercel-cost:2026-08:USD"), Some(100));
    let project = out
        .usage
        .iter()
        .find(|row| row.period == "2026-07" && row.resource_id == "prj_ledgers")
        .expect("project usage");
    assert_eq!(project.quantity, 1250);
    assert!(out.usage.iter().any(|row| row.period == "2026-07"
        && row.resource_id == "unattributed"
        && row.quantity == 75));
    assert!(
        out.notes
            .iter()
            .any(|note| note.contains("prepaid credit purchase"))
    );
    assert!(
        out.notes
            .iter()
            .any(|note| note.contains("invoices are not in the REST API"))
    );
    let (method, uri, authorization) = client.requests.lock().unwrap()[0].clone();
    assert_eq!(method, "GET");
    assert!(uri.starts_with("https://api.vercel.com/v1/billing/charges?from=2026-07-01T00:00:00Z&to=2026-09-01T00:00:00Z"), "{uri}");
    assert_eq!(authorization, format!("Bearer {VERCEL_TOKEN}"));
}

#[tokio::test]
async fn vercel_sends_the_team_scope_when_configured() {
    let client = Recorded::new(vercel_routes());
    let (out, _) = run(
        &Vercel,
        VERCEL_TOKEN,
        client.clone(),
        &json!({"expense_account": "6100", "team_id": "team_abc123"}),
        "2026-07-01",
        "2026-08-31",
    )
    .await;
    out.expect("sync succeeds");
    assert!(
        client.requests.lock().unwrap()[0]
            .1
            .ends_with("&teamId=team_abc123")
    );
}

#[tokio::test]
async fn vercel_refusals_retries_and_a_malformed_line_are_mapped() {
    let client = Recorded::script(vec![(401, "{}")]);
    let (out, _) = run(
        &Vercel,
        VERCEL_TOKEN,
        client.clone(),
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 401, .. })));
    assert_eq!(client.calls(), 1);
    let client = Recorded::script(vec![(403, "{}")]);
    let (out, _) = run(
        &Vercel,
        VERCEL_TOKEN,
        client.clone(),
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 403, .. })));
    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let (out, pause) = run(
        &Vercel,
        VERCEL_TOKEN,
        client.clone(),
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(client.calls(), 3);
    assert_eq!(pause.0.lock().unwrap().len(), 2);
    let client = Recorded::script(vec![(500, "{}"), (500, "{}"), (500, "{}")]);
    let (out, _) = run(
        &Vercel,
        VERCEL_TOKEN,
        client,
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 500, .. })));
    let client = Recorded::script(vec![(200, "{\"BilledCost\":1}\nnot json")]);
    let (out, _) = run(
        &Vercel,
        VERCEL_TOKEN,
        client,
        &json!({"expense_account": "6100"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Malformed { .. })));
}

#[test]
fn vercel_config_is_checked_before_a_sync() {
    assert!(
        Vercel
            .validate_config(&json!({"team_id": "team_abc123"}))
            .is_ok()
    );
    assert!(Vercel.validate_config(&json!({})).is_ok());
    assert!(
        Vercel
            .validate_config(&json!({"team_id": "team id"}))
            .is_err()
    );
    assert!(Vercel.validate_config(&json!({"team_id": ""})).is_err());
}

#[test]
fn the_cloud_connectors_are_registered_with_their_hosts_and_fields() {
    let aws = ledgers_connect::connector::by_id("aws").expect("registered");
    assert_eq!(aws.allowed_hosts(), &["ce.us-east-1.amazonaws.com"]);
    let names: Vec<&str> = aws.credential_fields().iter().map(|f| f.name).collect();
    assert_eq!(names, ["access_key_id", "secret_access_key"]);
    let vercel = ledgers_connect::connector::by_id("vercel").expect("registered");
    assert_eq!(vercel.allowed_hosts(), &["api.vercel.com"]);
    assert_eq!(vercel.credential_fields()[0].name, "token");
}
