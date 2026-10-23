//! The connections commands through the server's dispatch (issue #142): a token
//! is sealed on the way in and never comes back out, a server with no key
//! refuses to hold one, an agent cannot create or sync a connection, and a
//! sync with no outbound client is recorded as a failed run.

mod common;

use common::fixture;
use ledgers_commands::auth::{Limits, Principal, Role, Tier};
use ledgers_connect::secret::ConnectKey;
use ledgers_server::dispatch::ServerDispatch;
use ledgers_server::routes::{Dispatch, Presented};
use serde_json::{Value, json};

const KEY: &str = "0123456789abcdef0123456789abcdef-test-only-key";
const TOKEN: &str = "cfat_SERVER_TEST_TOKEN_NEVER_LEAVES_THE_SERVER";

fn owner() -> Presented {
    Presented::Principal(Principal::Person {
        person_id: "person-owner".to_owned(),
        role: Role::Owner,
    })
}

fn agent() -> Presented {
    Presented::Principal(Principal::Agent {
        key_id: "agent-key-test".to_owned(),
        tier: Tier::Draft,
        limits: Limits::new(),
    })
}

/// The unit the connections default to, in the fixture company.
fn seed_unit(fx: &common::Fixture) {
    pollster::block_on(fx.deps.db.execute(&cratefield_core::Statement::with_values(
        "INSERT INTO dimension_value (company_id, kind, code, name, own_books, created_at) VALUES (?, 'unit', 'FZ', 'Factory Zero', 1, ?)".to_owned(),
        vec![
            sea_query::Value::from(common::COMPANY),
            sea_query::Value::from(common::NOW),
        ],
    )))
    .expect("the unit is seeded");
}

fn create_body(name: &str) -> Value {
    json!({
        "connector": "cloudflare",
        "name": name,
        "config": {"account_id": "0123456789abcdef0123456789abcdef", "expense_account": "6100"},
        "default_unit": "FZ",
        "token": TOKEN,
    })
}

#[test]
fn a_token_is_sealed_and_never_returned() {
    let fx = fixture();
    seed_unit(&fx);
    let key = ConnectKey::from_secret(KEY).unwrap();
    let dispatch = ServerDispatch::new(common::COMPANY).with_connect_key(Some(key.clone()));
    let created = pollster::block_on(dispatch.dispatch(
        &fx.deps,
        "connections.create",
        create_body("Server test"),
        "req-create",
        Some(owner()),
    ))
    .expect("created");
    let text = created.to_string();
    assert!(
        !text.contains(TOKEN),
        "the response never carries the token"
    );
    assert!(!text.contains("v1."), "nor its sealed form");
    assert_eq!(created["has_credential"], true);

    let id = created["id"].as_str().unwrap().to_owned();
    let sealed = pollster::block_on(ledgers_connect::store::sealed_credential(
        fx.deps.db.as_ref(),
        common::COMPANY,
        &id,
    ))
    .unwrap()
    .expect("stored");
    assert_eq!(
        key.open(&sealed, &id).unwrap(),
        TOKEN,
        "stored sealed to this connection"
    );

    let listed = pollster::block_on(dispatch.dispatch(
        &fx.deps,
        "connections.list",
        json!({}),
        "req-list",
        Some(owner()),
    ))
    .unwrap();
    let listed_text = listed.to_string();
    assert!(!listed_text.contains(TOKEN));
    assert!(!listed_text.contains("v1."));
    assert_eq!(listed["connections"][0]["has_credential"], true);
}

#[test]
fn a_server_without_a_key_refuses_to_hold_a_token() {
    let fx = fixture();
    let dispatch = ServerDispatch::new(common::COMPANY);
    let result = pollster::block_on(dispatch.dispatch(
        &fx.deps,
        "connections.create",
        create_body("No key"),
        "req-nokey",
        Some(owner()),
    ));
    let problem = result.expect_err("refused");
    assert!(
        problem
            .detail
            .unwrap_or_default()
            .contains("LEDGERS_CONNECT_KEY"),
        "the refusal names the setting"
    );
}

#[test]
fn an_agent_cannot_create_or_sync_a_connection() {
    let fx = fixture();
    seed_unit(&fx);
    let dispatch = ServerDispatch::new(common::COMPANY)
        .with_connect_key(Some(ConnectKey::from_secret(KEY).unwrap()));
    assert!(
        pollster::block_on(dispatch.dispatch(
            &fx.deps,
            "connections.create",
            create_body("Agent"),
            "req-agent",
            Some(agent())
        ))
        .is_err()
    );
    assert!(
        pollster::block_on(dispatch.dispatch(
            &fx.deps,
            "connections.sync_now",
            json!({"connection_id": "anything"}),
            "req-agent-sync",
            Some(agent()),
        ))
        .is_err()
    );
}

#[test]
fn a_sync_without_an_outbound_client_is_recorded_as_a_failed_run() {
    let fx = fixture();
    seed_unit(&fx);
    let key = ConnectKey::from_secret(KEY).unwrap();
    let dispatch = ServerDispatch::new(common::COMPANY).with_connect_key(Some(key));
    let created = pollster::block_on(dispatch.dispatch(
        &fx.deps,
        "connections.create",
        create_body("Sync me"),
        "req-sync-create",
        Some(owner()),
    ))
    .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    // The fixture's ports carry no outbound HTTP client, so the vendor cannot be reached.
    let run = pollster::block_on(dispatch.dispatch(
        &fx.deps,
        "connections.sync_now",
        json!({"connection_id": id, "since": "2026-07-01", "until": "2026-07-31"}),
        "req-sync",
        Some(owner()),
    ))
    .expect("the failed run is recorded, not raised");
    assert_eq!(run["status"], "failed");
    assert!(
        run["error_message"]
            .as_str()
            .unwrap()
            .contains("no outbound HTTP client")
    );
    assert!(!run.to_string().contains(TOKEN));
}
