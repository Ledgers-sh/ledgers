//! People sign-in (issue #12): the Backend-for-Frontend session the API holds.
//!
//! The browser never sees a token. `GET /v1/auth/login` starts an OIDC
//! authorization-code flow with PKCE against the per-app Cratefield auth
//! instance (`LEDGERS_AUTH_ISSUER`); `GET /v1/auth/callback` exchanges the code,
//! verifies the ID token and opens a server-side session. The session is an
//! opaque random id in an httpOnly cookie on the API host; its SHA-256 is the
//! only thing the database keeps. Commands run as `Principal::Person` only for
//! accounts whose roles entitle them to the company ([`may_use_company`]).
//!
//! Browser calls with the cookie are checked by Origin: a state-changing
//! request must name an allowed app origin, which cross-site pages cannot
//! forge. Agent keys (`Authorization: Bearer`) are unaffected.

use crate::dispatch::Deps;
use crate::members;
use crate::routes::{Presented, SessionPresented, requested_company};
use async_trait::async_trait;
use bytes::Bytes;
use cratefield_core::axum::{
    self, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use cratefield_core::{
    Clock, Config, ConfigError, Database, DbError, HttpClient, Migrations, Module, ModuleContext,
    Port, Problem, ProblemDef, SqlMigration, Statement,
};
use ledgers_auth::oidc::{
    self, Discovery, LoginState, OidcError, PublishedKey, b64url, pkce_challenge,
};
use ledgers_auth::session::{
    IDLE_TTL_SECONDS, LOGIN_COOKIE, LOGIN_TTL_SECONDS, SESSION_COOKIE, SESSION_TTL_SECONDS,
    SessionBuilder, clear_login_cookie, login_cookie, session_key_hash,
};
use ledgers_commands::CommandError;
use ledgers_commands::auth::{Authenticator, Credential, Principal};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The module name, which is also its mount: the sign-in routes live under
/// `/v1/auth` (`/v1/auth/login`, `/v1/auth/callback`, `/v1/auth/logout`,
/// `/v1/auth/session`). The harness mounts a module's routes only under
/// `/v1/<name>`, so the sign-in cannot sit at the root.
pub const PEOPLE_MODULE: &str = "auth";

/// The path the issuer redirects back to, relative to the API origin.
pub const CALLBACK_PATH: &str = "/v1/auth/callback";

/// The ports the sign-in needs: the session table, time, ids and the HTTP
/// client for discovery, the key set and the token exchange.
pub const PEOPLE_PORTS: &[Port] = &[Port::Db, Port::Clock, Port::IdGen, Port::HttpClient];

/// Migrations of the people session table, shipped with the server.
pub const PEOPLE_MIGRATIONS: &[SqlMigration] = &[
    SqlMigration::new(
        "0001",
        "people_session",
        include_str!("../migrations/sqlite/0001_people_session.sql"),
    ),
    SqlMigration::new(
        "0002",
        "people_session_auth_time",
        include_str!("../migrations/sqlite/0002_people_session_auth_time.sql"),
    ),
    SqlMigration::new(
        "0003",
        "people_profile",
        include_str!("../migrations/sqlite/0003_people_profile.sql"),
    ),
];

/// Sign-in is off: `LEDGERS_AUTH_ISSUER` is not set, or the instance is not
/// fully configured. 503, so a caller can tell "not set up" from "failed".
pub const SIGN_IN_NOT_CONFIGURED: ProblemDef = ProblemDef {
    slug: "sign-in-not-configured",
    status: StatusCode::SERVICE_UNAVAILABLE,
    title: "Sign-in is not configured",
    description: "This server has no people sign-in issuer configured.",
};

/// The sign-in callback could not be completed (state, nonce, token, key).
pub const SIGN_IN_FAILED: ProblemDef = ProblemDef {
    slug: "sign-in-failed",
    status: StatusCode::BAD_REQUEST,
    title: "Sign-in failed",
    description: "The sign-in could not be completed. The details are in the server log.",
};

/// No valid session cookie: never signed in, expired, or signed out.
pub const SESSION_REQUIRED: ProblemDef = ProblemDef {
    slug: "session-required",
    status: StatusCode::UNAUTHORIZED,
    title: "Sign-in required",
    description: "There is no valid person session on this request.",
};

/// The signed-in person is not a member of the company. The app shows a screen
/// that asks them to ask the owner for an invitation.
pub const NOT_A_MEMBER: ProblemDef = ProblemDef {
    slug: "not-a-member",
    status: StatusCode::FORBIDDEN,
    title: "You're not a member of this company yet",
    description: "The signed-in account holds no membership in this company. Ask the owner for an invitation.",
};

/// A cookie-authenticated request came from an origin that is not an app
/// origin, or a state-changing request named none.
pub const SESSION_ORIGIN_FORBIDDEN: ProblemDef = ProblemDef {
    slug: "session-origin-forbidden",
    status: StatusCode::FORBIDDEN,
    title: "Request origin not allowed",
    description: "A cookie-authenticated request must come from an allowed app origin.",
};

/// The sign-in configuration, read at harness build time.
#[derive(Clone, Debug)]
pub struct PeopleConfig {
    /// The issuer, exactly as discovery names it (no trailing slash).
    pub issuer: String,
    /// The client id registered with the issuer (a public PKCE client).
    pub client_id: String,
    /// The API origin, `LEDGERS_PUBLIC_URL`; the callback is `<api>/v1/auth/callback`.
    pub api_url: String,
    /// Where the browser is sent after sign-in (`LEDGERS_APP_URL`).
    pub app_url: String,
    /// The origins a cookie-authenticated request may come from.
    pub app_origins: Vec<String>,
    /// The key that seals the login-state cookie, derived from `HARNESS_SECRET`.
    pub seal_key: Vec<u8>,
}

impl PeopleConfig {
    /// Reads the sign-in configuration. `Ok(None)` when no issuer is set:
    /// sign-in is then off and the routes answer 503.
    ///
    /// # Errors
    /// A message naming the missing or malformed variable.
    pub fn from_config(
        cfg: &dyn Config,
        api_base: &str,
        app_origins: Vec<String>,
        dev: bool,
    ) -> Result<Option<Self>, String> {
        let Some(issuer) = cfg
            .get("LEDGERS_AUTH_ISSUER")
            .map(|v| v.trim().trim_end_matches('/').to_owned())
            .filter(|v| !v.is_empty())
        else {
            return Ok(None);
        };
        let issuer_origin = oidc::origin_of(&issuer)
            .ok_or("LEDGERS_AUTH_ISSUER must be an absolute http(s) URL")?;
        let local = issuer_origin.starts_with("http://localhost")
            || issuer_origin.starts_with("http://127.0.0.1");
        if issuer_origin.starts_with("http://") && !(dev && local) {
            return Err("LEDGERS_AUTH_ISSUER must be https (http is allowed for localhost in LEDGERS_DEV=1)".into());
        }
        let client_id = cfg
            .get("LEDGERS_AUTH_CLIENT_ID")
            .filter(|v| !v.trim().is_empty())
            .ok_or("LEDGERS_AUTH_CLIENT_ID is required when LEDGERS_AUTH_ISSUER is set")?;
        let app_url = cfg
            .get("LEDGERS_APP_URL")
            .map(|v| v.trim().trim_end_matches('/').to_owned())
            .filter(|v| oidc::origin_of(v).is_some())
            .ok_or("LEDGERS_APP_URL must be set to the app's absolute URL when sign-in is on")?;
        let secret = cfg
            .get("HARNESS_SECRET")
            .filter(|v| !v.is_empty())
            .ok_or("HARNESS_SECRET is required when sign-in is on (it seals the login cookie)")?;
        Ok(Some(PeopleConfig {
            issuer,
            client_id,
            api_url: api_base.trim_end_matches('/').to_owned(),
            app_url,
            app_origins,
            seal_key: oidc::derive_key(secret.as_bytes(), "ledgers-people-login-cookie"),
        }))
    }

    /// The callback URL the issuer must have registered for this client.
    #[must_use]
    pub fn redirect_uri(&self) -> String {
        format!("{}{CALLBACK_PATH}", self.api_url)
    }
}

/// The people sign-in as a harness module: routes under `/auth`, and the
/// session table as its migration.
pub struct PeopleModule {
    config: Option<PeopleConfig>,
    company_id: String,
}

impl PeopleModule {
    /// The module for `config` (`None`: sign-in off, routes answer 503) and the
    /// company whose membership decides who may use it.
    #[must_use]
    pub fn new(config: Option<PeopleConfig>, company_id: impl Into<String>) -> Self {
        Self {
            config,
            company_id: company_id.into(),
        }
    }
}

/// The table the member list reads each person's address and last activity
/// from (see `members::touch_profile`).
pub const PROFILE_TABLE: &str = "people_profile";

impl Module for PeopleModule {
    fn name(&self) -> &'static str {
        PEOPLE_MODULE
    }

    fn tables(&self) -> &'static [&'static str] {
        &[PROFILE_TABLE]
    }

    /// The profile row is the person's own: erasing the account removes it.
    fn personal_data(&self) -> &'static [cratefield_core::PersonalDataSet] {
        const SETS: [cratefield_core::PersonalDataSet; 1] = [cratefield_core::PersonalDataSet {
            table: PROFILE_TABLE,
            subject: "person_id",
            kind: cratefield_core::DataKind::Contact,
            disposition: cratefield_core::Disposition::Erase,
            description: "What the company shows about you: the address from your last sign-in and when you last used the app.",
            redacted: &[],
            subject_via: None,
        }];
        &SETS
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        PEOPLE_PORTS
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        let deps = Deps::from_ports(&ctx.ports)
            .expect("the people ports (db, clock, id_gen) are declared in requires()");
        let state = Arc::new(PeopleState {
            deps,
            http: ctx.ports.http.clone(),
            config: self.config.clone(),
            company_id: self.company_id.clone(),
        });
        axum::Router::new()
            .route("/login", get(login))
            .route("/callback", get(callback))
            .route("/logout", post(logout))
            .route("/session", get(session))
            .route("/roles", get(roles))
            .route("/people", get(people))
            .route("/invitations", get(invitations))
            .route("/invitations/{id}", delete(revoke_invitation_route))
            .with_state(state)
    }

    fn migrations(&self) -> Migrations {
        Migrations::sqlite(PEOPLE_MIGRATIONS)
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }
}

struct PeopleState {
    deps: Deps,
    http: Option<Arc<dyn HttpClient>>,
    config: Option<PeopleConfig>,
    company_id: String,
}

// ---------------------------------------------------------------------------
// Cookies, origins and presentation
// ---------------------------------------------------------------------------

/// The value of cookie `name` in the request's Cookie headers.
#[must_use]
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_owned())
        .filter(|value| !value.is_empty())
}

/// The request's `Origin`, when it sent one.
#[must_use]
pub fn origin_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// How a request presents its caller. A bearer header wins over a cookie;
/// with neither, the request is anonymous. Only `safe` (GET/HEAD/OPTIONS)
/// methods may run with a cookie and no Origin.
#[must_use]
pub fn presented_from(headers: &HeaderMap, safe: bool) -> Option<Presented> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    {
        return Some(Presented::Bearer(token.to_owned()));
    }
    cookie_value(headers, SESSION_COOKIE).map(|id| {
        Presented::Session(SessionPresented {
            id,
            origin: origin_header(headers),
            safe,
        })
    })
}

/// The app origin a request came from, if it is one of `allowed`.
#[must_use]
pub fn allowed_origin<'a>(allowed: &'a [String], origin: &str) -> Option<&'a str> {
    allowed
        .iter()
        .find(|candidate| candidate.as_str() == origin)
        .map(String::as_str)
}

/// Response headers for an origin allowed to make credentialed calls. The
/// origin is echoed exactly, never `*`, and `Allow-Credentials` is set.
#[must_use]
pub fn credentialed_cors_headers(origin: &str) -> Vec<(&'static str, String)> {
    vec![
        ("access-control-allow-origin", origin.to_owned()),
        ("access-control-allow-credentials", "true".to_owned()),
        ("vary", "Origin".to_owned()),
    ]
}

/// The origin check for a cookie-authenticated request: an allowed origin
/// when one was sent, and a state-changing request must send one.
///
/// # Errors
/// [`SESSION_ORIGIN_FORBIDDEN`] when the origin is not allowed, or is missing
/// on a state-changing request.
pub fn check_session_origin(
    allowed: &[String],
    origin: Option<&str>,
    safe: bool,
) -> Result<(), Problem> {
    match origin {
        Some(origin) if allowed_origin(allowed, origin).is_some() => Ok(()),
        Some(_) => Err(Problem::new(&SESSION_ORIGIN_FORBIDDEN)),
        None if safe => Ok(()),
        None => Err(Problem::new(&SESSION_ORIGIN_FORBIDDEN).with_detail(
            "a state-changing request with a session cookie must name its app origin",
        )),
    }
}

// ---------------------------------------------------------------------------
// The session table
// ---------------------------------------------------------------------------

/// One stored session, as the request path reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub person_id: String,
    pub email: Option<String>,
    pub roles: Vec<String>,
    pub csrf_token: String,
    pub idle_expires_at: i64,
    pub absolute_expires_at: i64,
    /// When the person last authenticated at the issuer (epoch seconds).
    pub auth_time: Option<i64>,
}

impl SessionRow {
    /// Whether the session has run out at `now` (epoch seconds).
    #[must_use]
    pub fn expired_at(&self, now: i64) -> bool {
        now >= self.idle_expires_at || now >= self.absolute_expires_at
    }
}

/// Stores a new session. Expired sessions are swept on the way in.
///
/// # Errors
/// The database error, as a [`cratefield_core::DbError`].
pub async fn store_session(
    db: &dyn Database,
    session_id: &str,
    person: &PersonClaims,
    now: i64,
) -> Result<SessionRow, cratefield_core::DbError> {
    db.execute(&Statement::with_values(
        "DELETE FROM people_session WHERE absolute_expires_at <= ?",
        vec![sea_query::Value::from(now)],
    ))
    .await?;
    let row = SessionRow {
        person_id: person.person_id.clone(),
        email: person.email.clone(),
        roles: person.roles.clone(),
        csrf_token: random_token(32),
        idle_expires_at: now + IDLE_TTL_SECONDS,
        absolute_expires_at: now + i64::try_from(SESSION_TTL_SECONDS).unwrap_or(i64::MAX),
        auth_time: person.auth_time,
    };
    db.execute(&Statement::with_values(
        "INSERT INTO people_session (id_hash, person_id, email, roles, csrf_token, created_at, idle_expires_at, absolute_expires_at, auth_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        vec![
            sea_query::Value::from(session_key_hash(session_id)),
            sea_query::Value::from(row.person_id.clone()),
            sea_query::Value::String(row.email.clone().map(Box::new)),
            sea_query::Value::from(serde_json::to_string(&row.roles).unwrap_or_else(|_| "[]".into())),
            sea_query::Value::from(row.csrf_token.clone()),
            sea_query::Value::from(now),
            sea_query::Value::from(row.idle_expires_at),
            sea_query::Value::from(row.absolute_expires_at),
            sea_query::Value::BigInt(row.auth_time),
        ],
    ))
    .await?;
    Ok(row)
}

/// Reads a session by its cookie id. `None` when there is no such session.
///
/// # Errors
/// The database error.
pub async fn load_session(
    db: &dyn Database,
    session_id: &str,
) -> Result<Option<SessionRow>, cratefield_core::DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT person_id, email, roles, csrf_token, idle_expires_at, absolute_expires_at, auth_time FROM people_session WHERE id_hash = ?",
            vec![sea_query::Value::from(session_key_hash(session_id))],
        ))
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let roles: Vec<String> = row
        .get::<String>("roles")
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    Ok(Some(SessionRow {
        person_id: row.get::<String>("person_id").unwrap_or_default(),
        email: row.get::<String>("email"),
        roles,
        csrf_token: row.get::<String>("csrf_token").unwrap_or_default(),
        idle_expires_at: row.get::<i64>("idle_expires_at").unwrap_or(0),
        absolute_expires_at: row.get::<i64>("absolute_expires_at").unwrap_or(0),
        auth_time: row.get::<i64>("auth_time"),
    }))
}

/// Slides the idle expiry forward once it is half spent, never past the
/// absolute expiry. Writes are rare: a busy session updates about twice an hour.
///
/// # Errors
/// The database error.
pub async fn slide_session(
    db: &dyn Database,
    session_id: &str,
    row: &SessionRow,
    now: i64,
) -> Result<SessionRow, cratefield_core::DbError> {
    if row.idle_expires_at - now > IDLE_TTL_SECONDS / 2 {
        return Ok(row.clone());
    }
    let idle = (now + IDLE_TTL_SECONDS).min(row.absolute_expires_at);
    db.execute(&Statement::with_values(
        "UPDATE people_session SET idle_expires_at = ? WHERE id_hash = ?",
        vec![
            sea_query::Value::from(idle),
            sea_query::Value::from(session_key_hash(session_id)),
        ],
    ))
    .await?;
    // The member list's "last active" is the time of this write, so it moves
    // at the same rate as the session slides: about twice an hour, not per call.
    members::touch_profile(db, &row.person_id, row.email.as_deref(), now).await?;
    Ok(SessionRow {
        idle_expires_at: idle,
        ..row.clone()
    })
}

/// Deletes a session (logout). Idempotent.
///
/// # Errors
/// The database error.
pub async fn delete_session(
    db: &dyn Database,
    session_id: &str,
) -> Result<(), cratefield_core::DbError> {
    db.execute(&Statement::with_values(
        "DELETE FROM people_session WHERE id_hash = ?",
        vec![sea_query::Value::from(session_key_hash(session_id))],
    ))
    .await
    .map(|_| ())
}

/// The verified person a sign-in produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonClaims {
    pub person_id: String,
    pub email: Option<String>,
    pub roles: Vec<String>,
    /// When the person authenticated at the issuer (the ID token's `auth_time`).
    pub auth_time: Option<i64>,
}

/// Whether a `return_to` value is a path inside the app: a single leading
/// slash, no protocol-relative or backslash form, no control characters.
#[must_use]
pub fn safe_return_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.starts_with("/\\")
        && path.len() <= 512
        && path.chars().all(|c| !c.is_control())
}

/// The session authenticator behind a cookie-presented command: resolves the
/// cookie to a `Principal::Person`, enforcing the origin, the role rule and
/// the expiry, and slides the idle timeout.
pub struct SessionAuthenticator {
    pub db: Arc<dyn Database>,
    pub clock: Arc<dyn Clock>,
    pub app_origins: Vec<String>,
    /// The company whose membership decides the person's role.
    pub company_id: String,
    /// Whether the sign-in's `owner` claim may bootstrap the company's owner.
    /// Only the deployment's own company may be bootstrapped this way: a
    /// company created later gets its owner from its creator, never from a
    /// claim presented against it (docs/permissions.md, "Companies").
    pub bootstrap: bool,
    pub presented: SessionPresented,
}

#[async_trait]
impl Authenticator for SessionAuthenticator {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        let presented = &self.presented;
        check_session_origin(
            &self.app_origins,
            presented.origin.as_deref(),
            presented.safe,
        )
        .map_err(CommandError::Domain)?;
        let session_id = credential.as_str();
        let now = self.clock.now().unix_timestamp();
        let row = load_session(self.db.as_ref(), session_id)
            .await?
            .filter(|row| !row.expired_at(now))
            .ok_or(CommandError::Unauthenticated)?;
        let claims: &[String] = if self.bootstrap { &row.roles } else { &[] };
        let role = members::resolve_role(
            self.db.as_ref(),
            &self.company_id,
            &row.person_id,
            claims,
            OffsetDateTime::from_unix_timestamp(now).unwrap_or(OffsetDateTime::UNIX_EPOCH),
        )
        .await?
        .ok_or_else(|| CommandError::Domain(not_a_member()))?;
        let row = slide_session(self.db.as_ref(), session_id, &row, now).await?;
        Ok(Principal::Person {
            person_id: row.person_id,
            role,
        })
    }
}

/// The 403 for a signed-in person who holds no membership in the company.
#[must_use]
pub fn not_a_member() -> Problem {
    Problem::new(&NOT_A_MEMBER)
        .with_detail("You're not a member of this company yet. Ask the owner for an invitation.")
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

fn no_store(response: &mut Response) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

fn problem_response(problem: Problem) -> Response {
    let mut response = problem.into_response();
    no_store(&mut response);
    response
}

fn append_header(response: &mut Response, name: header::HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().append(name, value);
    }
}

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the operating system entropy source is available");
    b64url(&buf)
}

fn unix_now(state: &PeopleState) -> i64 {
    state.deps.clock.now().unix_timestamp()
}

async fn http_get(http: &dyn HttpClient, url: &str) -> Result<Vec<u8>, String> {
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(url)
        .header(header::ACCEPT, "application/json")
        .body(Bytes::new())
        .map_err(|err| err.to_string())?;
    let response = http.send(request).await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("GET {url} answered {}", response.status()));
    }
    Ok(response.into_body().to_vec())
}

async fn fetch_discovery(
    http: &dyn HttpClient,
    config: &PeopleConfig,
) -> Result<Discovery, OidcError> {
    let url = format!("{}/.well-known/openid-configuration", config.issuer);
    let body = http_get(http, &url).await.map_err(OidcError::Discovery)?;
    oidc::parse_discovery(&body, &config.issuer)
}

async fn fetch_keys(
    http: &dyn HttpClient,
    discovery: &Discovery,
) -> Result<Vec<PublishedKey>, OidcError> {
    let body = http_get(http, &discovery.jwks_uri)
        .await
        .map_err(OidcError::Jwks)?;
    oidc::parse_jwks(&body)
}

async fn exchange_code(
    http: &dyn HttpClient,
    discovery: &Discovery,
    config: &PeopleConfig,
    code: &str,
    verifier: &str,
) -> Result<String, String> {
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", &config.redirect_uri())
        .append_pair("client_id", &config.client_id)
        .append_pair("code_verifier", verifier)
        .finish();
    let request = http::Request::builder()
        .method(http::Method::POST)
        .uri(discovery.token_endpoint.as_str())
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Bytes::from(form))
        .map_err(|err| err.to_string())?;
    let response = http.send(request).await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("token endpoint answered {}", response.status()));
    }
    let body: Value = serde_json::from_slice(response.body()).map_err(|err| err.to_string())?;
    body.get("id_token")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "token response has no id_token".to_owned())
}

/// `GET /v1/auth/login`: starts the authorization-code flow.
async fn login(
    State(state): State<Arc<PeopleState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let (Some(config), Some(http)) = (state.config.as_ref(), state.http.as_ref()) else {
        return problem_response(Problem::new(&SIGN_IN_NOT_CONFIGURED));
    };
    let discovery = match fetch_discovery(http.as_ref(), config).await {
        Ok(discovery) => discovery,
        Err(err) => {
            tracing::warn!(error = %err, "sign-in: discovery failed");
            return problem_response(Problem::new(&SIGN_IN_FAILED));
        }
    };
    let verifier = random_token(48);
    // Every interactive sign-in shows the sign-in page, where a person with a
    // passkey is asked for it: reusing the identity provider's session would let
    // an old email-link sign-in open the books without the passkey. Only an
    // explicit `prompt=none` asks for a silent sign-in.
    let reauthenticate = query.get("prompt").map(String::as_str) != Some("none");
    let login = LoginState {
        state: random_token(32),
        nonce: random_token(32),
        verifier: verifier.clone(),
        exp: unix_now(&state) + LOGIN_TTL_SECONDS,
        return_to: query
            .get("return_to")
            .filter(|path| safe_return_path(path))
            .cloned(),
    };
    let payload = serde_json::to_vec(&login).expect("the login state serializes");
    let sealed = oidc::seal(&config.seal_key, &payload);
    let location = oidc::authorize_url_with(
        &discovery,
        &config.client_id,
        &config.redirect_uri(),
        &login.state,
        &login.nonce,
        &pkce_challenge(&verifier),
        reauthenticate,
    );
    let mut response = (StatusCode::FOUND, "").into_response();
    no_store(&mut response);
    append_header(&mut response, header::LOCATION, &location);
    append_header(&mut response, header::SET_COOKIE, &login_cookie(&sealed));
    response
}

/// What a sign-in records: the company's membership (the first owner's
/// sign-in makes the organization; any other person signs in without a
/// membership and is told so by `session`) and the profile the member list shows.
async fn record_sign_in(
    db: &dyn Database,
    company_id: &str,
    person: &PersonClaims,
    now: i64,
) -> Result<(), DbError> {
    let at = OffsetDateTime::from_unix_timestamp(now).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    members::resolve_role(db, company_id, &person.person_id, &person.roles, at).await?;
    members::touch_profile(db, &person.person_id, person.email.as_deref(), now).await
}

/// `GET /v1/auth/callback`: exchanges the code, verifies the ID token and opens
/// the session.
async fn callback(
    State(state): State<Arc<PeopleState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let (Some(config), Some(http)) = (state.config.as_ref(), state.http.as_ref()) else {
        return problem_response(Problem::new(&SIGN_IN_NOT_CONFIGURED));
    };
    let now = unix_now(&state);
    let fail = |reason: &str| {
        tracing::warn!(reason, "sign-in callback refused");
        let mut response = problem_response(
            Problem::new(&SIGN_IN_FAILED)
                .with_detail("The sign-in could not be completed. Start again from the app."),
        );
        append_header(&mut response, header::SET_COOKIE, &clear_login_cookie());
        response
    };
    if let Some(error) = query.get("error") {
        return fail(&format!("the issuer returned {error}"));
    }
    let Some(login) = cookie_value(&headers, LOGIN_COOKIE)
        .and_then(|sealed| oidc::open(&config.seal_key, &sealed))
        .and_then(|payload| serde_json::from_slice::<LoginState>(&payload).ok())
    else {
        return fail("no valid sign-in transaction cookie");
    };
    if login.exp <= now {
        return fail("sign-in transaction expired");
    }
    let state_param = query.get("state").map_or("", String::as_str);
    if !oidc::ct_eq(state_param.as_bytes(), login.state.as_bytes()) {
        return fail("state does not match");
    }
    let Some(code) = query.get("code").filter(|code| !code.is_empty()) else {
        return fail("no authorization code");
    };

    let discovery = match fetch_discovery(http.as_ref(), config).await {
        Ok(discovery) => discovery,
        Err(err) => return fail(&err.to_string()),
    };
    let id_token =
        match exchange_code(http.as_ref(), &discovery, config, code, &login.verifier).await {
            Ok(token) => token,
            Err(err) => return fail(&format!("token exchange: {err}")),
        };
    let keys = match fetch_keys(http.as_ref(), &discovery).await {
        Ok(keys) => keys,
        Err(err) => return fail(&err.to_string()),
    };
    let claims = match oidc::verify_id_token(
        &id_token,
        &keys,
        &config.issuer,
        &config.client_id,
        &login.nonce,
        now,
    ) {
        Ok(claims) => claims,
        Err(err) => return fail(&format!("ID token: {err}")),
    };

    let person = PersonClaims {
        person_id: claims.sub,
        email: claims.email.filter(|_| claims.email_verified == Some(true)),
        roles: claims.roles,
        auth_time: claims.auth_time,
    };
    let session_id = random_token(32);
    let db = state.deps.db.as_ref();
    if let Err(err) = store_session(db, &session_id, &person, now).await {
        tracing::error!(error = %err, "sign-in: could not store the session");
        return problem_response(Problem::new(&SIGN_IN_FAILED));
    }
    if let Err(err) = record_sign_in(db, &state.company_id, &person, now).await {
        tracing::error!(error = %err, "sign-in: could not record the membership");
        return problem_response(Problem::new(&SIGN_IN_FAILED));
    }
    let mut response = (StatusCode::SEE_OTHER, "").into_response();
    no_store(&mut response);
    let target = login
        .return_to
        .as_deref()
        .filter(|path| safe_return_path(path))
        .map_or_else(
            || config.app_url.clone(),
            |path| format!("{}{path}", config.app_url.trim_end_matches('/')),
        );
    append_header(&mut response, header::LOCATION, &target);
    append_header(
        &mut response,
        header::SET_COOKIE,
        &SessionBuilder::new(session_id).build(),
    );
    append_header(&mut response, header::SET_COOKIE, &clear_login_cookie());
    response
}

/// `POST /v1/auth/logout`: ends the session and clears the cookie.
async fn logout(State(state): State<Arc<PeopleState>>, headers: HeaderMap) -> Response {
    let Some(config) = state.config.as_ref() else {
        return problem_response(Problem::new(&SIGN_IN_NOT_CONFIGURED));
    };
    let origin = origin_header(&headers);
    // Logout changes state, so a named origin must be an app origin. A request
    // with no Origin is only a cookie clear: nothing a cross-site page gains.
    if let Some(origin) = origin.as_deref()
        && allowed_origin(&config.app_origins, origin).is_none()
    {
        return problem_response(Problem::new(&SESSION_ORIGIN_FORBIDDEN));
    }
    if let Some(id) = cookie_value(&headers, SESSION_COOKIE)
        && let Err(err) = delete_session(state.deps.db.as_ref(), &id).await
    {
        tracing::error!(error = %err, "logout: could not delete the session");
        return problem_response(Problem::internal());
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    no_store(&mut response);
    append_header(&mut response, header::SET_COOKIE, &SessionBuilder::clear());
    response
}

/// `GET /v1/auth/session`: the signed-in person, or 401/403.
async fn session(State(state): State<Arc<PeopleState>>, headers: HeaderMap) -> Response {
    let Some(config) = state.config.as_ref() else {
        return problem_response(Problem::new(&SIGN_IN_NOT_CONFIGURED));
    };
    let Some(id) = cookie_value(&headers, SESSION_COOKIE) else {
        return problem_response(Problem::new(&SESSION_REQUIRED));
    };
    let expired = || {
        let mut response = problem_response(Problem::new(&SESSION_REQUIRED));
        append_header(&mut response, header::SET_COOKIE, &SessionBuilder::clear());
        response
    };
    if let Err(problem) = check_session_origin(
        &config.app_origins,
        origin_header(&headers).as_deref(),
        true,
    ) {
        return problem_response(problem);
    }
    // The membership is checked in the company the request names (its
    // `X-Ledgers-Company` header), or the deployment's company when it names none.
    let company = match requested_company(&state.company_id, &headers) {
        Ok(company) => company,
        Err(problem) => return problem_response(problem),
    };
    let now = unix_now(&state);
    let db = state.deps.db.as_ref();
    let row = match load_session(db, &id).await {
        Ok(Some(row)) if !row.expired_at(now) => row,
        Ok(_) => return expired(),
        Err(err) => {
            tracing::error!(error = %err, "session: could not read the session");
            return problem_response(Problem::internal());
        }
    };
    let at = OffsetDateTime::from_unix_timestamp(now).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let role = match members::resolve_role(db, &company, &row.person_id, &row.roles, at).await {
        Ok(Some(role)) => role,
        Ok(None) => return problem_response(not_a_member()),
        Err(err) => {
            tracing::error!(error = %err, "session: could not read the membership");
            return problem_response(Problem::internal());
        }
    };
    let row = match slide_session(db, &id, &row, now).await {
        Ok(row) => row,
        Err(err) => {
            tracing::error!(error = %err, "session: could not slide the session");
            return problem_response(Problem::internal());
        }
    };
    let expires_at = OffsetDateTime::from_unix_timestamp(row.idle_expires_at)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok());
    let mut response = Json(json!({
        "personId": row.person_id,
        "email": row.email,
        "companyId": company,
        "role": role.as_str(),
        "roles": [role.as_str()],
        "csrfToken": row.csrf_token,
        "expiresAt": expires_at,
    }))
    .into_response();
    no_store(&mut response);
    response
}

// ---------------------------------------------------------------------------
// People: the matrix, the member list and invitations
// ---------------------------------------------------------------------------

/// A caller who may manage people is refused with this.
pub const MANAGE_FORBIDDEN: ProblemDef = ProblemDef {
    slug: "people-manage-forbidden",
    status: StatusCode::FORBIDDEN,
    title: "Only an owner or admin can manage people",
    description: "Inviting, changing roles and removing people is for owners and admins.",
};

/// The invitation to revoke is not an open one of this company.
pub const INVITATION_NOT_FOUND: ProblemDef = ProblemDef {
    slug: "invitation-not-found",
    status: StatusCode::NOT_FOUND,
    title: "No open invitation with that id",
    description: "The invitation was already accepted, revoked or has lapsed.",
};

/// The signed-in member a people route acts for, in the company it named.
struct SessionMember {
    role: ledgers_commands::Role,
    company: String,
}

/// Resolves the cookie session to a company member, checking the Origin of a
/// state-changing request. The error is the problem the caller answers with.
async fn session_member(
    state: &PeopleState,
    headers: &HeaderMap,
    safe: bool,
) -> Result<SessionMember, Problem> {
    let Some(config) = state.config.as_ref() else {
        return Err(Problem::new(&SIGN_IN_NOT_CONFIGURED));
    };
    let Some(id) = cookie_value(headers, SESSION_COOKIE) else {
        return Err(Problem::new(&SESSION_REQUIRED));
    };
    check_session_origin(&config.app_origins, origin_header(headers).as_deref(), safe)?;
    let company = requested_company(&state.company_id, headers)?;
    let now = unix_now(state);
    let db = state.deps.db.as_ref();
    let row = match load_session(db, &id).await {
        Ok(Some(row)) if !row.expired_at(now) => row,
        Ok(_) => return Err(Problem::new(&SESSION_REQUIRED)),
        Err(err) => {
            tracing::error!(error = %err, "people: could not read the session");
            return Err(Problem::internal());
        }
    };
    let at = OffsetDateTime::from_unix_timestamp(now).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let role = match members::resolve_role(db, &company, &row.person_id, &row.roles, at).await {
        Ok(Some(role)) => role,
        Ok(None) => return Err(not_a_member()),
        Err(err) => {
            tracing::error!(error = %err, "people: could not read the membership");
            return Err(Problem::internal());
        }
    };
    if let Err(err) = slide_session(db, &id, &row, now).await {
        tracing::error!(error = %err, "people: could not slide the session");
        return Err(Problem::internal());
    }
    Ok(SessionMember { role, company })
}

/// The member-management check: owners and admins only.
fn may_manage_people(member: &SessionMember) -> Result<(), Problem> {
    if members::may_manage(member.role) {
        Ok(())
    } else {
        Err(Problem::new(&MANAGE_FORBIDDEN))
    }
}

/// An epoch-seconds instant as RFC 3339 text, for the app.
fn rfc3339_or_none(epoch: Option<i64>) -> Option<String> {
    epoch
        .and_then(|secs| OffsetDateTime::from_unix_timestamp(secs).ok())
        .and_then(|at| at.format(&Rfc3339).ok())
}

/// `GET /v1/auth/roles`: the permission matrix, for the read-only panel.
async fn roles(State(state): State<Arc<PeopleState>>, headers: HeaderMap) -> Response {
    if let Err(problem) = session_member(&state, &headers, true).await {
        return problem_response(problem);
    }
    let roles: Vec<Value> = ledgers_policy::roles::matrix()
        .into_iter()
        .map(|(role, classes)| {
            json!({
                "role": role.as_str(),
                "classes": classes.into_iter().map(ledgers_policy::roles::Class::name).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut response = Json(json!({ "roles": roles })).into_response();
    no_store(&mut response);
    response
}

/// `GET /v1/auth/people`: every member of the company, with the email and
/// last activity the company shows. One query for the list, whatever its size.
async fn people(State(state): State<Arc<PeopleState>>, headers: HeaderMap) -> Response {
    let member = match session_member(&state, &headers, true).await {
        Ok(member) => member,
        Err(problem) => return problem_response(problem),
    };
    let rows = match members::members(state.deps.db.as_ref(), &member.company).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(error = %err, "people: could not list the members");
            return problem_response(Problem::internal());
        }
    };
    let list: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "personId": row.person_id,
                "email": row.email,
                "role": row.role,
                "invitedBy": row.invited_by,
                "joinedAt": row.joined_at,
                "lastActiveAt": rfc3339_or_none(row.last_active_at),
            })
        })
        .collect();
    let mut response = Json(json!({
        "members": list,
        "me": { "role": member.role.as_str() },
        "canManage": members::may_manage(member.role),
    }))
    .into_response();
    no_store(&mut response);
    response
}

/// `GET /v1/auth/invitations`: the invitations still open. The address is
/// not shown: the company keeps only its hash.
async fn invitations(State(state): State<Arc<PeopleState>>, headers: HeaderMap) -> Response {
    let member = match session_member(&state, &headers, true).await {
        Ok(member) => member,
        Err(problem) => return problem_response(problem),
    };
    if let Err(problem) = may_manage_people(&member) {
        return problem_response(problem);
    }
    let now = state.deps.clock.now();
    let rows =
        match members::pending_invitations(state.deps.db.as_ref(), &member.company, now).await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::error!(error = %err, "people: could not list the invitations");
                return problem_response(Problem::internal());
            }
        };
    let list: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "role": row.role,
                "invitedBy": row.invited_by,
                "createdAt": row.created_at,
                "expiresAt": row.expires_at,
            })
        })
        .collect();
    let mut response = Json(json!({ "invitations": list })).into_response();
    no_store(&mut response);
    response
}

/// `DELETE /v1/auth/invitations/{id}`: revokes an open invitation.
async fn revoke_invitation_route(
    State(state): State<Arc<PeopleState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let member = match session_member(&state, &headers, false).await {
        Ok(member) => member,
        Err(problem) => return problem_response(problem),
    };
    if let Err(problem) = may_manage_people(&member) {
        return problem_response(problem);
    }
    match members::revoke_invitation(state.deps.db.as_ref(), &member.company, &id).await {
        Ok(true) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            no_store(&mut response);
            response
        }
        Ok(false) => problem_response(Problem::new(&INVITATION_NOT_FOUND)),
        Err(err) => {
            tracing::error!(error = %err, "people: could not revoke the invitation");
            problem_response(Problem::internal())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_value_finds_the_named_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; __Host-ledgers-session=abc; other=1"),
        );
        assert_eq!(
            cookie_value(&headers, SESSION_COOKIE).as_deref(),
            Some("abc")
        );
        assert_eq!(cookie_value(&headers, "missing"), None);
    }

    #[test]
    fn bearer_wins_over_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer lk_live_x"),
        );
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-ledgers-session=abc"),
        );
        assert_eq!(
            presented_from(&headers, false),
            Some(Presented::Bearer("lk_live_x".into()))
        );
    }

    #[test]
    fn origin_rule_requires_origin_for_state_changes() {
        let allowed = vec!["https://app.example".to_owned()];
        assert!(check_session_origin(&allowed, Some("https://app.example"), false).is_ok());
        assert!(check_session_origin(&allowed, Some("https://evil.example"), false).is_err());
        assert!(check_session_origin(&allowed, None, false).is_err());
        assert!(check_session_origin(&allowed, None, true).is_ok());
    }

    #[test]
    fn expiry_is_absolute_or_idle() {
        let row = SessionRow {
            person_id: "p".into(),
            email: None,
            roles: vec![],
            csrf_token: String::new(),
            idle_expires_at: 100,
            absolute_expires_at: 1_000,
            auth_time: None,
        };
        assert!(!row.expired_at(99));
        assert!(row.expired_at(100));
    }
}

#[cfg(test)]
mod return_path_tests {
    use super::safe_return_path;

    #[test]
    fn return_to_stays_inside_the_app() {
        assert!(safe_return_path("/approvals/apr-1"));
        assert!(!safe_return_path("//evil.example/x"));
        assert!(!safe_return_path("/\\evil.example"));
        assert!(!safe_return_path("https://evil.example"));
        assert!(!safe_return_path("approvals/apr-1"));
        assert!(!safe_return_path("/a\nb"));
    }
}
