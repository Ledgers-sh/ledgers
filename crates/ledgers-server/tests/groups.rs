//! Groups and intercompany agreements over HTTP (issue #128): who may run a
//! group command, and what a person who belongs to one company is shown of another.
//!
//! A request names its company with `X-Ledgers-Company`. These tests prove the
//! isolation the header relies on: a person is served only the companies they
//! hold a membership in, a person or agent key of one company is refused on
//! another, a signed-in owner claim never bootstraps a company it was not
//! created for, and creating a company makes its creator the owner.
#![forbid(unsafe_code)]
#![allow(dead_code)]

use cratefield_core::axum::body::to_bytes;
use cratefield_core::axum::http::{HeaderMap, Method, Request, StatusCode, header};
use cratefield_core::{Clock, Database, IdGen, Module, Ports, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::module::DbModule;
use ledgers_ledger::LedgerModule;
use ledgers_policy::module::PolicyModule;
use ledgers_server::companies::{CompaniesModule, DEFAULT_COMPANIES_PER_PERSON};
use ledgers_server::dispatch::{self, ApiKeysModule, Deps, ServerDispatch};
use ledgers_server::mcp::McpModule;
use ledgers_server::members::MembersModule;
use ledgers_server::people::{self, PeopleConfig, PeopleModule, PersonClaims};
use ledgers_server::routes::{self, Dispatch};
use ledgers_server::timing::{CountingDb, DbTiming};
use serde_json::{Value, json};
use std::sync::Arc;
use time::OffsetDateTime;
use tower::ServiceExt;

const HOME: &str = dispatch::DEV_COMPANY_ID;
const APP: &str = "https://app.test";
const API: &str = "https://api.test";
const START: i64 = 1_792_674_000; // 2026-10-22T10:00:00Z
const SESSION_COOKIE: &str = "__Host-ledgers-session";
const COMPANY_HEADER: &str = "x-ledgers-company";

/// A clock the tests hold still.
struct Fixed(OffsetDateTime);

impl Clock for Fixed {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

#[derive(Default)]
struct Ids(std::sync::atomic::AtomicU64);

impl IdGen for Ids {
    fn ulid(&self) -> String {
        let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("01COMPANIES{n:016}")
    }
}

struct Fx {
    router: cratefield_core::axum::Router,
    deps: Deps,
    /// The bearer token of the development key the fixture bootstraps for HOME.
    home_key: String,
}

fn config() -> PeopleConfig {
    PeopleConfig {
        issuer: "https://issuer.test".to_owned(),
        client_id: "ledgers-api-test".to_owned(),
        api_url: API.to_owned(),
        app_url: APP.to_owned(),
        app_origins: vec![APP.to_owned()],
        seal_key: ledgers_auth::oidc::derive_key(
            b"test-harness-secret",
            "ledgers-people-login-cookie",
        ),
    }
}

fn fixture() -> Fx {
    fixture_with(DEFAULT_COMPANIES_PER_PERSON)
}

/// The fixture with `limit` companies allowed per person.
fn fixture_with(limit: usize) -> Fx {
    let clock: Arc<dyn Clock> = Arc::new(Fixed(
        OffsetDateTime::from_unix_timestamp(START).expect("valid time"),
    ));
    let ids: Arc<dyn IdGen> = Arc::new(Ids::default());
    let registry = dispatch::registry();
    let dispatch_impl: Arc<dyn Dispatch> =
        Arc::new(ServerDispatch::new(HOME).with_app_origins(vec![APP.to_owned()]));
    let domain = routes::domain_modules(&registry, &dispatch_impl).expect("domain modules");
    let timing = Arc::new(DbTiming::default());
    let builtin: Vec<Box<dyn Module>> = vec![
        Box::new(LedgerModule),
        Box::new(DbModule),
        Box::new(PolicyModule),
        Box::new(ledgers_storage::StorageModule),
        Box::new(ledgers_approvals::module::ApprovalsModule),
        Box::new(ledgers_approvals::module::ApprovalsInboxModule),
        Box::new(ledgers_approvals::module::ApprovalsNotifyModule),
        Box::new(ApiKeysModule),
        Box::new(PeopleModule::new(Some(config()), HOME)),
        Box::new(MembersModule::new(vec![APP.to_owned()])),
        Box::new(CompaniesModule::new(vec![APP.to_owned()]).with_company_limit(limit)),
        Box::new(McpModule::new(
            Arc::new(registry.clone()),
            dispatch_impl.clone(),
            API.to_owned(),
        )),
        Box::new(ledgers_documents::DocumentsStoreModule),
        Box::new(ledgers_bills::BillsModule),
        Box::new(ledgers_tax::module::TaxModule),
        Box::new(ledgers_group::GroupModule),
    ];
    let patch_clock = clock.clone();
    let patch_ids = ids.clone();
    let patch_timing = timing.clone();
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
            let inner = ports
                .db
                .clone()
                .expect("the testing kit provides a database");
            let counted: Arc<dyn Database> = Arc::new(CountingDb::new(inner, patch_timing));
            let served: Arc<dyn Database> = counted;
            ports.db = Some(served.clone());
            ports.auth = Some(Arc::new(ledgers_server::members::SessionIdentity::new(
                served,
                ports.clock.clone().expect("clock"),
            )));
        },
    );
    let db = kit.db.clone();
    let deps = Deps {
        db: db.clone(),
        clock: clock.clone(),
        id_gen: ids,
        rate_limiter: None,
        blob: None,
        http: None,
    };
    pollster::block_on(dispatch::ensure_company(
        &deps,
        HOME,
        "Halden Works GmbH",
        "EUR",
    ))
    .expect("home company row");
    let home_key = pollster::block_on(dispatch::bootstrap_dev(&deps, HOME)).expect("bootstrap");
    Fx {
        router: kit.router.clone(),
        deps,
        home_key,
    }
}

/// How a request presents its caller.
#[derive(Clone, Copy)]
enum As<'a> {
    Cookie(&'a str),
    Bearer(&'a str),
    Anonymous,
}

/// One request through the router, optionally naming a company.
fn call(
    fx: &Fx,
    method: Method,
    path: &str,
    who: As,
    company: Option<&str>,
    origin: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    match who {
        As::Cookie(id) => {
            builder = builder.header(header::COOKIE, format!("{SESSION_COOKIE}={id}"));
        }
        As::Bearer(token) => {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        As::Anonymous => {}
    }
    if let Some(company) = company {
        builder = builder.header(COMPANY_HEADER, company);
    }
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
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
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, headers, value)
    })
}

/// A session for `sub`, stored the way a finished sign-in stores it.
fn session(fx: &Fx, sub: &str, claims: &[&str]) -> String {
    let id = format!("sess-{sub}");
    let now = fx.deps.clock.now().unix_timestamp();
    let person = PersonClaims {
        person_id: sub.to_owned(),
        email: None,
        roles: claims.iter().map(|c| (*c).to_owned()).collect(),
        auth_time: Some(now),
    };
    pollster::block_on(people::store_session(
        fx.deps.db.as_ref(),
        &id,
        &person,
        now,
    ))
    .expect("store session");
    id
}

/// Gives `sub` a membership of `company` with `role`.
fn add_member(fx: &Fx, company: &str, sub: &str, role: &str) {
    let now = fx
        .deps
        .clock
        .now()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    pollster::block_on(fx.deps.db.execute(&Statement::with_values(
        "INSERT INTO org_members (org_id, user_sub, role, invited_by, created_at) VALUES (?, ?, ?, NULL, ?)",
        vec![
            sea_query::Value::from(company),
            sea_query::Value::from(sub),
            sea_query::Value::from(role),
            sea_query::Value::from(now),
        ],
    )))
    .expect("insert membership");
}

/// Creates a company as `creator` through the API and returns its id.
fn create_company(fx: &Fx, creator_session: &str, name: &str, pack: &str) -> String {
    let (status, _, body) = call(
        fx,
        Method::POST,
        "/v1/companies",
        As::Cookie(creator_session),
        None,
        Some(APP),
        Some(json!({
            "name": name,
            "baseCurrency": "EUR",
            "countryPack": pack,
            "fiscalYearStartMonth": 1,
        })),
    );
    assert_eq!(status, StatusCode::CREATED, "create company: {body}");
    body["id"].as_str().expect("company id").to_owned()
}

const GROUP: &str = "01HTTPGROUP000000000000A";
const SECOND: &str = "01HTTPCOMPANYSECOND000000";

/// A group the owner creates over the API: the home company and a second company.
fn group_body(second: &str) -> Value {
    json!({
        "id": GROUP,
        "name": "Factory Zero Group (test)",
        "members": [
            { "company_id": HOME, "kind": "operating", "ownership_bp": 10000 },
            { "company_id": second, "kind": "ip_holding", "ownership_bp": 10000 }
        ]
    })
}

#[test]
fn group_commands_are_for_people_and_an_agent_key_is_refused() {
    let fx = fixture();
    let (status, _, body) = call(
        &fx,
        Method::POST,
        "/v1/groups",
        As::Bearer(&fx.home_key),
        Some(HOME),
        None,
        Some(group_body(SECOND)),
    );
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(problem_slug(&body), "human-only", "{body}");
}

#[test]
fn a_group_shows_a_member_only_the_companies_the_member_belongs_to() {
    let fx = fixture();
    let ana = session(&fx, "ana", &[]);
    add_member(&fx, HOME, "ana", "owner");
    let second = create_company(&fx, &ana, "Factory Zero IP (test)", "sg");
    let ben = session(&fx, "ben", &[]);
    add_member(&fx, HOME, "ben", "admin");

    // Ana owns both companies, so she may create the group.
    let (status, _, body) = call(
        &fx,
        Method::POST,
        "/v1/groups",
        As::Cookie(&ana),
        Some(HOME),
        Some(APP),
        Some(group_body(&second)),
    );
    assert!(status.is_success(), "create group: {status} {body}");
    assert_eq!(body["members"].as_array().map(Vec::len), Some(2), "{body}");

    // Ben is an admin of the home company only: he may read, but not create.
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/groups",
        As::Cookie(&ben),
        Some(HOME),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    let groups = body["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{body}");
    assert_eq!(
        groups[0]["members"].as_array().map(Vec::len),
        Some(1),
        "{body}"
    );
    assert_eq!(groups[0]["has_hidden_members"], json!(true), "{body}");
    assert!(
        !body.to_string().contains(&second),
        "the other company is not named: {body}"
    );

    let (status, _, body) = call(
        &fx,
        Method::POST,
        "/v1/groups",
        As::Cookie(&ben),
        Some(HOME),
        Some(APP),
        Some(group_body(&second)),
    );
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "only an owner creates a group: {body}"
    );

    let (status, _, body) = call(
        &fx,
        Method::GET,
        &format!("/v1/groups/{GROUP}/view?period=2026-01&as_of=2026-01-31"),
        As::Cookie(&ben),
        Some(HOME),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["members"].as_array().map(Vec::len), Some(1), "{body}");
    assert_eq!(body["members"][0]["company"]["id"], json!(HOME), "{body}");
    assert!(
        body["consolidated"]
            .as_str()
            .unwrap_or_default()
            .contains("Ledgers Cloud")
    );
}

#[test]
fn a_person_outside_the_group_gets_nothing_of_it() {
    let fx = fixture();
    let ana = session(&fx, "ana", &[]);
    add_member(&fx, HOME, "ana", "owner");
    let second = create_company(&fx, &ana, "Factory Zero IP (test)", "sg");
    let (status, _, body) = call(
        &fx,
        Method::POST,
        "/v1/groups",
        As::Cookie(&ana),
        Some(HOME),
        Some(APP),
        Some(group_body(&second)),
    );
    assert!(status.is_success(), "{body}");

    // Cara is a member of nothing: the company header itself is refused.
    let cara = session(&fx, "cara", &[]);
    let (status, _, body) = call(
        &fx,
        Method::GET,
        &format!("/v1/groups/{GROUP}/view?period=2026-01&as_of=2026-01-31"),
        As::Cookie(&cara),
        Some(HOME),
        None,
        None,
    );
    assert!(
        !status.is_success(),
        "a stranger reads no group: {status} {body}"
    );
    assert!(!body.to_string().contains("Factory Zero Group"), "{body}");
}

fn problem_slug(body: &Value) -> String {
    body["type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}
