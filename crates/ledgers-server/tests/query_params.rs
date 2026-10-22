//! HTTP tests for GET parameters: the path and query values of a GET command
//! are converted to the types its input schema names (issue: the Bills list
//! sends `?limit=100`, which an integer field refused as the string "100").

mod common;

use common::{fixture, json};
use cratefield_core::axum::http::Method;
use serde_json::json;

#[test]
fn bills_list_accepts_an_integer_limit() {
    let fx = fixture();
    let response = fx.send(Method::GET, "/v1/bills?limit=100", &fx.keys.read, None);
    assert_eq!(response.status.as_u16(), 200, "{}", json(&response));
    assert!(json(&response)["bills"].is_array());
}

#[test]
fn bills_list_refuses_a_non_integer_limit_naming_the_field() {
    let fx = fixture();
    let response = fx.send(Method::GET, "/v1/bills?limit=abc", &fx.keys.read, None);
    assert_eq!(response.status.as_u16(), 422, "{}", json(&response));
    let body = json(&response);
    assert!(
        body["type"]
            .as_str()
            .unwrap_or_default()
            .ends_with("invalid-parameter"),
        "{body}"
    );
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("`limit`"),
        "{body}"
    );
}

#[test]
fn entries_list_accepts_limit_and_after() {
    let fx = fixture();
    assert_eq!(
        fx.post("query-1", "5010", "4000", "100000").status.as_u16(),
        200
    );
    assert_eq!(
        fx.post("query-2", "5010", "4000", "200000").status.as_u16(),
        200
    );

    let first = fx.send(Method::GET, "/v1/entries?limit=1", &fx.keys.read, None);
    assert_eq!(first.status.as_u16(), 200, "{}", json(&first));
    let page = json(&first)["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(page.len(), 1, "limit=1 returns one entry");
    let after = page[0]["entry_id"].as_str().expect("entry id").to_owned();

    let next = fx.send(
        Method::GET,
        &format!("/v1/entries?limit=10&after={after}"),
        &fx.keys.read,
        None,
    );
    assert_eq!(next.status.as_u16(), 200, "{}", json(&next));
}

#[test]
fn accounts_get_keeps_a_numeric_account_number_a_string() {
    // The dev chart seeds account 1030: a numeric-looking number must reach the
    // command as the string "1030", or the account lookup refuses the request.
    let fx = fixture();
    let response = fx.send(Method::GET, "/v1/accounts/1030", &fx.keys.read, None);
    assert_eq!(response.status.as_u16(), 200, "{}", json(&response));
    assert_eq!(json(&response)["account"]["number"], json!("1030"));
}

#[test]
fn documents_list_accepts_an_integer_limit() {
    let fx = fixture();
    let response = fx.send(Method::GET, "/v1/documents?limit=5", &fx.keys.read, None);
    assert_eq!(response.status.as_u16(), 200, "{}", json(&response));
}
