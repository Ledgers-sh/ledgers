//! The Google Cloud, Azure and Supabase connectors against recorded answers
//! (issue #142). No test reaches a vendor. Each answer is a fixture or an inline
//! script, and every request and body is recorded.
//!
//! The Google Cloud signing key in `fixtures/gcp_test_only_not_a_real_key.pem`
//! is a throwaway 2048-bit test key. It signs nothing outside these tests.

mod support;

use ledgers_connect::azure::AzureCost;
use ledgers_connect::connector::{Connector, SyncContext};
use ledgers_connect::gcp::{GoogleCloud, pem_to_der, signed_assertion};
use ledgers_connect::http::VendorError;
use ledgers_connect::model::SyncOutput;
use ledgers_connect::supabase::Supabase;
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaKeyPair, UnparsedPublicKey};
use serde_json::{Value, json};
use std::sync::Arc;
use support::{Recorded, RecordedPause, Route, vendor};

const NOW: &str = "2026-10-24T10:00:00Z";
const GCP_PEM: &str = include_str!("../fixtures/gcp_test_only_not_a_real_key.pem");
const GCP_QUERY: &str = include_str!("../fixtures/gcp_query_2026_07.json");
const AZURE_QUERY: &str = include_str!("../fixtures/azure_query_2026_07.json");
const SUPABASE_PROJECTS: &str = include_str!("../fixtures/supabase_projects.json");
const SUPABASE_ADDONS: &str = include_str!("../fixtures/supabase_addons.json");
const SUPABASE_USAGE: &str = include_str!("../fixtures/supabase_usage.json");
const SUPABASE_TOKEN: &str = "sbp_test_token_not_real";
const AZURE_SECRET: &str = "s3cr3t/+=test";

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

fn total(out: &SyncOutput, id: &str) -> Option<i64> {
    out.invoices
        .iter()
        .find(|invoice| invoice.external_id == id)
        .map(|invoice| invoice.total_minor)
}

// --- Google Cloud ----------------------------------------------------------

fn gcp_credential() -> String {
    let account = json!({"client_email": "ledgers-billing@billing-proj-test.iam.gserviceaccount.com", "private_key": GCP_PEM});
    json!({"service_account_json": account.to_string()}).to_string()
}

fn gcp_config() -> Value {
    json!({"project": "billing-proj-test", "dataset": "billing_export", "table": "gcp_billing_export_v1", "expense_account": "6100"})
}

fn gcp_routes() -> Vec<Route> {
    vec![
        Route {
            url: "oauth2.googleapis.com/token",
            body: "grant_type",
            status: 200,
            answer: r#"{"access_token":"ya29.TEST","expires_in":3600}"#.to_owned(),
        },
        Route {
            url: "bigquery.googleapis.com/bigquery/v2/projects/billing-proj-test/queries",
            body: "usage_start_time",
            status: 200,
            answer: GCP_QUERY.to_owned(),
        },
    ]
}

#[test]
fn gcp_signs_an_rs256_assertion_that_verifies_with_the_key() {
    let assertion = signed_assertion("svc@example.test", GCP_PEM, 1_792_836_000).expect("signed");
    let parts: Vec<&str> = assertion.split('.').collect();
    assert_eq!(parts.len(), 3, "a JWT has three parts");
    let header: Value = serde_json::from_slice(&base64_url(parts[0])).unwrap();
    assert_eq!(header, json!({"alg": "RS256", "typ": "JWT"}));
    let claims: Value = serde_json::from_slice(&base64_url(parts[1])).unwrap();
    assert_eq!(claims["iss"], "svc@example.test");
    assert_eq!(claims["aud"], "https://oauth2.googleapis.com/token");
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/bigquery.readonly"
    );
    assert_eq!(claims["iat"], 1_792_836_000_i64);
    assert_eq!(claims["exp"], 1_792_839_600_i64);

    // The signature must verify with the public half of the key, and no other message.
    let der = pem_to_der(GCP_PEM).unwrap();
    let public = RsaKeyPair::from_pkcs8(&der)
        .unwrap()
        .public()
        .as_ref()
        .to_vec();
    let signature = base64_url(parts[2]);
    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let key = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public);
    key.verify(signing_input.as_bytes(), &signature)
        .expect("the signature verifies");
    assert!(key.verify(b"another message", &signature).is_err());
}

fn base64_url(part: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part)
        .expect("base64url")
}

#[tokio::test]
async fn gcp_exchanges_the_assertion_and_books_the_export_by_month_currency_and_project() {
    let client = Recorded::new(gcp_routes());
    let (out, _) = run(
        &GoogleCloud,
        &gcp_credential(),
        client.clone(),
        &gcp_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    // USD: 30.623456 + 1.50 = 32.123456, rounded half up to cents. EUR is negative: noted, not booked.
    assert_eq!(total(&out, "gcp-cost:2026-07:USD"), Some(3212));
    assert_eq!(total(&out, "gcp-cost:2026-07:EUR"), None);
    assert!(out.notes.iter().any(|note| note.contains("negative")));
    // July has USD and EUR, so no resource weights are written for it (weights are per period).
    assert!(
        out.usage.is_empty(),
        "a multi-currency period writes no weights"
    );
    assert!(
        out.notes
            .iter()
            .any(|note| note.contains("more than one currency"))
    );

    let requests = client.requests.lock().unwrap().clone();
    assert_eq!(requests[0].1, "https://oauth2.googleapis.com/token");
    let bodies = client.bodies.lock().unwrap().clone();
    assert!(
        bodies[0].contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer"),
        "{}",
        bodies[0]
    );
    assert!(
        bodies[0].contains("assertion=eyJ"),
        "the assertion is a JWT"
    );
    assert_eq!(
        requests[1].2, "Bearer ya29.TEST",
        "the BigQuery call carries the access token"
    );
    let sql: Value = serde_json::from_str(&bodies[1]).unwrap();
    let text = sql["query"].as_str().unwrap();
    assert!(
        text.contains("FROM `billing-proj-test.billing_export.gcp_billing_export_v1`"),
        "{text}"
    );
    assert!(
        text.contains("usage_start_time >= TIMESTAMP('2026-07-01')"),
        "{text}"
    );
    assert!(
        text.contains("usage_start_time < TIMESTAMP('2026-08-01')"),
        "{text}"
    );
    assert_eq!(sql["useLegacySql"], false);
}

#[tokio::test]
async fn gcp_follows_a_running_job_and_the_page_tokens() {
    let running = r#"{"jobComplete":false,"jobReference":{"jobId":"job_run_1","location":"US"}}"#;
    let done_page_one = r#"{"jobComplete":true,"jobReference":{"jobId":"job_run_1","location":"US"},"schema":{"fields":[{"name":"period"},{"name":"currency"},{"name":"project_id"},{"name":"project_name"},{"name":"net_cost"}]},"rows":[{"f":[{"v":"2026-07"},{"v":"USD"},{"v":"ledgers-prod"},{"v":"Ledgers"},{"v":"1.00"}]}],"pageToken":"page-2"}"#;
    let page_two = r#"{"jobComplete":true,"rows":[{"f":[{"v":"2026-07"},{"v":"USD"},{"v":"sqeezin"},{"v":"Sqeezin"},{"v":"2.00"}]}]}"#;
    let client = Recorded::script(vec![
        (200, r#"{"access_token":"ya29.SCRIPT"}"#),
        (200, running),
        (200, done_page_one),
        (200, page_two),
    ]);
    let (out, _) = run(
        &GoogleCloud,
        &gcp_credential(),
        client.clone(),
        &gcp_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    assert_eq!(client.calls(), 4, "token, the job, the poll, the next page");
    assert_eq!(total(&out, "gcp-cost:2026-07:USD"), Some(300));
    assert!(
        out.usage
            .iter()
            .any(|row| row.resource_id == "sqeezin" && row.quantity == 200)
    );
    let uris: Vec<String> = client
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.1.clone())
        .collect();
    assert!(uris[3].contains("pageToken=page-2"), "{}", uris[3]);
}

#[tokio::test]
async fn gcp_maps_refusals_throttling_and_server_errors() {
    let client = Recorded::script(vec![(401, "{}")]);
    let (out, _) = run(
        &GoogleCloud,
        &gcp_credential(),
        client.clone(),
        &gcp_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 401, .. })));
    assert_eq!(client.calls(), 1, "a refused key is not retried");

    let client = Recorded::script(vec![(200, r#"{"access_token":"ya29.X"}"#), (403, "{}")]);
    let (out, _) = run(
        &GoogleCloud,
        &gcp_credential(),
        client,
        &gcp_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 403, .. })));

    let client = Recorded::script(vec![
        (200, r#"{"access_token":"ya29.X"}"#),
        (429, "{}"),
        (429, "{}"),
        (429, "{}"),
    ]);
    let (out, pause) = run(
        &GoogleCloud,
        &gcp_credential(),
        client.clone(),
        &gcp_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(
        client.calls(),
        4,
        "the token, then three attempts at the query"
    );
    assert_eq!(pause.0.lock().unwrap().len(), 2);

    let client = Recorded::script(vec![
        (200, r#"{"access_token":"ya29.X"}"#),
        (500, "{}"),
        (503, "{}"),
        (500, "{}"),
    ]);
    let (out, _) = run(
        &GoogleCloud,
        &gcp_credential(),
        client,
        &gcp_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 500, .. })));
}

#[tokio::test]
async fn gcp_refuses_a_key_that_is_not_a_service_account_and_bad_names() {
    let client = Recorded::new(gcp_routes());
    let bad_key = json!({"service_account_json": json!({"client_email": "x@y.test", "private_key": "not a key"}).to_string()}).to_string();
    let (out, _) = run(
        &GoogleCloud,
        &bad_key,
        client.clone(),
        &gcp_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Malformed { .. })), "{out:?}");
    assert_eq!(client.calls(), 0, "no request is sent with an unusable key");

    assert!(GoogleCloud.validate_config(&gcp_config()).is_ok());
    assert!(
        GoogleCloud
            .validate_config(&json!({"project": "Bad_Project", "dataset": "d", "table": "t"}))
            .is_err()
    );
    assert!(
        GoogleCloud
            .validate_config(
                &json!({"project": "billing-proj-test", "dataset": "a.b", "table": "t"})
            )
            .is_err()
    );
    assert!(
        GoogleCloud
            .validate_config(
                &json!({"project": "billing-proj-test", "dataset": "d", "table": "t`x"})
            )
            .is_err()
    );
}

// --- Azure -----------------------------------------------------------------

const TENANT: &str = "11111111-2222-3333-4444-555555555555";
const CLIENT: &str = "66666666-7777-8888-9999-aaaaaaaaaaaa";
const SUBSCRIPTION: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

fn azure_config() -> Value {
    json!({"tenant_id": TENANT, "client_id": CLIENT, "subscription_id": SUBSCRIPTION, "expense_account": "6100"})
}

fn azure_credential() -> String {
    json!({"client_secret": AZURE_SECRET}).to_string()
}

fn azure_routes() -> Vec<Route> {
    vec![
        Route {
            url: "login.microsoftonline.com",
            body: "client_credentials",
            status: 200,
            answer: r#"{"access_token":"az-TEST","token_type":"Bearer"}"#.to_owned(),
        },
        Route {
            url: "management.azure.com/subscriptions/",
            body: "PreTaxCost",
            status: 200,
            answer: AZURE_QUERY.to_owned(),
        },
    ]
}

#[tokio::test]
async fn azure_reads_each_month_with_a_client_credentials_token() {
    let client = Recorded::new(azure_routes());
    let (out, _) = run(
        &AzureCost,
        &azure_credential(),
        client.clone(),
        &azure_config(),
        "2026-07-01",
        "2026-08-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    // Each month's rows: USD 12.50 + 0.25 = 12.75; EUR 3.125 rounds to 3.13.
    assert_eq!(total(&out, "azure-cost:2026-07:USD"), Some(1275));
    assert_eq!(total(&out, "azure-cost:2026-08:USD"), Some(1275));
    assert_eq!(total(&out, "azure-cost:2026-07:EUR"), Some(313));
    // July has USD and EUR, so no weights are written; the notes say why.
    assert!(out.usage.is_empty());
    assert!(
        out.notes
            .iter()
            .any(|note| note.contains("more than one currency"))
    );

    let requests = client.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3, "the token, then one query per month");
    assert!(requests[1].1.contains(&format!("subscriptions/{SUBSCRIPTION}/providers/Microsoft.CostManagement/query?api-version=2025-03-01")));
    assert_eq!(requests[1].2, "Bearer az-TEST");
    let bodies = client.bodies.lock().unwrap().clone();
    assert!(
        bodies[0].contains("scope=https%3A%2F%2Fmanagement.azure.com%2F.default"),
        "{}",
        bodies[0]
    );
    assert!(
        bodies[0].contains("client_secret=s3cr3t%2F%2B%3Dtest"),
        "the secret is percent-encoded, never raw"
    );
    let query: Value = serde_json::from_str(&bodies[1]).unwrap();
    assert_eq!(query["type"], "Usage");
    assert_eq!(query["timePeriod"]["from"], "2026-07-01T00:00:00Z");
    assert_eq!(query["timePeriod"]["to"], "2026-07-31T23:59:59Z");
    assert_eq!(
        query["dataset"]["aggregation"]["totalCost"]["name"],
        "PreTaxCost"
    );
}

#[tokio::test]
async fn azure_follows_next_links_within_a_month() {
    let page_one = r#"{"properties":{"columns":[{"name":"PreTaxCost"},{"name":"ResourceGroup"},{"name":"Currency"}],"nextLink":"https://management.azure.com/subscriptions/next-page","rows":[[1.0,"rg-a","USD"]]}}"#;
    let page_two = r#"{"properties":{"columns":[{"name":"PreTaxCost"},{"name":"ResourceGroup"},{"name":"Currency"}],"nextLink":null,"rows":[[2.0,"rg-b","USD"]]}}"#;
    let client = Recorded::script(vec![
        (200, r#"{"access_token":"az-X"}"#),
        (200, page_one),
        (200, page_two),
    ]);
    let (out, _) = run(
        &AzureCost,
        &azure_credential(),
        client.clone(),
        &azure_config(),
        "2026-07-01",
        "2026-07-15",
    )
    .await;
    let out = out.expect("sync succeeds");
    assert_eq!(client.calls(), 3);
    assert_eq!(total(&out, "azure-cost:2026-07:USD"), Some(300));
    assert!(
        client.requests.lock().unwrap()[2]
            .1
            .ends_with("/subscriptions/next-page")
    );
}

#[tokio::test]
async fn azure_maps_refusals_throttling_and_server_errors() {
    let client = Recorded::script(vec![(401, "{}")]);
    let (out, _) = run(
        &AzureCost,
        &azure_credential(),
        client.clone(),
        &azure_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 401, .. })));
    assert_eq!(client.calls(), 1);

    let client = Recorded::script(vec![(200, r#"{"access_token":"az-X"}"#), (403, "{}")]);
    let (out, _) = run(
        &AzureCost,
        &azure_credential(),
        client,
        &azure_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 403, .. })));

    let client = Recorded::script(vec![
        (200, r#"{"access_token":"az-X"}"#),
        (429, "{}"),
        (429, "{}"),
        (429, "{}"),
    ]);
    let (out, pause) = run(
        &AzureCost,
        &azure_credential(),
        client,
        &azure_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(pause.0.lock().unwrap().len(), 2);

    let client = Recorded::script(vec![
        (200, r#"{"access_token":"az-X"}"#),
        (500, "{}"),
        (500, "{}"),
        (500, "{}"),
    ]);
    let (out, _) = run(
        &AzureCost,
        &azure_credential(),
        client,
        &azure_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 500, .. })));
}

#[test]
fn azure_config_and_credential_are_checked() {
    assert!(AzureCost.validate_config(&azure_config()).is_ok());
    assert!(AzureCost.validate_config(&json!({"tenant_id": "not-a-guid", "client_id": CLIENT, "subscription_id": SUBSCRIPTION})).is_err());
    assert!(
        AzureCost
            .validate_config(&json!({"tenant_id": TENANT, "client_id": CLIENT}))
            .is_err()
    );
    let fields: Vec<&str> = AzureCost
        .credential_fields()
        .iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(fields, ["client_secret"]);
    assert_eq!(
        AzureCost.allowed_hosts(),
        &["login.microsoftonline.com", "management.azure.com"]
    );
}

// --- Supabase --------------------------------------------------------------

fn supabase_config() -> Value {
    json!({"organization": "acme-labs", "expense_account": "6100"})
}

fn supabase_routes() -> Vec<Route> {
    let answer = |text: &str| text.to_owned();
    vec![
        Route {
            url: "api.supabase.com/v1/organizations/acme-labs/projects",
            body: "",
            status: 200,
            answer: answer(SUPABASE_PROJECTS),
        },
        Route {
            url: "/v1/projects/abcdefghijklmnopqrst/billing/addons",
            body: "",
            status: 200,
            answer: answer(SUPABASE_ADDONS),
        },
        Route {
            url: "/v1/projects/abcdefghijklmnopqrst/analytics/endpoints/usage.api-counts",
            body: "",
            status: 200,
            answer: answer(SUPABASE_USAGE),
        },
        Route {
            url: "/v1/projects/tsrqponmlkjihgfedcba/billing/addons",
            body: "",
            status: 200,
            answer: answer(SUPABASE_ADDONS),
        },
        Route {
            url: "/v1/projects/tsrqponmlkjihgfedcba/analytics/endpoints/usage.api-counts",
            body: "",
            status: 200,
            answer: answer(SUPABASE_USAGE),
        },
    ]
}

#[tokio::test]
async fn supabase_reports_add_on_prices_and_request_counts_as_notes_and_books_nothing() {
    let client = Recorded::new(supabase_routes());
    let (out, _) = run(
        &Supabase,
        SUPABASE_TOKEN,
        client.clone(),
        &supabase_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    assert!(
        out.invoices.is_empty(),
        "no bill: the add-on prices carry no currency"
    );
    assert!(out.usage.is_empty(), "no usage rows: nothing is split");
    assert!(out.notes.iter().any(|note| note == "ledgers-prod: compute_instance ci_small at 15 per monthly (amount without currency, not booked)"), "{:?}", out.notes);
    assert!(out.notes.iter().any(|note| note == "ledgers-prod: last day REST 200 requests, auth 15, realtime 3, storage 6 (counts, not money)"), "{:?}", out.notes);
    assert!(out.notes.iter().any(|note| note.contains("not booked")));
    let requests = client.requests.lock().unwrap().clone();
    assert!(
        requests
            .iter()
            .all(|r| r.2 == format!("Bearer {SUPABASE_TOKEN}"))
    );
    assert!(requests.iter().any(|r| r.1.ends_with(
        "/v1/projects/abcdefghijklmnopqrst/analytics/endpoints/usage.api-counts?interval=1day"
    )));
}

#[tokio::test]
async fn supabase_maps_refusals_throttling_and_server_errors() {
    let client = Recorded::script(vec![(401, "{}")]);
    let (out, _) = run(
        &Supabase,
        SUPABASE_TOKEN,
        client.clone(),
        &supabase_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 401, .. })));
    assert_eq!(client.calls(), 1);

    let client = Recorded::script(vec![(200, SUPABASE_PROJECTS), (403, "{}")]);
    let (out, _) = run(
        &Supabase,
        SUPABASE_TOKEN,
        client,
        &supabase_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 403, .. })));

    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let (out, pause) = run(
        &Supabase,
        SUPABASE_TOKEN,
        client.clone(),
        &supabase_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(client.calls(), 3);
    assert_eq!(pause.0.lock().unwrap().len(), 2);

    let client = Recorded::script(vec![(500, "{}"), (500, "{}"), (500, "{}")]);
    let (out, _) = run(
        &Supabase,
        SUPABASE_TOKEN,
        client,
        &supabase_config(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 500, .. })));
}

#[test]
fn supabase_config_is_checked_and_the_connector_is_registered() {
    assert!(Supabase.validate_config(&supabase_config()).is_ok());
    assert!(
        Supabase
            .validate_config(&json!({"organization": "bad/slug"}))
            .is_err()
    );
    assert!(Supabase.validate_config(&json!({})).is_err());
    assert_eq!(Supabase.allowed_hosts(), &["api.supabase.com"]);
    assert!(ledgers_connect::connector::by_id("gcp").is_some());
    assert!(ledgers_connect::connector::by_id("azure").is_some());
    assert!(ledgers_connect::connector::by_id("supabase").is_some());
}
