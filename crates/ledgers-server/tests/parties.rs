//! HTTP tests for customers and vendors (issue #44): the party routes on the
//! real server stack over SQLite. VIES is off in this fixture, so no test
//! reaches the network.
mod common;

use common::fixture;
use cratefield_core::axum::http::{Method, StatusCode};
use serde_json::json;

#[test]
fn party_routes_create_filter_read_and_balance() {
    let fx = fixture();
    let created = fx.send(
        Method::POST,
        "/v1/parties",
        &fx.keys.draft,
        Some(
            &json!({
                "id": "party-http",
                "name": "Halden Works GmbH",
                "roles": "vendor",
                "vat_id": "DE136695976",
                "addresses": [{"line1": "Musterstrasse 1", "city": "Berlin", "country": "de"}],
                "contacts": [{"name": "Ada", "email": "ada@example.com"}]
            })
            .to_string(),
        ),
    );
    assert_eq!(
        created.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(created.body().as_ref())
    );
    assert_eq!(created.json()["party"]["addresses"][0]["country"], "DE");
    assert_eq!(created.json()["party"]["needs_review"], false);

    let found = fx.send(Method::GET, "/v1/parties?q=halden", &fx.keys.read, None);
    assert_eq!(
        found.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(found.body().as_ref())
    );
    assert_eq!(found.json()["total"], 1);
    assert_eq!(found.json()["parties"][0]["id"], "party-http");

    let reviewed = fx.send(
        Method::GET,
        "/v1/parties?needs_review=true&roles=vendor",
        &fx.keys.read,
        None,
    );
    assert_eq!(
        reviewed.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(reviewed.body().as_ref())
    );
    assert_eq!(
        reviewed.json()["total"],
        0,
        "a party created by hand needs no review"
    );

    let one = fx.send(Method::GET, "/v1/parties/party-http", &fx.keys.read, None);
    assert_eq!(
        one.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(one.body().as_ref())
    );
    assert_eq!(one.json()["party"]["name"], "Halden Works GmbH");
    assert_eq!(one.json()["bill_count"], 0);
    assert_eq!(
        one.json()["vies_enabled"],
        false,
        "VIES is off in this fixture"
    );

    let balances = fx.send(Method::GET, "/v1/parties/balances", &fx.keys.read, None);
    assert_eq!(
        balances.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(balances.body().as_ref())
    );
    assert_eq!(balances.json()["balances"], json!([]));
}

#[test]
fn party_routes_validate_update_check_and_refuse_a_reader() {
    let fx = fixture();
    let created = fx.send(
        Method::POST,
        "/v1/parties",
        &fx.keys.draft,
        Some(
            &json!({
                "id": "party-http",
                "name": "Halden Works GmbH",
                "roles": "vendor",
                "vat_id": "DE136695976",
                "addresses": [{"line1": "Musterstrasse 1", "city": "Berlin", "country": "de"}],
                "contacts": [{"name": "Ada", "email": "ada@example.com"}]
            })
            .to_string(),
        ),
    );
    assert_eq!(
        created.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(created.body().as_ref())
    );
    assert_eq!(created.json()["party"]["addresses"][0]["country"], "DE");
    assert_eq!(created.json()["party"]["needs_review"], false);

    let refused = fx.send(
        Method::POST,
        "/v1/parties/validate",
        &fx.keys.draft,
        Some(&json!({"vat_id": "DE000"}).to_string()),
    );
    assert_eq!(
        refused.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(refused.body().as_ref())
    );
    assert_eq!(refused.json()["valid"], false);
    assert_eq!(refused.json()["errors"][0]["field"], "vat_id");

    let updated = fx.send(
        Method::POST,
        "/v1/parties/party-http/update",
        &fx.keys.draft,
        Some(&json!({"payment_terms_days": 30}).to_string()),
    );
    assert_eq!(
        updated.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(updated.body().as_ref())
    );
    assert_eq!(updated.json()["party"]["payment_terms_days"], 30);

    let vies_off = fx.send(
        Method::POST,
        "/v1/parties/party-http/check-vat",
        &fx.keys.draft,
        Some(&json!({"country_code": "DE", "vat_number": "136695976"}).to_string()),
    );
    assert_eq!(
        vies_off.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        String::from_utf8_lossy(vies_off.body().as_ref())
    );
    assert!(
        String::from_utf8_lossy(vies_off.body().as_ref()).contains("LEDGERS_VIES"),
        "{}",
        String::from_utf8_lossy(vies_off.body().as_ref())
    );

    let read_only = fx.send(
        Method::POST,
        "/v1/parties",
        &fx.keys.read,
        Some(&json!({"id": "party-nope", "name": "Nope"}).to_string()),
    );
    assert_eq!(
        read_only.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(read_only.body().as_ref())
    );
}
