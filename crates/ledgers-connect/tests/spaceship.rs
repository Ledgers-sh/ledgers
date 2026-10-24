//! The Spaceship connector against recorded answers (issue #142). No test reaches
//! the registrar. Spaceship has no billing endpoint, so these tests check that a
//! sync books nothing, that the domain list is read as metadata, and that the
//! renewals window and the refusals behave.

mod support;

use ledgers_connect::connector::{Connector, SyncContext};
use ledgers_connect::http::VendorError;
use ledgers_connect::model::{DomainRecord, SyncOutput};
use ledgers_connect::spaceship::{RENEWAL_WINDOW_DAYS, Spaceship, renewals_within};
use serde_json::{Value, json};
use std::sync::Arc;
use support::{Recorded, Route, vendor};

const NOW: &str = "2026-10-25T10:00:00Z";
const TODAY: &str = "2026-10-25";
const PAGE1: &str = include_str!("../fixtures/spaceship_domains_page1.json");
const KEY: &str = "ss_key_test_not_real";
const SECRET: &str = "ss_secret_test_not_real";

fn credential() -> String {
    json!({"api_key": KEY, "api_secret": SECRET}).to_string()
}

async fn sync_with(client: Arc<Recorded>, credential: &str) -> Result<SyncOutput, VendorError> {
    let (http, _pause) = vendor(client);
    let config = json!({});
    Spaceship
        .sync(&SyncContext {
            http: &http,
            credential,
            now: NOW,
            config: &config,
            since: TODAY,
            until: TODAY,
        })
        .await
}

fn page_route() -> Vec<Route> {
    vec![Route {
        url: "spaceship.dev/api/v1/domains",
        body: "",
        status: 200,
        answer: PAGE1.to_owned(),
    }]
}

fn domain_page(first: usize, count: usize, total: usize) -> String {
    let items: Vec<Value> = (first..first + count)
        .map(|i| {
            json!({
                "name": format!("site-{i:03}.in"),
                "expirationDate": "2028-01-15T00:00:00Z",
                "autoRenew": i % 2 == 0,
                "lifecycleStatus": "active"
            })
        })
        .collect();
    json!({"items": items, "total": total}).to_string()
}

#[tokio::test]
async fn sends_the_key_and_secret_headers_and_the_domain_list_query() {
    let client = Recorded::new(page_route());
    sync_with(client.clone(), &credential())
        .await
        .expect("the sync answers");
    let log = client.header_log.lock().unwrap();
    let first = &log[0];
    let header = |name: &str| {
        first
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(header("x-api-key").as_deref(), Some(KEY));
    assert_eq!(header("x-api-secret").as_deref(), Some(SECRET));
    assert!(header("authorization").is_none(), "no bearer token is sent");
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests[0].0, "GET");
    assert_eq!(
        requests[0].1,
        "https://spaceship.dev/api/v1/domains?take=100&skip=0&orderBy=name"
    );
}

#[tokio::test]
async fn reads_the_domain_list_as_metadata_and_books_nothing() {
    let out = sync_with(Recorded::new(page_route()), &credential())
        .await
        .expect("the sync answers");
    assert!(out.invoices.is_empty(), "no bill is made");
    assert!(out.usage.is_empty(), "no usage row is made");
    assert!(out.entries.is_empty(), "no journal entry is made");
    let names: Vec<&str> = out.domains.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["squeez.in", "factory0.ventures", "lapsed.example"]);
    let squeezin = &out.domains[0];
    assert_eq!(squeezin.expires_on, "2026-11-20");
    assert!(squeezin.auto_renew);
    assert_eq!(squeezin.status.as_deref(), Some("active"));
    assert!(!out.domains[1].auto_renew);
    assert!(
        out.notes
            .iter()
            .any(|note| note.contains("nothing is booked")),
        "the run says why nothing is booked: {:?}",
        out.notes
    );
    assert!(
        out.notes
            .iter()
            .any(|note| note.contains("1 domain(s) had no name or expiry")),
        "the row without a name is counted, not guessed: {:?}",
        out.notes
    );
}

#[tokio::test]
async fn pages_by_skip_until_the_total_is_read() {
    let client = Recorded::script(vec![
        (200, &domain_page(0, 100, 101)),
        (200, &domain_page(100, 1, 101)),
    ]);
    let out = sync_with(client.clone(), &credential())
        .await
        .expect("both pages read");
    assert_eq!(client.calls(), 2, "a second page is requested");
    let requests = client.requests.lock().unwrap();
    assert!(requests[1].1.contains("skip=100"), "{}", requests[1].1);
    assert_eq!(out.domains.len(), 101);
}

#[tokio::test]
async fn a_repeated_sync_gives_the_same_metadata_and_books_nothing_twice() {
    let first = sync_with(Recorded::new(page_route()), &credential())
        .await
        .expect("first sync");
    let second = sync_with(Recorded::new(page_route()), &credential())
        .await
        .expect("second sync");
    assert_eq!(first.domains, second.domains);
    assert!(second.invoices.is_empty() && second.usage.is_empty() && second.entries.is_empty());
}

#[tokio::test]
async fn the_renewals_window_lists_the_expiring_and_the_lapsed_soonest_first() {
    let out = sync_with(Recorded::new(page_route()), &credential())
        .await
        .expect("the sync answers");
    let due: Vec<DomainRecord> = renewals_within(&out.domains, TODAY, RENEWAL_WINDOW_DAYS);
    let names: Vec<&str> = due.iter().map(|d| d.name.as_str()).collect();
    // lapsed.example expired on 2026-10-01 (still listed), squeez.in is due on
    // 2026-11-20 (inside the 60 days), factory0.ventures runs to 2027-08-01.
    assert_eq!(names, ["lapsed.example", "squeez.in"]);
}

#[tokio::test]
async fn refusals_are_named_and_not_retried() {
    let client = Recorded::script(vec![(401, "{}")]);
    let out = sync_with(client.clone(), &credential()).await;
    assert!(matches!(out, Err(VendorError::Refused { status: 401, .. })));
    assert_eq!(client.calls(), 1, "a 401 is not retried");

    let client = Recorded::script(vec![(403, "{}")]);
    let out = sync_with(client.clone(), &credential()).await;
    assert!(matches!(out, Err(VendorError::Refused { status: 403, .. })));
    assert_eq!(client.calls(), 1, "a 403 is not retried");
}

#[tokio::test]
async fn throttling_and_server_errors_are_retried_three_times_then_reported() {
    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let out = sync_with(client.clone(), &credential()).await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(client.calls(), 3, "three attempts on a 429");

    let client = Recorded::script(vec![(500, "{}"), (503, "{}"), (502, "{}")]);
    let out = sync_with(client.clone(), &credential()).await;
    assert!(matches!(
        out,
        Err(VendorError::Status {
            status: 502 | 503 | 500,
            ..
        })
    ));
    assert_eq!(client.calls(), 3, "three attempts on a 5xx");
}

#[tokio::test]
async fn a_recovered_throttle_reads_the_list() {
    let client = Recorded::script(vec![(429, "{}"), (200, PAGE1)]);
    let out = sync_with(client.clone(), &credential())
        .await
        .expect("the retry answers");
    assert_eq!(client.calls(), 2);
    assert_eq!(out.domains.len(), 3);
}

#[tokio::test]
async fn a_malformed_list_is_reported_with_its_field() {
    let client = Recorded::script(vec![(200, r#"{"total": 0}"#)]);
    let out = sync_with(client, &credential()).await;
    match out {
        Err(VendorError::Malformed { detail, .. }) => {
            assert!(detail.contains("items"), "{detail}");
        }
        other => panic!("expected a malformed error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_credential_missing_a_field_names_the_field_and_never_echoes_the_value() {
    let client = Recorded::new(page_route());
    let only_key = json!({"api_key": KEY}).to_string();
    let out = sync_with(client.clone(), &only_key).await;
    match out {
        Err(VendorError::Malformed { detail, .. }) => {
            assert!(detail.contains("api_secret"), "{detail}");
            assert!(!detail.contains(KEY));
        }
        other => panic!("expected the missing field to be named, got {other:?}"),
    }
    assert_eq!(client.calls(), 0, "no request is sent without both fields");
}

#[test]
fn the_connector_declares_two_fields_and_its_host() {
    let fields: Vec<&str> = Spaceship
        .credential_fields()
        .iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(fields, ["api_key", "api_secret"]);
    assert_eq!(Spaceship.allowed_hosts(), &["spaceship.dev"]);
    assert!(Spaceship.validate_config(&json!({})).is_ok());
}
