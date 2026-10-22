//! People sign-in tests (issue #12): the OIDC callback against a fake issuer
//! that serves discovery, a JWKS and the token endpoint, and the session
//! behaviour that follows: expiry, logout, the company role rule, the origin
//! check on state-changing calls, and the unchanged agent-key path.
#![forbid(unsafe_code)]
#![allow(dead_code)]

use async_trait::async_trait;
use bytes::Bytes;
use cratefield_core::axum::body::to_bytes;
use cratefield_core::axum::http::{HeaderMap, Method, Request, StatusCode, header};
use cratefield_core::{Clock, Config, HttpClient, HttpError, IdGen, Module, Ports};
use cratefield_testing::TestHarness;
use ledgers_auth::oidc::{b64url, derive_key};
use ledgers_db::module::DbModule;
use ledgers_ledger::LedgerModule;
use ledgers_policy::module::PolicyModule;
use ledgers_server::dispatch::{self, ApiKeysModule, Deps, ServerDispatch};
use ledgers_server::people::{self, PeopleConfig, PeopleModule};
use ledgers_server::routes::{self, Dispatch};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey, signature::Signer};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use tower::ServiceExt;

const COMPANY: &str = dispatch::DEV_COMPANY_ID;
const ISSUER: &str = "https://issuer.test";
const CLIENT: &str = "ledgers-api-staging";
const API: &str = "https://api.test";
const APP: &str = "https://app.test";
const START: i64 = 1_792_674_000; // 2026-10-22T10:00:00Z
const SESSION_COOKIE: &str = "__Host-ledgers-session";
const LOGIN_COOKIE: &str = "__Host-ledgers-login";

/// A clock the test moves forward.
struct SharedClock(Mutex<OffsetDateTime>);

impl SharedClock {
    fn at(unix: i64) -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            OffsetDateTime::from_unix_timestamp(unix).expect("valid time"),
        )))
    }
    fn set(&self, unix: i64) {
        *self.0.lock().unwrap() = OffsetDateTime::from_unix_timestamp(unix).unwrap();
    }
}

#[async_trait]
impl Clock for SharedClock {
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().unwrap()
    }
}

#[derive(Default)]
struct Ids(AtomicU64);

impl IdGen for Ids {
    fn ulid(&self) -> String {
        let n = self.0.fetch_add(1, Ordering::Relaxed) + 1;
        format!("01PEOPLEID{n:016X}")
    }
}

/// The signing key the issuer publishes under `kid` "k1".
fn issuer_key() -> SigningKey {
    SigningKey::from_slice(&[7u8; 32]).expect("a valid P-256 scalar")
}

/// A key the issuer does not publish: signs "wrong key" tokens.
fn rogue_key() -> SigningKey {
    SigningKey::from_slice(&[9u8; 32]).expect("a valid P-256 scalar")
}

fn jwks_for(key: &SigningKey, kid: &str) -> String {
    let verifying = VerifyingKey::from(key);
    let point = verifying.to_encoded_point(false);
    json!({"keys": [{
        "kty": "EC", "crv": "P-256", "kid": kid, "alg": "ES256", "use": "sig",
        "x": b64url(point.x().expect("x")), "y": b64url(point.y().expect("y")),
    }]})
    .to_string()
}

/// Signs `claims` as a JWS with ES256 under `key`, header `kid` and `typ`.
fn sign(key: &SigningKey, kid: &str, typ: Option<&str>, claims: &Value) -> String {
    let mut header = json!({"alg": "ES256", "kid": kid});
    if let Some(typ) = typ {
        header["typ"] = json!(typ);
    }
    let signing_input = format!(
        "{}.{}",
        b64url(header.to_string().as_bytes()),
        b64url(claims.to_string().as_bytes())
    );
    let signature: Signature = key.sign(signing_input.as_bytes());
    format!("{signing_input}.{}", b64url(&signature.to_bytes()))
}

/// The fake issuer: discovery, the key set and the token endpoint.
struct FakeIssuer {
    jwks: Mutex<String>,
    id_token: Mutex<String>,
    token_status: Mutex<u16>,
}

#[async_trait]
impl HttpClient for FakeIssuer {
    async fn send(
        &self,
        request: http::Request<Bytes>,
    ) -> Result<http::Response<Bytes>, HttpError> {
        let (status, body) = match request.uri().path() {
            "/.well-known/openid-configuration" => (
                200,
                json!({
                    "issuer": ISSUER,
                    "authorization_endpoint": format!("{ISSUER}/authorize"),
                    "token_endpoint": format!("{ISSUER}/token"),
                    "jwks_uri": format!("{ISSUER}/jwks"),
                })
                .to_string(),
            ),
            "/jwks" => (200, self.jwks.lock().unwrap().clone()),
            "/token" => {
                let status = *self.token_status.lock().unwrap();
                let id_token = self.id_token.lock().unwrap().clone();
                (
                    status,
                    json!({"id_token": id_token, "access_token": "at"}).to_string(),
                )
            }
            _ => (404, "{}".to_owned()),
        };
        Ok(http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Bytes::from(body))
            .expect("response"))
    }
}

struct Fixture {
    router: cratefield_core::axum::Router,
    clock: Arc<SharedClock>,
    issuer: Arc<FakeIssuer>,
    api_key: String,
    deps: Deps,
}

fn config() -> PeopleConfig {
    PeopleConfig {
        issuer: ISSUER.to_owned(),
        client_id: CLIENT.to_owned(),
        api_url: API.to_owned(),
        app_url: APP.to_owned(),
        app_origins: vec![APP.to_owned()],
        seal_key: derive_key(b"test-harness-secret", "ledgers-people-login-cookie"),
    }
}

fn fixture() -> Fixture {
    let clock = SharedClock::at(START);
    let clock_dyn: Arc<dyn Clock> = clock.clone();
    let ids: Arc<dyn IdGen> = Arc::new(Ids::default());
    let issuer = Arc::new(FakeIssuer {
        jwks: Mutex::new(jwks_for(&issuer_key(), "k1")),
        id_token: Mutex::new(String::new()),
        token_status: Mutex::new(200),
    });

    let registry = dispatch::registry();
    let dispatch_impl: Arc<dyn Dispatch> =
        Arc::new(ServerDispatch::new(COMPANY).with_app_origins(vec![APP.to_owned()]));
    let domain = routes::domain_modules(&registry, &dispatch_impl).expect("domain modules");

    let builtin: Vec<Box<dyn Module>> = vec![
        Box::new(LedgerModule),
        Box::new(DbModule),
        Box::new(PolicyModule),
        Box::new(ledgers_storage::StorageModule),
        Box::new(ledgers_approvals::module::ApprovalsModule),
        Box::new(ledgers_approvals::module::ApprovalsInboxModule),
        Box::new(ApiKeysModule),
        Box::new(PeopleModule::new(Some(config()))),
    ];
    let patch_clock = clock_dyn.clone();
    let patch_ids = ids.clone();
    let patch_issuer = issuer.clone();
    let kit = TestHarness::with_builder(
        builtin,
        move |builder| {
            domain
                .into_iter()
                .fold(builder, cratefield_core::HarnessBuilder::module_arc)
        },
        move |ports: &mut Ports| {
            ports.clock = Some(patch_clock);
            ports.id_gen = Some(patch_ids);
            ports.http = Some(patch_issuer as Arc<dyn HttpClient>);
        },
    );
    let deps = Deps {
        db: kit.db.clone(),
        clock: clock_dyn,
        id_gen: ids,
        rate_limiter: None,
    };
    // The company row approval requests reference; the served bootstrap creates it.
    pollster::block_on(deps.db.execute(&cratefield_core::Statement::with_values(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT (id) DO NOTHING"
            .to_owned(),
        vec![
            sea_query::Value::from(COMPANY),
            sea_query::Value::from("Halden Works GmbH"),
            sea_query::Value::from("EUR"),
            sea_query::Value::from(START),
            sea_query::Value::from(START),
        ],
    )))
    .expect("company row");
    let api_key = pollster::block_on(dispatch::bootstrap_dev(&deps, COMPANY)).expect("bootstrap");
    Fixture {
        router: kit.router.clone(),
        clock,
        issuer,
        api_key,
        deps,
    }
}

/// One request through the router. Returns status, headers and body.
fn call(
    fx: &Fixture,
    method: Method,
    uri: &str,
    headers: &[(&str, String)],
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, value.as_str());
    }
    let request = match body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(cratefield_core::axum::body::Body::from(json.to_string())),
        None => builder.body(cratefield_core::axum::body::Body::empty()),
    }
    .expect("request");
    pollster::block_on(async {
        let response = fx
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("infallible router");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec();
        (status, headers, bytes)
    })
}

fn set_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim_start_matches(&format!("{name}="))
                .to_owned()
        })
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

/// Starts a sign-in. Returns the login cookie value and the state and nonce
/// the authorization URL carried.
fn start_login(fx: &Fixture) -> (String, String, String) {
    let (status, headers, _) = call(fx, Method::GET, "/v1/auth/login", &[], None);
    assert_eq!(status, StatusCode::FOUND, "login redirects");
    let location = headers
        .get(header::LOCATION)
        .expect("location")
        .to_str()
        .unwrap()
        .to_owned();
    let url = url::Url::parse(&location).expect("absolute location");
    assert!(location.starts_with(&format!("{ISSUER}/authorize?")));
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["redirect_uri"], format!("{API}/v1/auth/callback"));
    assert_eq!(query["client_id"], CLIENT);
    let cookie = set_cookie(&headers, LOGIN_COOKIE).expect("login cookie set");
    (cookie, query["state"].clone(), query["nonce"].clone())
}

fn claims(nonce: &str, now: i64) -> Value {
    json!({
        "iss": ISSUER, "sub": "person-42", "aud": CLIENT, "azp": CLIENT,
        "exp": now + 300, "iat": now, "nonce": nonce,
        "email": "owner@halden.test", "email_verified": true, "roles": ["owner"],
    })
}

/// Completes a sign-in with `id_token` and the given state. Returns the response.
fn callback(fx: &Fixture, login_cookie: &str, state: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    call(
        fx,
        Method::GET,
        &format!("/v1/auth/callback?code=auth-code&state={state}"),
        &[("cookie", format!("{LOGIN_COOKIE}={login_cookie}"))],
        None,
    )
}

/// A full successful sign-in for `roles`. Returns the session cookie value.
fn sign_in(fx: &Fixture, roles: &[&str]) -> String {
    let (login, state, nonce) = start_login(fx);
    let mut token = claims(&nonce, fx.clock.now().unix_timestamp());
    token["roles"] = json!(roles);
    *fx.issuer.id_token.lock().unwrap() = sign(&issuer_key(), "k1", Some("JWT"), &token);
    let (status, headers, _) = callback(fx, &login, &state);
    assert_eq!(status, StatusCode::SEE_OTHER, "callback signs in");
    assert_eq!(headers.get(header::LOCATION).unwrap(), APP);
    set_cookie(&headers, SESSION_COOKIE).expect("session cookie set")
}

fn cookie(session: &str) -> (&'static str, String) {
    ("cookie", format!("{SESSION_COOKIE}={session}"))
}

fn entry(external_id: &str) -> Value {
    json!({
        "entry": {
            "journal_id": "", "document_date": "2026-10-15", "posting_date": "2026-10-15",
            "tax_point_date": null, "base_currency": "EUR",
            "description": "Invoice INV-1001 to Acme GmbH", "source_documents": [],
            "external_id": external_id, "source_app": "fixture", "reverses": null,
            "lines": [
                {"account_number": "1010", "side": "debit", "base": {"minor": "100000", "currency": "EUR"},
                 "fx": null, "tax_code": null, "dimensions": {}, "memo": null},
                {"account_number": "4000", "side": "credit", "base": {"minor": "100000", "currency": "EUR"},
                 "fx": null, "tax_code": null, "dimensions": {}, "memo": null}
            ]
        }
    })
}

// ---------------------------------------------------------------------------
// The callback
// ---------------------------------------------------------------------------

#[test]
fn sign_in_opens_a_session_the_api_can_read() {
    let fx = fixture();
    let session = sign_in(&fx, &["owner"]);
    let (name, value) = cookie(&session);
    let (status, headers, body) =
        call(&fx, Method::GET, "/v1/auth/session", &[(name, value)], None);
    assert_eq!(status, StatusCode::OK);
    let body = json_body(&body);
    assert_eq!(body["personId"], "person-42");
    assert_eq!(body["email"], "owner@halden.test");
    assert_eq!(body["roles"], json!(["owner"]));
    assert!(body["csrfToken"].as_str().unwrap().len() > 20);
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    let _ = headers;
}

#[test]
fn callback_rejects_a_state_that_does_not_match() {
    let fx = fixture();
    let (login, _state, nonce) = start_login(&fx);
    let token = claims(&nonce, START);
    *fx.issuer.id_token.lock().unwrap() = sign(&issuer_key(), "k1", Some("JWT"), &token);
    let (status, headers, _) = callback(&fx, &login, "forged-state");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        set_cookie(&headers, SESSION_COOKIE).is_none(),
        "no session on a bad state"
    );
}

#[test]
fn callback_rejects_without_the_login_cookie() {
    let fx = fixture();
    let (status, _, _) = call(
        &fx,
        Method::GET,
        "/v1/auth/callback?code=c&state=s",
        &[],
        None,
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn callback_rejects_a_bad_nonce() {
    let fx = fixture();
    let (login, state, _nonce) = start_login(&fx);
    let token = claims("some-other-nonce", START);
    *fx.issuer.id_token.lock().unwrap() = sign(&issuer_key(), "k1", Some("JWT"), &token);
    let (status, headers, _) = callback(&fx, &login, &state);
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(set_cookie(&headers, SESSION_COOKIE).is_none());
}

#[test]
fn callback_rejects_a_token_for_another_audience() {
    let fx = fixture();
    let (login, state, nonce) = start_login(&fx);
    let mut token = claims(&nonce, START);
    token["aud"] = json!("some-other-venture");
    token["azp"] = json!("some-other-venture");
    *fx.issuer.id_token.lock().unwrap() = sign(&issuer_key(), "k1", Some("JWT"), &token);
    let (status, _, _) = callback(&fx, &login, &state);
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn callback_rejects_an_expired_token() {
    let fx = fixture();
    let (login, state, nonce) = start_login(&fx);
    let mut token = claims(&nonce, START);
    token["exp"] = json!(START - 3600);
    token["iat"] = json!(START - 7200);
    *fx.issuer.id_token.lock().unwrap() = sign(&issuer_key(), "k1", Some("JWT"), &token);
    let (status, _, _) = callback(&fx, &login, &state);
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn callback_rejects_a_token_signed_by_an_unpublished_key() {
    let fx = fixture();
    let (login, state, nonce) = start_login(&fx);
    let token = claims(&nonce, START);
    // Same kid as the published key, different key material.
    *fx.issuer.id_token.lock().unwrap() = sign(&rogue_key(), "k1", Some("JWT"), &token);
    let (status, _, _) = callback(&fx, &login, &state);
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn callback_rejects_an_unsigned_token() {
    let fx = fixture();
    let (login, state, nonce) = start_login(&fx);
    let header = b64url(br#"{"alg":"none","kid":"k1"}"#);
    let payload = b64url(claims(&nonce, START).to_string().as_bytes());
    *fx.issuer.id_token.lock().unwrap() = format!("{header}.{payload}.");
    let (status, _, _) = callback(&fx, &login, &state);
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn callback_rejects_an_access_token_offered_as_an_id_token() {
    let fx = fixture();
    let (login, state, nonce) = start_login(&fx);
    let token = claims(&nonce, START);
    *fx.issuer.id_token.lock().unwrap() = sign(&issuer_key(), "k1", Some("at+jwt"), &token);
    let (status, _, _) = callback(&fx, &login, &state);
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn a_login_transaction_cannot_be_used_after_it_expires() {
    let fx = fixture();
    let (login, state, nonce) = start_login(&fx);
    fx.clock.set(START + 11 * 60);
    let token = claims(&nonce, START + 11 * 60);
    *fx.issuer.id_token.lock().unwrap() = sign(&issuer_key(), "k1", Some("JWT"), &token);
    let (status, _, _) = callback(&fx, &login, &state);
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn a_tampered_login_cookie_is_refused() {
    let fx = fixture();
    let (login, state, nonce) = start_login(&fx);
    let token = claims(&nonce, START);
    *fx.issuer.id_token.lock().unwrap() = sign(&issuer_key(), "k1", Some("JWT"), &token);
    let mut tampered = login.clone();
    tampered.replace_range(0..1, if login.starts_with('A') { "B" } else { "A" });
    let (status, _, _) = callback(&fx, &tampered, &state);
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn a_token_endpoint_failure_is_not_a_session() {
    let fx = fixture();
    let (login, state, _nonce) = start_login(&fx);
    *fx.issuer.token_status.lock().unwrap() = 400;
    let (status, headers, _) = callback(&fx, &login, &state);
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(set_cookie(&headers, SESSION_COOKIE).is_none());
}

// ---------------------------------------------------------------------------
// Sessions, roles and logout
// ---------------------------------------------------------------------------

#[test]
fn session_route_is_401_without_a_cookie() {
    let fx = fixture();
    let (status, _, _) = call(&fx, Method::GET, "/v1/auth/session", &[], None);
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[test]
fn a_session_expires_when_idle_and_when_its_absolute_lifetime_ends() {
    let fx = fixture();
    let session = sign_in(&fx, &["owner"]);
    let (name, value) = cookie(&session);

    // Idle: 31 minutes without use.
    fx.clock.set(START + 31 * 60);
    let (status, _, _) = call(
        &fx,
        Method::GET,
        "/v1/auth/session",
        &[(name, value.clone())],
        None,
    );
    assert_eq!(status, StatusCode::UNAUTHORIZED, "idle timeout");

    // Absolute: an active session still ends at 12 hours.
    let session = sign_in(&fx, &["owner"]);
    let (name, value) = cookie(&session);
    for step in 1..=24 {
        fx.clock.set(START + 31 * 60 + step * 20 * 60);
        let (status, _, _) = call(
            &fx,
            Method::GET,
            "/v1/auth/session",
            &[(name, value.clone())],
            None,
        );
        assert_eq!(status, StatusCode::OK, "still active at step {step}");
    }
    fx.clock.set(START + 13 * 3600);
    let (status, _, _) = call(&fx, Method::GET, "/v1/auth/session", &[(name, value)], None);
    assert_eq!(status, StatusCode::UNAUTHORIZED, "absolute lifetime ends");
}

#[test]
fn logout_invalidates_the_session_on_the_next_request() {
    let fx = fixture();
    let session = sign_in(&fx, &["owner"]);
    let (name, value) = cookie(&session);
    let origin = vec![("origin", APP.to_owned()), (name, value.clone())];
    let (status, headers, _) = call(&fx, Method::POST, "/v1/auth/logout", &origin, None);
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        set_cookie(&headers, SESSION_COOKIE).as_deref(),
        Some(""),
        "cookie cleared"
    );
    let (status, _, _) = call(&fx, Method::GET, "/v1/auth/session", &[(name, value)], None);
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[test]
fn a_signed_in_non_owner_is_refused_with_403() {
    let fx = fixture();
    let session = sign_in(&fx, &["member"]);
    let (name, value) = cookie(&session);
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/auth/session",
        &[(name, value.clone())],
        None,
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        json_body(&body)["detail"]
            .as_str()
            .unwrap()
            .contains("owner or an admin")
    );

    let (status, _, body) = call(
        &fx,
        Method::POST,
        "/v1/entries",
        &[(name, value), ("origin", APP.to_owned())],
        Some(entry("NON-OWNER-1")),
    );
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&body)
    );
}

#[test]
fn an_admin_may_use_the_company() {
    let fx = fixture();
    let session = sign_in(&fx, &["admin"]);
    let (name, value) = cookie(&session);
    let (status, _, _) = call(&fx, Method::GET, "/v1/auth/session", &[(name, value)], None);
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Commands with a session: origin (CSRF) and principal
// ---------------------------------------------------------------------------

#[test]
fn a_state_changing_command_with_a_cookie_needs_an_allowed_origin() {
    let fx = fixture();
    let session = sign_in(&fx, &["owner"]);
    let (name, value) = cookie(&session);

    // No Origin: refused.
    let (status, _, _) = call(
        &fx,
        Method::POST,
        "/v1/entries",
        &[(name, value.clone())],
        Some(entry("CSRF-NO-ORIGIN")),
    );
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A foreign Origin: refused, even though the cookie is valid.
    let (status, _, _) = call(
        &fx,
        Method::POST,
        "/v1/entries",
        &[
            (name, value.clone()),
            ("origin", "https://evil.test".to_owned()),
        ],
        Some(entry("CSRF-EVIL-ORIGIN")),
    );
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The app's origin: the owner posts.
    let (status, _, body) = call(
        &fx,
        Method::POST,
        "/v1/entries",
        &[(name, value.clone()), ("origin", APP.to_owned())],
        Some(entry("OWNER-POST-1")),
    );
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
}

#[test]
fn a_read_with_a_cookie_needs_no_origin() {
    let fx = fixture();
    let session = sign_in(&fx, &["owner"]);
    let (name, value) = cookie(&session);
    let (status, _, _) = call(&fx, Method::GET, "/v1/accounts", &[(name, value)], None);
    assert_eq!(status, StatusCode::OK);
}

#[test]
fn logout_from_a_foreign_origin_is_refused() {
    let fx = fixture();
    let session = sign_in(&fx, &["owner"]);
    let (name, value) = cookie(&session);
    let (status, _, _) = call(
        &fx,
        Method::POST,
        "/v1/auth/logout",
        &[
            (name, value.clone()),
            ("origin", "https://evil.test".to_owned()),
        ],
        None,
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&fx, Method::GET, "/v1/auth/session", &[(name, value)], None);
    assert_eq!(
        status,
        StatusCode::OK,
        "a refused logout leaves the session alone"
    );
}

#[test]
fn agent_keys_still_work_without_any_cookie() {
    let fx = fixture();
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        &[("authorization", format!("Bearer {}", fx.api_key))],
        None,
    );
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let (status, _, _) = call(
        &fx,
        Method::POST,
        "/v1/entries",
        &[("authorization", format!("Bearer {}", fx.api_key))],
        Some(entry("KEY-POST-1")),
    );
    assert_eq!(status, StatusCode::OK);
}

#[test]
fn sign_in_is_503_when_no_issuer_is_configured() {
    let clock = SharedClock::at(START);
    let kit = TestHarness::with_builder(
        vec![
            Box::new(DbModule) as Box<dyn Module>,
            Box::new(PeopleModule::new(None)),
        ],
        |builder| builder,
        move |ports: &mut Ports| {
            ports.clock = Some(clock as Arc<dyn Clock>);
            ports.id_gen = Some(Arc::new(Ids::default()) as Arc<dyn IdGen>);
        },
    );
    let response = pollster::block_on(
        kit.router.clone().oneshot(
            Request::builder()
                .uri("/v1/auth/login")
                .body(cratefield_core::axum::body::Body::empty())
                .unwrap(),
        ),
    )
    .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn from_config_reads_the_issuer_and_refuses_a_partial_setup() {
    struct Env(Vec<(&'static str, &'static str)>);
    impl Config for Env {
        fn get(&self, key: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }
    let full = Env(vec![
        ("LEDGERS_AUTH_ISSUER", "https://auth-staging.ledgers.sh/"),
        ("LEDGERS_AUTH_CLIENT_ID", "ledgers-api-staging"),
        ("LEDGERS_APP_URL", "https://app-staging.ledgers.sh"),
        ("HARNESS_SECRET", "s3cret"),
    ]);
    let cfg = PeopleConfig::from_config(&full, API, vec![APP.into()], false)
        .expect("valid")
        .expect("sign-in on");
    assert_eq!(cfg.issuer, "https://auth-staging.ledgers.sh");

    let unset = Env(vec![]);
    assert!(
        PeopleConfig::from_config(&unset, API, vec![], false)
            .unwrap()
            .is_none()
    );

    let no_client = Env(vec![
        ("LEDGERS_AUTH_ISSUER", "https://auth-staging.ledgers.sh"),
        ("LEDGERS_APP_URL", "https://app-staging.ledgers.sh"),
        ("HARNESS_SECRET", "s3cret"),
    ]);
    assert!(PeopleConfig::from_config(&no_client, API, vec![], false).is_err());

    let plain_http = Env(vec![
        ("LEDGERS_AUTH_ISSUER", "http://auth.example"),
        ("LEDGERS_AUTH_CLIENT_ID", "c"),
        ("LEDGERS_APP_URL", "https://app.example"),
        ("HARNESS_SECRET", "s3cret"),
    ]);
    assert!(PeopleConfig::from_config(&plain_http, API, vec![], true).is_err());
    let local = Env(vec![
        ("LEDGERS_AUTH_ISSUER", "http://localhost:8787"),
        ("LEDGERS_AUTH_CLIENT_ID", "c"),
        ("LEDGERS_APP_URL", "http://localhost:5173"),
        ("HARNESS_SECRET", "s3cret"),
    ]);
    assert!(
        PeopleConfig::from_config(&local, API, vec![], true)
            .unwrap()
            .is_some()
    );
}

#[test]
fn people_module_declares_the_session_migration() {
    let module = PeopleModule::new(None);
    let migrations = module.migrations();
    assert_eq!(people::PEOPLE_MIGRATIONS.len(), 2);
    assert_eq!(people::PEOPLE_MIGRATIONS[0].name, "people_session");
    assert_eq!(
        people::PEOPLE_MIGRATIONS[1].name,
        "people_session_auth_time"
    );
    assert_eq!(migrations.sqlite.len(), 2);
}
