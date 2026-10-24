//! The Jira Cloud connector against recorded answers (issue #142). No test reaches
//! Atlassian: every answer is a fixture, and every request is recorded. Jira books
//! no invoice: the tests pin that, and that the planned cost is a note only.

mod support;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ledgers_connect::connector::{Connector, SyncContext, by_id};
use ledgers_connect::http::VendorError;
use ledgers_connect::jira::{Jira, site_host};
use ledgers_connect::model::SyncOutput;
use serde_json::{Value, json};
use std::sync::Arc;
use support::{Recorded, Route, vendor};

const NOW: &str = "2026-10-25T10:00:00Z";
const SITE: &str = "example-team.atlassian.net";
const EMAIL: &str = "jira-reader@example.test";
const TOKEN: &str = "jira_api_token_test_not_real";
const USERS: &str = include_str!("../fixtures/jira_users_search.json");
const ROLES: &str = include_str!("../fixtures/jira_application_roles.json");
const PROJECTS: &str = include_str!("../fixtures/jira_project_search.json");

fn credential() -> String {
    json!({ "email": EMAIL, "api_token": TOKEN }).to_string()
}

fn config(price: Option<&str>) -> Value {
    let mut config = json!({ "site": SITE, "expense_account": "6300" });
    if let Some(price) = price {
        config["price_per_seat"] = json!(price);
        config["price_currency"] = json!("USD");
    }
    config
}

fn routes() -> Vec<Route> {
    vec![
        Route {
            url: "/rest/api/3/users/search",
            body: "",
            status: 200,
            answer: USERS.to_owned(),
        },
        Route {
            url: "/rest/api/3/applicationrole",
            body: "",
            status: 200,
            answer: ROLES.to_owned(),
        },
        Route {
            url: "/rest/api/3/project/search",
            body: "",
            status: 200,
            answer: PROJECTS.to_owned(),
        },
    ]
}

async fn sync_with(client: Arc<Recorded>, config: &Value) -> Result<SyncOutput, VendorError> {
    let (http, _pause) = vendor(client);
    Jira.sync(&SyncContext {
        http: &http,
        credential: &credential(),
        now: NOW,
        config,
        since: "2026-07-01",
        until: "2026-10-25",
    })
    .await
}

fn joined(out: &SyncOutput) -> String {
    out.notes.join("\n")
}

#[test]
fn jira_is_registered_with_its_fields() {
    let connector = by_id("jira").expect("registered");
    let names: Vec<&str> = connector
        .credential_fields()
        .iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(names, ["email", "api_token"]);
    assert_eq!(connector.display_name(), "Jira (Atlassian)");
}

#[test]
fn the_site_must_be_an_atlassian_cloud_host() {
    assert_eq!(
        site_host(" Example-Team.atlassian.net ").as_deref(),
        Ok("example-team.atlassian.net")
    );
    for bad in [
        "example.test",
        "https://example.atlassian.net",
        "evil.test.atlassian.net.example.test",
        "-team.atlassian.net",
        ".atlassian.net",
        "team_one.atlassian.net",
    ] {
        assert!(site_host(bad).is_err(), "{bad} must be refused");
    }
    assert!(
        Jira.validate_config(&json!({"site": "example.test", "expense_account": "6300"}))
            .is_err()
    );
}

#[tokio::test]
async fn sends_basic_auth_to_the_site_host_only() {
    let client = Recorded::new(routes());
    sync_with(client.clone(), &config(None))
        .await
        .expect("sync");
    let expected = format!("Basic {}", STANDARD.encode(format!("{EMAIL}:{TOKEN}")));
    let log = client.header_log.lock().unwrap();
    for headers in log.iter() {
        let auth = headers
            .iter()
            .find(|(n, _)| n == "authorization")
            .map(|(_, v)| v.clone());
        assert_eq!(auth.as_deref(), Some(expected.as_str()));
    }
    let requests = client.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .all(|(_, url, _)| url.starts_with(&format!("https://{SITE}/rest/api/3/")))
    );
}

#[tokio::test]
async fn counts_active_atlassian_seats_and_reports_products_and_projects() {
    let out = sync_with(Recorded::new(routes()), &config(None))
        .await
        .expect("sync");
    let notes = joined(&out);
    assert!(
        notes.contains("Seats in use on example-team.atlassian.net: 2 active Atlassian account(s)"),
        "{notes}"
    );
    assert!(
        notes.contains("Jira Software: 2 of 10 seats in use"),
        "{notes}"
    );
    assert!(
        notes.contains("Jira Service Management: 0 in use, unlimited seats"),
        "{notes}"
    );
    assert!(notes.contains("2 project(s) read"), "{notes}");
}

#[tokio::test]
async fn never_books_an_invoice_and_projects_become_usage_for_the_current_month() {
    let out = sync_with(Recorded::new(routes()), &config(Some("8.60")))
        .await
        .expect("sync");
    assert!(out.invoices.is_empty(), "no Atlassian cost is booked");
    assert!(out.entries.is_empty());
    let keys: Vec<(&str, &str)> = out
        .usage
        .iter()
        .map(|u| (u.period.as_str(), u.resource_id.as_str()))
        .collect();
    assert_eq!(keys, [("2026-10", "OPS"), ("2026-10", "SQZ")]);
    assert!(out.usage.iter().all(|u| u.resource_kind == "jira-project"));
}

#[tokio::test]
async fn the_planned_cost_is_a_note_from_the_typed_price_and_says_it_is_not_booked() {
    let out = sync_with(Recorded::new(routes()), &config(Some("8.60")))
        .await
        .expect("sync");
    let notes = joined(&out);
    assert!(
        notes.contains("Planned monthly cost 17.20 USD: 2 seat(s) x 8.600000 USD per seat"),
        "{notes}"
    );
    assert!(
        notes.contains("not a charge, and it is not booked"),
        "{notes}"
    );
    assert!(notes.contains("bank feed or the invoice inbox"), "{notes}");
    let none = sync_with(Recorded::new(routes()), &config(None))
        .await
        .expect("sync");
    assert!(joined(&none).contains("No price per seat is set"));
}

#[test]
fn a_price_needs_a_currency_and_a_decimal() {
    assert!(Jira.validate_config(&config(Some("8.60"))).is_ok());
    assert!(Jira.validate_config(&config(Some("eight"))).is_err());
    let mut no_currency = config(Some("8.60"));
    no_currency
        .as_object_mut()
        .unwrap()
        .remove("price_currency");
    assert!(Jira.validate_config(&no_currency).is_err());
}

#[tokio::test]
async fn a_token_without_admin_rights_loses_only_the_product_counts() {
    let client = Recorded::script(vec![(200, USERS), (403, "{}"), (200, PROJECTS)]);
    let out = sync_with(client.clone(), &config(None))
        .await
        .expect("sync still answers");
    assert!(joined(&out).contains("Product seat counts not read"));
    assert!(joined(&out).contains("Seats in use"));
}

#[tokio::test]
async fn a_refused_token_fails_the_sync_and_is_not_retried() {
    for status in [401_u16, 403] {
        let client = Recorded::script(vec![(status, "{}")]);
        let out = sync_with(client.clone(), &config(None)).await;
        assert!(matches!(out, Err(VendorError::Refused { status: s, .. }) if s == status));
        assert_eq!(client.calls(), 1);
    }
}

#[tokio::test]
async fn throttling_is_retried_with_backoff_then_reported() {
    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let (http, pause) = vendor(client.clone());
    let out = Jira
        .sync(&SyncContext {
            http: &http,
            credential: &credential(),
            now: NOW,
            config: &config(None),
            since: "2026-07-01",
            until: "2026-10-25",
        })
        .await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(client.calls(), 3);
    assert_eq!(pause.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn server_errors_are_retried_then_reported() {
    let client = Recorded::script(vec![(502, "{}"), (503, "{}"), (500, "{}")]);
    let out = sync_with(client.clone(), &config(None)).await;
    assert!(matches!(out, Err(VendorError::Status { status: 500, .. })));
    assert_eq!(client.calls(), 3);
}

#[tokio::test]
async fn a_repeat_sync_gives_the_same_usage() {
    let first = sync_with(Recorded::new(routes()), &config(Some("8.60")))
        .await
        .expect("first");
    let second = sync_with(Recorded::new(routes()), &config(Some("8.60")))
        .await
        .expect("second");
    assert_eq!(first.usage, second.usage);
    assert_eq!(first.invoices, second.invoices);
}
