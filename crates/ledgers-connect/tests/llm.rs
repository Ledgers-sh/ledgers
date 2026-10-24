//! The `OpenAI` and `OpenRouter` connectors against recorded answers (issue #142).
//! No test reaches a vendor. Each answer is a fixture or an inline script, and every
//! request is recorded with its URL and authorization header.

mod support;

use ledgers_connect::connector::{Connector, SyncContext};
use ledgers_connect::http::VendorError;
use ledgers_connect::model::SyncOutput;
use ledgers_connect::openai::OpenAi;
use ledgers_connect::openrouter::OpenRouter;
use serde_json::json;
use std::sync::Arc;
use support::{Recorded, RecordedPause, Route, vendor};

const NOW: &str = "2026-10-24T10:00:00Z";
const OPENAI_FIXTURE: &str = include_str!("../fixtures/openai_costs_2026_07.json");
const OPENAI_KEY: &str = "sk-admin-test-not-real";
const OPENROUTER_KEY: &str = "mgmt-test-not-real";

async fn run(
    connector: &dyn Connector,
    credential: &str,
    client: Arc<Recorded>,
    since: &str,
    until: &str,
) -> (Result<SyncOutput, VendorError>, Arc<RecordedPause>) {
    let (http, pause) = vendor(client);
    let config = json!({"expense_account": "6200"});
    let out = connector
        .sync(&SyncContext {
            http: &http,
            credential,
            now: NOW,
            config: &config,
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

// --- OpenAI ----------------------------------------------------------------

fn openai_routes() -> Vec<Route> {
    vec![Route {
        url: "api.openai.com/v1/organization/costs",
        body: "",
        status: 200,
        answer: OPENAI_FIXTURE.to_owned(),
    }]
}

#[tokio::test]
async fn openai_books_costs_by_month_currency_and_project_from_the_admin_endpoint() {
    let client = Recorded::new(openai_routes());
    let (out, _) = run(
        &OpenAi,
        OPENAI_KEY,
        client.clone(),
        "2026-07-01",
        "2026-08-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    // July: 10.123456 + 0.50 + 20 = 30.623456, rounded to cents. August: 5.25.
    assert_eq!(total(&out, "openai-cost:2026-07:USD"), Some(3062));
    assert_eq!(total(&out, "openai-cost:2026-08:USD"), Some(525));
    // July has one currency, so its weights are kept: 30.123456 USD for the project, 0.50 unattributed.
    let project = out
        .usage
        .iter()
        .find(|row| row.resource_id == "proj_ledgers" && row.period == "2026-07")
        .expect("project weight");
    assert_eq!(project.quantity, 3012);
    assert!(
        out.usage
            .iter()
            .any(|row| row.resource_id == "unattributed" && row.quantity == 50)
    );
    let requests = client.requests.lock().unwrap().clone();
    assert_eq!(requests[0].2, format!("Bearer {OPENAI_KEY}"));
    let uri = &requests[0].1;
    // 2026-07-01 and 2026-09-01 as Unix seconds; daily buckets grouped by project.
    assert!(uri.contains("start_time=1782864000"), "{uri}");
    assert!(uri.contains("end_time=1788220800"), "{uri}");
    assert!(
        uri.contains("bucket_width=1d") && uri.contains("group_by=project_id"),
        "{uri}"
    );
}

#[tokio::test]
async fn openai_follows_the_cursor_while_has_more() {
    let page_one = r#"{"data":[{"start_time":1782864000,"results":[{"amount":{"value":1.0,"currency":"usd"},"project_id":"p1"}]}],"has_more":true,"next_page":"cursor-2"}"#;
    let page_two = r#"{"data":[{"start_time":1782950400,"results":[{"amount":{"value":2.0,"currency":"usd"},"project_id":"p1"}]}],"has_more":false,"next_page":null}"#;
    let client = Recorded::script(vec![(200, page_one), (200, page_two)]);
    let (out, _) = run(
        &OpenAi,
        OPENAI_KEY,
        client.clone(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    assert_eq!(client.calls(), 2);
    assert_eq!(total(&out, "openai-cost:2026-07:USD"), Some(300));
    assert!(
        client.requests.lock().unwrap()[1]
            .1
            .contains("page=cursor-2")
    );
}

#[tokio::test]
async fn openai_skips_buckets_outside_the_window_and_maps_errors() {
    // The window is August only, so July's buckets are skipped.
    let client = Recorded::new(openai_routes());
    let (out, _) = run(&OpenAi, OPENAI_KEY, client, "2026-08-01", "2026-08-31").await;
    let out = out.expect("sync succeeds");
    assert_eq!(out.invoices.len(), 1);
    assert_eq!(out.invoices[0].external_id, "openai-cost:2026-08:USD");

    for status in [401_u16, 403] {
        let client = Recorded::script(vec![(status, "{}")]);
        let (out, _) = run(
            &OpenAi,
            OPENAI_KEY,
            client.clone(),
            "2026-07-01",
            "2026-07-31",
        )
        .await;
        assert!(
            matches!(out, Err(VendorError::Refused { status: s, .. }) if s == status),
            "{status}"
        );
        assert_eq!(client.calls(), 1, "a refusal is not retried");
    }
    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let (out, pause) = run(
        &OpenAi,
        OPENAI_KEY,
        client.clone(),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(client.calls(), 3);
    assert_eq!(pause.0.lock().unwrap().len(), 2);
    let client = Recorded::script(vec![(500, "{}"), (503, "{}"), (500, "{}")]);
    let (out, _) = run(&OpenAi, OPENAI_KEY, client, "2026-07-01", "2026-07-31").await;
    assert!(matches!(out, Err(VendorError::Status { status: 500, .. })));
    // A project key cannot read costs: the endpoint answers 404, which is reported, not booked.
    let client = Recorded::script(vec![(404, "{}")]);
    let (out, _) = run(&OpenAi, OPENAI_KEY, client, "2026-07-01", "2026-07-31").await;
    assert!(
        matches!(out, Err(VendorError::Status { status: 404, .. })),
        "{out:?}"
    );
}

#[test]
fn openai_is_registered_and_needs_an_admin_key() {
    let connector = ledgers_connect::connector::by_id("openai").expect("registered");
    assert_eq!(connector.allowed_hosts(), &["api.openai.com"]);
    assert_eq!(connector.credential_fields()[0].name, "token");
    assert!(connector.credential_fields()[0].label.contains("Admin"));
    assert!(OpenAi.validate_config(&json!({})).is_ok());
}

// --- OpenRouter ------------------------------------------------------------

/// The activity answer for `day`: `rows` is the JSON of the `data` array.
fn activity(rows: &str) -> String {
    format!(r#"{{"data":{rows}}}"#)
}

fn openrouter_routes() -> Vec<Route> {
    vec![
        Route {
            url: "openrouter.ai/api/v1/activity?date=2026-10-20",
            body: "",
            status: 200,
            answer: activity(
                r#"[{"model":"a","usage":1.5,"requests":3},{"model":"b","usage":0.25,"requests":1}]"#,
            ),
        },
        Route {
            url: "openrouter.ai/api/v1/activity?date=2026-10-21",
            body: "",
            status: 200,
            answer: activity("[]"),
        },
        Route {
            url: "openrouter.ai/api/v1/activity?date=2026-10-22",
            body: "",
            status: 200,
            answer: activity(r#"[{"model":"a","usage":0.005,"requests":1}]"#),
        },
        Route {
            url: "openrouter.ai/api/v1/activity?date=2026-10-23",
            body: "",
            status: 200,
            answer: activity(r#"[{"model":"b","usage":2.0,"requests":4}]"#),
        },
    ]
}

#[tokio::test]
async fn openrouter_books_each_completed_day_once_and_keeps_zero_days_out() {
    let client = Recorded::new(openrouter_routes());
    let (out, _) = run(
        &OpenRouter,
        OPENROUTER_KEY,
        client.clone(),
        "2026-10-20",
        "2026-10-23",
    )
    .await;
    let out = out.expect("sync succeeds");
    // 1.50 + 0.25 = 1.75; 0.005 rounds half up to 1 cent; a day with no rows is not booked.
    assert_eq!(total(&out, "openrouter-usage:2026-10-20:USD"), Some(175));
    assert_eq!(total(&out, "openrouter-usage:2026-10-22:USD"), Some(1));
    assert_eq!(total(&out, "openrouter-usage:2026-10-23:USD"), Some(200));
    assert!(total(&out, "openrouter-usage:2026-10-21:USD").is_none());
    assert_eq!(out.invoices.len(), 3);
    assert!(
        out.invoices
            .iter()
            .all(|i| i.period == "2026-10" && i.currency == "USD")
    );
    assert!(out.usage.is_empty(), "no per-day usage weights are written");
    assert!(out.notes.iter().any(|note| note.contains("default unit")));
    assert!(
        out.notes
            .iter()
            .any(|note| note == "2026-10-20: 4 request(s) across 2 model(s)"),
        "{:?}",
        out.notes
    );
    let requests = client.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 4, "one activity request per completed day");
    assert!(
        requests
            .iter()
            .all(|r| r.2 == format!("Bearer {OPENROUTER_KEY}"))
    );
}

#[tokio::test]
async fn openrouter_test_run_reads_yesterday_to_prove_the_key_and_books_nothing() {
    let client = Recorded::new(vec![Route {
        url: "openrouter.ai/api/v1/activity?date=2026-10-23",
        body: "",
        status: 200,
        answer: activity(r#"[{"model":"b","usage":2.0,"requests":4}]"#),
    }]);
    let (out, _) = run(
        &OpenRouter,
        OPENROUTER_KEY,
        client.clone(),
        "2026-10-24",
        "2026-10-24",
    )
    .await;
    let out = out.expect("the key works");
    assert!(out.invoices.is_empty(), "a test books nothing");
    assert_eq!(client.calls(), 1);
    assert!(out.notes.iter().any(|note| note.starts_with("2026-10-23:")));
}

#[tokio::test]
async fn openrouter_reads_only_the_retained_thirty_days() {
    // August is older than the 30 retained days, so only the probe of yesterday is read.
    let client = Recorded::new(openrouter_routes());
    let (out, _) = run(
        &OpenRouter,
        OPENROUTER_KEY,
        client.clone(),
        "2026-08-01",
        "2026-08-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    assert!(out.invoices.is_empty());
    assert_eq!(client.calls(), 1);
    assert!(
        client.requests.lock().unwrap()[0]
            .1
            .ends_with("date=2026-10-23")
    );
}

#[tokio::test]
async fn openrouter_maps_refusals_throttling_and_malformed_answers() {
    // A regular key is refused by the management endpoint.
    let client = Recorded::script(vec![(403, "{}")]);
    let (out, _) = run(
        &OpenRouter,
        OPENROUTER_KEY,
        client.clone(),
        "2026-10-23",
        "2026-10-23",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 403, .. })));
    assert_eq!(client.calls(), 1);
    let client = Recorded::script(vec![(401, "{}")]);
    let (out, _) = run(
        &OpenRouter,
        OPENROUTER_KEY,
        client,
        "2026-10-23",
        "2026-10-23",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Refused { status: 401, .. })));
    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let (out, pause) = run(
        &OpenRouter,
        OPENROUTER_KEY,
        client.clone(),
        "2026-10-23",
        "2026-10-23",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(client.calls(), 3);
    assert_eq!(pause.0.lock().unwrap().len(), 2);
    let client = Recorded::script(vec![(500, "{}"), (500, "{}"), (500, "{}")]);
    let (out, _) = run(
        &OpenRouter,
        OPENROUTER_KEY,
        client,
        "2026-10-23",
        "2026-10-23",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Status { status: 500, .. })));
    let client = Recorded::script(vec![(200, r#"{"nothing":true}"#)]);
    let (out, _) = run(
        &OpenRouter,
        OPENROUTER_KEY,
        client,
        "2026-10-23",
        "2026-10-23",
    )
    .await;
    assert!(matches!(out, Err(VendorError::Malformed { .. })));
}

#[test]
fn openrouter_is_registered_with_its_host_and_management_key() {
    let connector = ledgers_connect::connector::by_id("openrouter").expect("registered");
    assert_eq!(connector.allowed_hosts(), &["openrouter.ai"]);
    assert!(
        connector.credential_fields()[0]
            .label
            .contains("Management")
    );
    assert!(OpenRouter.validate_config(&json!({})).is_ok());
}
