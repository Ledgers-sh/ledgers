//! Agent key management (`keys.list`, `keys.create`, `keys.revoke`): people
//! only, the token shown once, revocation that bites at once on HTTP and MCP,
//! `last_used_at`, and no secret in the logs, the audit trail or an error.

mod common;

use common::http::{Auth, call, mcp};
use common::{OWNER_PERSON, OWNER_SESSION, fixture_with_options, owner_session};
use cratefield_core::Statement;
use cratefield_core::axum::http::Method;
use ledgers_server::McpModule;
use ledgers_server::dispatch::{self, ServerDispatch};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// The app origin the owner session may call from.
const APP: &str = common::http::APP_ORIGIN;

/// The fixture: the owner session stored, the app origin allowed, and the
/// MCP endpoint mounted beside the domain modules.
fn fixture() -> common::Fixture {
    let mcp_module = McpModule::new(
        Arc::new(dispatch::registry()),
        Arc::new(ServerDispatch::new(common::COMPANY).with_app_origins(vec![APP.to_owned()])),
        "https://api.test".to_owned(),
    );
    let fx = fixture_with_options(vec![Arc::new(mcp_module)], None, vec![APP.to_owned()]);
    owner_session(&fx);
    fx
}

/// The bodies that create a key: a post-tier key with EUR limits.
fn create_body(name: &str, tier: &str) -> Value {
    json!({
        "name": name,
        "tier": tier,
        "limits": { "EUR": { "per_entry": "1000.00", "per_day": "5000.00" } }
    })
}

/// Creates a key as the owner and returns the response body.
fn create_as_owner(fx: &common::Fixture, name: &str, tier: &str) -> Value {
    let reply = call(
        fx,
        Method::POST,
        "/v1/keys",
        Auth::Session(OWNER_SESSION),
        true,
        Some(create_body(name, tier)),
    );
    assert_eq!(reply.status.as_u16(), 200, "create failed: {}", reply.text);
    reply.json()
}

/// The prefix of a token: everything before the 64-hex-digit secret.
fn prefix_of(token: &str) -> &str {
    &token[..token.len() - 65]
}

/// The secret part of a token: the last 64 hex digits.
fn secret_of(token: &str) -> &str {
    &token[token.len() - 64..]
}

/// Lists the keys as the owner.
fn list_as_owner(fx: &common::Fixture) -> Value {
    let reply = call(
        fx,
        Method::GET,
        "/v1/keys",
        Auth::Session(OWNER_SESSION),
        false,
        None,
    );
    assert_eq!(reply.status.as_u16(), 200, "list failed: {}", reply.text);
    reply.json()
}

/// One key from the owner's list, by its prefix.
fn listed(fx: &common::Fixture, prefix: &str) -> Value {
    list_as_owner(fx)["keys"]
        .as_array()
        .expect("keys array")
        .iter()
        .find(|key| key["prefix"] == prefix)
        .cloned()
        .unwrap_or_else(|| panic!("no listed key with prefix {prefix}"))
}

#[test]
fn owner_creates_a_key_and_the_token_is_returned_once() {
    let fx = fixture();
    let created = create_as_owner(&fx, "Bank bot", "post-within-limits");
    let token = created["token"].as_str().expect("token").to_owned();
    assert!(token.starts_with("lk_live_"), "token shape: {token}");
    assert_eq!(created["key"]["prefix"], prefix_of(&token));
    assert_eq!(created["key"]["name"], "Bank bot");
    assert_eq!(created["key"]["tier"], "post-within-limits");
    assert_eq!(created["key"]["created_by"], OWNER_PERSON);
    assert_eq!(created["key"]["limits"]["EUR"]["per_entry"], "1000.00");
    assert_eq!(created["key"]["limits"]["EUR"]["per_day"], "5000.00");
    assert!(created["key"]["last_used_at"].is_null());
    assert!(created["key"]["revoked_at"].is_null());

    // The list shows the prefix and never the token or its secret.
    let list = list_as_owner(&fx).to_string();
    assert!(list.contains(prefix_of(&token)));
    assert!(!list.contains(&token), "list carries the full token");
    assert!(!list.contains(secret_of(&token)), "list carries the secret");
    let key = listed(&fx, prefix_of(&token));
    assert!(key.get("token").is_none());

    // The token works, as a bearer, for a read.
    let read = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        Auth::Bearer(&token),
        false,
        None,
    );
    assert_eq!(read.status.as_u16(), 200, "new key refused: {}", read.text);
}

#[test]
fn limits_are_stored_and_listed_in_the_currency_exponent() {
    let fx = fixture();
    let reply = call(
        &fx,
        Method::POST,
        "/v1/keys",
        Auth::Session(OWNER_SESSION),
        true,
        Some(json!({
            "name": "Exponents",
            "tier": "draft",
            "limits": {
                "JPY": { "per_entry": "500", "per_day": "5000" },
                "BHD": { "per_entry": "1.234", "per_day": "10" }
            }
        })),
    );
    assert_eq!(reply.status.as_u16(), 200, "{}", reply.text);
    let token = reply.json()["token"].as_str().expect("token").to_owned();
    let key = listed(&fx, prefix_of(&token));
    assert_eq!(key["limits"]["JPY"]["per_entry"], "500");
    assert_eq!(key["limits"]["BHD"]["per_entry"], "1.234");
    assert_eq!(key["limits"]["BHD"]["per_day"], "10.000");
}

#[test]
fn agent_keys_get_403_on_every_key_command_and_create_nothing() {
    let fx = fixture();
    let before = list_as_owner(&fx)["keys"].as_array().expect("keys").len();
    let agent = fx.keys.full.clone();

    let list = call(
        &fx,
        Method::GET,
        "/v1/keys",
        Auth::Bearer(&agent),
        false,
        None,
    );
    assert_eq!(
        list.status.as_u16(),
        403,
        "agent listed keys: {}",
        list.text
    );
    let create = call(
        &fx,
        Method::POST,
        "/v1/keys",
        Auth::Bearer(&agent),
        false,
        Some(create_body("Sneaky", "post-within-limits")),
    );
    assert_eq!(
        create.status.as_u16(),
        403,
        "agent created a key: {}",
        create.text
    );
    let revoke = call(
        &fx,
        Method::POST,
        "/v1/keys/any-id/revoke",
        Auth::Bearer(&agent),
        false,
        Some(json!({})),
    );
    assert_eq!(
        revoke.status.as_u16(),
        403,
        "agent revoked a key: {}",
        revoke.text
    );

    assert_eq!(
        list_as_owner(&fx)["keys"].as_array().expect("keys").len(),
        before,
        "a refused agent call changed the key set"
    );
}

#[test]
fn missing_or_unknown_credentials_get_401() {
    let fx = fixture();
    let anonymous = call(&fx, Method::GET, "/v1/keys", Auth::Anonymous, false, None);
    assert_eq!(anonymous.status.as_u16(), 401);
    let forged = call(
        &fx,
        Method::GET,
        "/v1/keys",
        Auth::Bearer("lk_live_0123456789abcdef_00"),
        false,
        None,
    );
    assert_eq!(forged.status.as_u16(), 401);
}

#[test]
fn a_session_write_without_the_app_origin_is_refused() {
    let fx = fixture();
    let reply = call(
        &fx,
        Method::POST,
        "/v1/keys",
        Auth::Session(OWNER_SESSION),
        false,
        Some(create_body("No origin", "read")),
    );
    assert_eq!(reply.status.as_u16(), 403, "{}", reply.text);
    assert_eq!(
        list_as_owner(&fx)["keys"].as_array().expect("keys").len(),
        4
    );
}

#[test]
fn bad_names_tiers_and_limits_are_refused_with_400() {
    let fx = fixture();
    let cases = [
        json!({"name": "   ", "tier": "read", "limits": {}}),
        json!({"name": "x".repeat(81), "tier": "read", "limits": {}}),
        json!({"name": "ok", "tier": "admin", "limits": {}}),
        json!({"name": "ok", "tier": "read", "limits": {"EUR": {"per_entry": "1.005", "per_day": "1"}}}),
        json!({"name": "ok", "tier": "read", "limits": {"EUR": {"per_entry": "-1", "per_day": "1"}}}),
        json!({"name": "ok", "tier": "read", "limits": {"EUR": {"per_entry": "1e3", "per_day": "1"}}}),
        json!({"name": "ok", "tier": "read", "limits": {"EUR": {"per_entry": 1000, "per_day": "1"}}}),
        json!({"name": "ok", "tier": "read", "limits": {"ZZZ": {"per_entry": "1", "per_day": "1"}}}),
    ];
    for body in cases {
        let reply = call(
            &fx,
            Method::POST,
            "/v1/keys",
            Auth::Session(OWNER_SESSION),
            true,
            Some(body.clone()),
        );
        assert!(
            reply.status.is_client_error() && reply.status.as_u16() != 401,
            "{body} should be refused, got {}: {}",
            reply.status,
            reply.text
        );
    }
    assert_eq!(
        list_as_owner(&fx)["keys"].as_array().expect("keys").len(),
        4
    );
}

#[test]
fn revocation_takes_effect_on_the_next_http_request() {
    let fx = fixture();
    let created = create_as_owner(&fx, "Revoke me", "post-within-limits");
    let token = created["token"].as_str().expect("token").to_owned();
    let id = created["key"]["id"].as_str().expect("id").to_owned();

    let before = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        Auth::Bearer(&token),
        false,
        None,
    );
    assert_eq!(before.status.as_u16(), 200);

    let revoke = call(
        &fx,
        Method::POST,
        &format!("/v1/keys/{id}/revoke"),
        Auth::Session(OWNER_SESSION),
        true,
        Some(json!({})),
    );
    assert_eq!(revoke.status.as_u16(), 200, "{}", revoke.text);
    assert!(revoke.json()["key"]["revoked_at"].is_string());

    // The same clock reading, the very next request: refused.
    let after = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        Auth::Bearer(&token),
        false,
        None,
    );
    assert_eq!(
        after.status.as_u16(),
        401,
        "revoked key still works: {}",
        after.text
    );
    let key = listed(&fx, prefix_of(&token));
    assert!(
        key["revoked_at"].is_string(),
        "the revoked key stays listed, marked"
    );
}

#[test]
fn revocation_takes_effect_on_the_next_mcp_tool_call() {
    let fx = fixture();
    let created = create_as_owner(&fx, "Over MCP", "post-within-limits");
    let token = created["token"].as_str().expect("token").to_owned();
    let id = created["key"]["id"].as_str().expect("id").to_owned();
    let call_read = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "accounts_list", "arguments": {}}});

    let before = mcp(&fx, &token, call_read.clone());
    assert_eq!(before.status.as_u16(), 200);
    assert!(!before.text.contains("\"isError\":true"), "{}", before.text);

    let revoke = call(
        &fx,
        Method::POST,
        &format!("/v1/keys/{id}/revoke"),
        Auth::Session(OWNER_SESSION),
        true,
        Some(json!({})),
    );
    assert_eq!(revoke.status.as_u16(), 200, "{}", revoke.text);

    let after = mcp(&fx, &token, call_read);
    assert!(
        after.text.contains("\"isError\":true") || after.status.as_u16() == 401,
        "revoked key still runs MCP tools: {}",
        after.text
    );
}

#[test]
fn agents_are_not_offered_the_key_commands_over_mcp() {
    let fx = fixture();
    let created = create_as_owner(&fx, "Listing", "read");
    let token = created["token"].as_str().expect("token").to_owned();
    let reply = mcp(
        &fx,
        &token,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    );
    assert_eq!(reply.status.as_u16(), 200, "{}", reply.text);
    assert!(
        !reply.text.contains("keys_"),
        "a key command is offered to agents: {}",
        reply.text
    );
}

#[test]
fn revoking_twice_is_idempotent_and_an_unknown_id_is_not_found() {
    let fx = fixture();
    let created = create_as_owner(&fx, "Twice", "read");
    let id = created["key"]["id"].as_str().expect("id").to_owned();
    let path = format!("/v1/keys/{id}/revoke");
    let first = call(
        &fx,
        Method::POST,
        &path,
        Auth::Session(OWNER_SESSION),
        true,
        Some(json!({})),
    );
    let second = call(
        &fx,
        Method::POST,
        &path,
        Auth::Session(OWNER_SESSION),
        true,
        Some(json!({})),
    );
    assert_eq!(first.status.as_u16(), 200);
    assert_eq!(second.status.as_u16(), 200);
    assert_eq!(
        first.json()["key"]["revoked_at"],
        second.json()["key"]["revoked_at"]
    );

    let unknown = call(
        &fx,
        Method::POST,
        "/v1/keys/no-such-key/revoke",
        Auth::Session(OWNER_SESSION),
        true,
        Some(json!({})),
    );
    assert_eq!(unknown.status.as_u16(), 404, "{}", unknown.text);
}

#[test]
fn last_used_at_is_set_by_the_first_use_and_listed() {
    let fx = fixture();
    let created = create_as_owner(&fx, "Used", "read");
    let token = created["token"].as_str().expect("token").to_owned();
    let prefix = prefix_of(&token).to_owned();
    assert!(listed(&fx, &prefix)["last_used_at"].is_null());
    let read = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        Auth::Bearer(&token),
        false,
        None,
    );
    assert_eq!(read.status.as_u16(), 200);
    assert!(listed(&fx, &prefix)["last_used_at"].is_string());
}

#[test]
fn the_secret_never_reaches_the_logs_the_audit_trail_or_an_error() {
    // Capture every log line the server writes while a key is created, used,
    // refused and revoked.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("capture lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Capture {
            self.clone()
        }
    }

    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(capture.clone())
        .finish();
    let (token, refused_text, audit_text) = tracing::subscriber::with_default(subscriber, || {
        let fx = fixture();
        let created = create_as_owner(&fx, "Logs", "post-within-limits");
        let token = created["token"].as_str().expect("token").to_owned();
        let id = created["key"]["id"].as_str().expect("id").to_owned();
        let used = call(
            &fx,
            Method::GET,
            "/v1/accounts",
            Auth::Bearer(&token),
            false,
            None,
        );
        assert_eq!(used.status.as_u16(), 200);
        let revoke = call(
            &fx,
            Method::POST,
            &format!("/v1/keys/{id}/revoke"),
            Auth::Session(OWNER_SESSION),
            true,
            Some(json!({})),
        );
        assert_eq!(revoke.status.as_u16(), 200);
        // A refused use of the revoked key: the error must not echo it.
        let refused = call(
            &fx,
            Method::GET,
            "/v1/accounts",
            Auth::Bearer(&token),
            false,
            None,
        );
        assert_eq!(refused.status.as_u16(), 401);
        // The audit trail: every payload the key operations wrote.
        let rows = pollster::block_on(cratefield_core::Database::query(
            fx.deps.db.as_ref(),
            &Statement::with_values("SELECT payload FROM audit_event".to_owned(), vec![]),
        ))
        .expect("audit rows");
        let audit_text = rows
            .rows
            .iter()
            .filter_map(|row| row.get::<String>("payload"))
            .collect::<Vec<_>>()
            .join("\n");
        (token, refused.text, audit_text)
    });
    let logs = String::from_utf8_lossy(&capture.0.lock().expect("capture lock")).into_owned();
    let secret = secret_of(&token);
    assert!(!logs.contains(&token), "the full token reached the logs");
    assert!(!logs.contains(secret), "the secret reached the logs");
    assert!(
        !refused_text.contains(secret),
        "the error echoed the secret"
    );
    assert!(
        !audit_text.contains(secret),
        "the audit trail holds the secret"
    );
    assert!(
        audit_text.contains(prefix_of(&token)),
        "the audit trail should name the public prefix"
    );
}
