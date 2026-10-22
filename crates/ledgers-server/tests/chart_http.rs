//! HTTP tests for the chart-of-accounts commands: a person (owner session)
//! manages the chart; an agent key gets 403 unless its policy allows chart
//! changes, and every refusal is audited.

mod common;

use common::http::{APP_ORIGIN, Auth, call};
use common::{Fixture, Method, fixture_with_options, json, owner_session};
use cratefield_core::Statement;
use cratefield_core::axum::http::StatusCode;
use ledgers_commands::auth::Tier;
use ledgers_policy::store::{deserialize_key, serialize_key};
use sea_query::Value as SeaValue;

/// A fixture whose person session and origin are wired, with the owner
/// signed in.
fn chart_fixture() -> Fixture {
    let fx = fixture_with_options(Vec::new(), None, vec![APP_ORIGIN.to_owned()]);
    owner_session(&fx);
    fx
}

/// Lets the unlimited post-tier key manage the chart: the policy row's flag.
fn allow_chart_changes_for_the_post_key(fx: &Fixture) {
    pollster::block_on(async {
        let rows = fx
            .deps
            .db
            .query(&Statement::with_values(
                "SELECT id, policy FROM agent_key WHERE company_id = ?".to_owned(),
                vec![SeaValue::from(common::COMPANY)],
            ))
            .await
            .expect("read the key rows");
        let mut changed = 0;
        for row in &rows.rows {
            let policy: String = row.get("policy").expect("policy column");
            let mut key = deserialize_key(&policy).expect("a stored key policy");
            // The key the fixture issues with no limit in any currency.
            if key.tier == Tier::PostWithinLimits && key.limits.is_empty() {
                key.chart_changes = true;
                let id: String = row.get("id").expect("id column");
                let updated = serialize_key(&key).expect("serialise the key");
                fx.deps
                    .db
                    .execute(&Statement::with_values(
                        "UPDATE agent_key SET policy = ? WHERE company_id = ? AND id = ?"
                            .to_owned(),
                        vec![
                            SeaValue::from(updated),
                            SeaValue::from(common::COMPANY),
                            SeaValue::from(id),
                        ],
                    ))
                    .await
                    .expect("store the key policy");
                changed += 1;
            }
        }
        assert_eq!(changed, 1, "exactly the unlimited key is granted the flag");
    });
}

fn audit_mentions(fx: &Fixture, needle: &str) -> i64 {
    pollster::block_on(async {
        let rows = fx
            .deps
            .db
            .query(&Statement::with_values(
                "SELECT COUNT(*) AS n FROM audit_event WHERE company_id = ? AND payload LIKE ?"
                    .to_owned(),
                vec![
                    SeaValue::from(common::COMPANY),
                    SeaValue::from(format!("%{needle}%")),
                ],
            ))
            .await
            .expect("count audit rows");
        rows.first()
            .and_then(|row| row.get::<i64>("n"))
            .unwrap_or_default()
    })
}

fn create_body(number: &str, name: &str, account_type: &str) -> serde_json::Value {
    serde_json::json!({ "number": number, "name": name, "type": account_type })
}

#[test]
fn an_agent_key_without_the_chart_permission_gets_403_and_the_refusal_is_audited() {
    let fx = chart_fixture();
    let reply = call(
        &fx,
        Method::POST,
        "/v1/accounts",
        Auth::Bearer(&fx.keys.full),
        false,
        Some(create_body("1990", "Agent account", "asset")),
    );
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text);
    assert!(
        audit_mentions(&fx, "accounts.create") >= 1 && audit_mentions(&fx, "denied") >= 1,
        "the refused attempt is recorded as a denied event"
    );
    assert_eq!(
        pollster::block_on(fx.deps.db.query(&Statement::with_values(
            "SELECT number FROM account WHERE company_id = ? AND number = ?".to_owned(),
            vec![SeaValue::from(common::COMPANY), SeaValue::from("1990")],
        )))
        .expect("read")
        .rows
        .len(),
        0,
        "no account was created"
    );
}

#[test]
fn an_agent_key_with_the_chart_permission_may_create_and_archive() {
    let fx = chart_fixture();
    allow_chart_changes_for_the_post_key(&fx);
    let created = call(
        &fx,
        Method::POST,
        "/v1/accounts",
        Auth::Bearer(&fx.keys.unlimited),
        false,
        Some(create_body("1991", "Granted account", "expense")),
    );
    assert_eq!(created.status, StatusCode::OK, "{}", created.text);
    let archived = call(
        &fx,
        Method::POST,
        "/v1/accounts/1991/archive",
        Auth::Bearer(&fx.keys.unlimited),
        false,
        Some(serde_json::json!({})),
    );
    assert_eq!(archived.status, StatusCode::OK, "{}", archived.text);
    assert_eq!(archived.json()["account"]["archived"], true);
}

#[test]
fn a_read_key_is_refused_a_chart_write_before_anything_is_stored() {
    let fx = chart_fixture();
    let reply = call(
        &fx,
        Method::POST,
        "/v1/accounts",
        Auth::Bearer(&fx.keys.read),
        false,
        Some(create_body("1992", "Read key", "asset")),
    );
    assert!(
        reply.status == StatusCode::FORBIDDEN,
        "a read key cannot write the chart: {}",
        reply.text
    );
}

#[test]
fn a_person_creates_renames_archives_and_restores_with_the_owner_session() {
    let fx = chart_fixture();
    let created = call(
        &fx,
        Method::POST,
        "/v1/accounts",
        Auth::Session(common::OWNER_SESSION),
        true,
        Some(create_body("1100", "Petty cash box", "asset")),
    );
    assert_eq!(created.status, StatusCode::OK, "{}", created.text);
    let account = &created.json()["account"];
    assert_eq!(account["display_name"], "Petty cash box");
    assert_eq!(account["account_type"], "asset");
    assert_eq!(account["archived"], false);

    // The same number again is a conflict, not a second row.
    let again = call(
        &fx,
        Method::POST,
        "/v1/accounts",
        Auth::Session(common::OWNER_SESSION),
        true,
        Some(create_body("1100", "Again", "asset")),
    );
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);

    let renamed = call(
        &fx,
        Method::POST,
        "/v1/accounts/1100/update",
        Auth::Session(common::OWNER_SESSION),
        true,
        Some(serde_json::json!({ "name": "Front till", "tags": ["cash"] })),
    );
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text);
    assert_eq!(renamed.json()["account"]["display_name"], "Front till");
    assert_eq!(
        renamed.json()["account"]["tags"],
        serde_json::json!(["cash"])
    );

    let archived = call(
        &fx,
        Method::POST,
        "/v1/accounts/1100/archive",
        Auth::Session(common::OWNER_SESSION),
        true,
        Some(serde_json::json!({})),
    );
    assert_eq!(archived.status, StatusCode::OK, "{}", archived.text);
    assert_eq!(archived.json()["account"]["archived"], true);

    let restored = call(
        &fx,
        Method::POST,
        "/v1/accounts/1100/restore",
        Auth::Session(common::OWNER_SESSION),
        true,
        Some(serde_json::json!({})),
    );
    assert_eq!(restored.status, StatusCode::OK, "{}", restored.text);
    assert_eq!(restored.json()["account"]["archived"], false);

    assert!(
        audit_mentions(&fx, "account.update") >= 1,
        "the rename is in the audit log"
    );
}

#[test]
fn archive_with_a_balance_is_refused_and_the_detail_names_the_rule() {
    let fx = chart_fixture();
    let posted = fx.post("chart-bal-1", "5010", "4000", "100000");
    assert_eq!(posted.status.as_u16(), 200, "{}", posted.json());
    let refused = call(
        &fx,
        Method::POST,
        "/v1/accounts/4000/archive",
        Auth::Session(common::OWNER_SESSION),
        true,
        Some(serde_json::json!({})),
    );
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    let body = refused.json();
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("non-zero balance"),
        "{body}"
    );
}

#[test]
fn accounts_get_returns_the_balance_and_the_entries_that_touched_it() {
    let fx = chart_fixture();
    let posted = fx.post("chart-get-1", "5010", "4000", "100000");
    assert_eq!(posted.status.as_u16(), 200, "{}", posted.json());
    let got = fx.send(Method::GET, "/v1/accounts/4000", &fx.keys.read, None);
    assert_eq!(got.status.as_u16(), 200, "{}", got.json());
    let body = json(&got);
    assert_eq!(body["account"]["number"], "4000");
    assert_eq!(body["account"]["balances"][0]["minor"], "-100000");
    let activity = body["activity"].as_array().cloned().unwrap_or_default();
    assert_eq!(activity.len(), 1, "{body}");
    assert_eq!(activity[0]["minor"], "-100000");
    assert_eq!(activity[0]["currency"], "EUR");
    assert!(
        !activity[0]["entry_id"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );

    let missing = fx.send(Method::GET, "/v1/accounts/9999", &fx.keys.read, None);
    assert_eq!(missing.status.as_u16(), 404, "{}", missing.json());
}
