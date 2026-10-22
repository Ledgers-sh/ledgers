//! Several companies in one deployment (docs/permissions.md, "Companies").
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
use cratefield_core::{Clock, IdGen, Module, Ports, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::module::DbModule;
use ledgers_ledger::LedgerModule;
use ledgers_policy::module::PolicyModule;
use ledgers_server::companies::CompaniesModule;
use ledgers_server::dispatch::{self, ApiKeysModule, Deps, ServerDispatch};
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
        Box::new(CompaniesModule::new(vec![APP.to_owned()])),
        Box::new(ledgers_documents::DocumentsStoreModule),
        Box::new(ledgers_bills::BillsModule),
        Box::new(ledgers_tax::module::TaxModule),
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
            ports.db = Some(Arc::new(CountingDb::new(inner, patch_timing)));
            let counted = ports.db.clone().expect("counted database");
            ports.auth = Some(Arc::new(ledgers_server::members::SessionIdentity::new(
                counted,
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

/// The slug of a problem document: the last segment of its `type` URL.
fn problem_type(body: &Value) -> String {
    body["type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn entry_json(external_id: &str) -> Value {
    json!({
        "entry": {
            "journal_id": "",
            "document_date": "2026-10-15",
            "posting_date": "2026-10-15",
            "tax_point_date": null,
            "base_currency": "EUR",
            "description": "Invoice INV-1001 to Acme GmbH",
            "source_documents": [],
            "external_id": external_id,
            "source_app": "fixture",
            "reverses": null,
            "lines": [
                {"account_number": "5010", "side": "debit", "base": {"minor": "100000", "currency": "EUR"},
                 "fx": null, "tax_code": null, "dimensions": {}, "memo": null},
                {"account_number": "4000", "side": "credit", "base": {"minor": "100000", "currency": "EUR"},
                 "fx": null, "tax_code": null, "dimensions": {}, "memo": null}
            ]
        }
    })
}

fn external_ids(body: &Value) -> Vec<String> {
    body["entries"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["external_id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_person_is_served_only_the_companies_they_belong_to() {
    let fx = fixture();
    let ana = session(&fx, "ana", &[]);
    add_member(&fx, HOME, "ana", "owner");
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/companies",
        As::Cookie(&ana),
        None,
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["companies"]
        .as_array()
        .expect("companies")
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec![HOME]);

    let second = create_company(&fx, &ana, "Factory Zero (test)", "nl");
    let (_, _, body) = call(
        &fx,
        Method::GET,
        "/v1/companies",
        As::Cookie(&ana),
        None,
        None,
        None,
    );
    let ids: Vec<&str> = body["companies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&HOME) && ids.contains(&second.as_str()),
        "{body}"
    );
    let created = body["companies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == second.as_str())
        .expect("created company listed");
    assert_eq!(created["role"], "owner");
}

#[test]
fn a_member_of_one_company_is_refused_on_the_other() {
    let fx = fixture();
    add_member(&fx, HOME, "bob", "accountant");
    let bob = session(&fx, "bob", &[]);
    let ana = session(&fx, "ana", &["owner"]);
    add_member(&fx, HOME, "ana", "owner");
    let other = create_company(&fx, &ana, "Second company", "generic");

    // Bob reads his own company fine.
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Cookie(&bob),
        Some(HOME),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    // Bob is not a member of the other company: refused, never served.
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Cookie(&bob),
        Some(&other),
        None,
        None,
    );
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(problem_type(&body), "not-a-member", "{body}");
    // The same holds for a write.
    let (status, _, body) = call(
        &fx,
        Method::POST,
        "/v1/entries",
        As::Cookie(&bob),
        Some(&other),
        Some(APP),
        Some(entry_json("bob-on-other")),
    );
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(problem_type(&body), "not-a-member", "{body}");
    // Ana, owner of both, is served the other company.
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Cookie(&ana),
        Some(&other),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[test]
fn entries_of_one_company_never_show_in_another() {
    let fx = fixture();
    let ana = session(&fx, "ana", &["owner"]);
    add_member(&fx, HOME, "ana", "owner");
    let other = create_company(&fx, &ana, "Second company", "generic");

    let (status, _, body) = call(
        &fx,
        Method::POST,
        "/v1/entries",
        As::Cookie(&ana),
        Some(&other),
        Some(APP),
        Some(entry_json("only-in-other")),
    );
    assert!(status.is_success(), "post into other: {status} {body}");

    let (_, _, in_other) = call(
        &fx,
        Method::GET,
        "/v1/entries",
        As::Cookie(&ana),
        Some(&other),
        None,
        None,
    );
    assert_eq!(
        external_ids(&in_other),
        vec!["only-in-other".to_owned()],
        "{in_other}"
    );
    let (status, _, in_home) = call(
        &fx,
        Method::GET,
        "/v1/entries",
        As::Cookie(&ana),
        None,
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{in_home}");
    assert!(
        !external_ids(&in_home).contains(&"only-in-other".to_owned()),
        "{in_home}"
    );
}

#[test]
fn an_agent_key_is_bound_to_its_own_company() {
    let fx = fixture();
    // Positive control: the key serves its own company.
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Bearer(&fx.home_key),
        None,
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    // Named against another company it is not a key of that company: refused.
    let ana = session(&fx, "ana", &["owner"]);
    add_member(&fx, HOME, "ana", "owner");
    let other = create_company(&fx, &ana, "Second company", "generic");
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Bearer(&fx.home_key),
        Some(&other),
        None,
        None,
    );
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, _, body) = call(
        &fx,
        Method::POST,
        "/v1/entries",
        As::Bearer(&fx.home_key),
        Some(&other),
        None,
        Some(entry_json("agent-on-other")),
    );
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    // And the person's own access is unaffected.
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Cookie(&ana),
        Some(&other),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[test]
fn an_owner_claim_never_bootstraps_a_company_it_was_not_made_for() {
    let fx = fixture();
    let eve = session(&fx, "eve", &["owner"]);
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Cookie(&eve),
        Some("01NOSUCHCOMPANY"),
        None,
        None,
    );
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(problem_type(&body), "not-a-member", "{body}");
    let rows = pollster::block_on(fx.deps.db.query(&Statement::with_values(
        "SELECT COUNT(*) AS n FROM orgs WHERE id = ?",
        vec![sea_query::Value::from("01NOSUCHCOMPANY")],
    )))
    .expect("count orgs");
    let n: i64 = rows.rows[0].get("n").expect("count");
    assert_eq!(n, 0, "no organization may be created by a header");
}

#[test]
fn an_invalid_company_header_is_refused_before_any_query() {
    let fx = fixture();
    let ana = session(&fx, "ana", &["owner"]);
    for bad in ["has space", "x/../y", &"a".repeat(65)] {
        let (status, _, body) = call(
            &fx,
            Method::GET,
            "/v1/accounts",
            As::Cookie(&ana),
            Some(bad),
            None,
            None,
        );
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert_eq!(problem_type(&body), "invalid-company", "{body}");
    }
}

#[test]
fn creating_a_company_needs_a_session_an_origin_and_valid_input() {
    let fx = fixture();
    let good = json!({"name": "Ok", "baseCurrency": "EUR", "countryPack": "nl", "fiscalYearStartMonth": 1});
    let (status, _, _) = call(
        &fx,
        Method::POST,
        "/v1/companies",
        As::Anonymous,
        None,
        Some(APP),
        Some(good.clone()),
    );
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let ana = session(&fx, "ana", &["owner"]);
    let (status, _, _) = call(
        &fx,
        Method::POST,
        "/v1/companies",
        As::Cookie(&ana),
        None,
        Some("https://evil.test"),
        Some(good.clone()),
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
    for bad in [
        json!({"name": "  ", "baseCurrency": "EUR", "countryPack": "nl", "fiscalYearStartMonth": 1}),
        json!({"name": "X", "baseCurrency": "eur", "countryPack": "nl", "fiscalYearStartMonth": 1}),
        json!({"name": "X", "baseCurrency": "EUR", "countryPack": "xx", "fiscalYearStartMonth": 1}),
        json!({"name": "X", "baseCurrency": "EUR", "countryPack": "nl", "fiscalYearStartMonth": 13}),
    ] {
        let (status, _, body) = call(
            &fx,
            Method::POST,
            "/v1/companies",
            As::Cookie(&ana),
            None,
            Some(APP),
            Some(bad),
        );
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
}

#[test]
fn a_created_company_starts_from_its_country_chart_and_fiscal_year() {
    let fx = fixture();
    let ana = session(&fx, "ana", &["owner"]);
    add_member(&fx, HOME, "ana", "owner");
    let nl = create_company(&fx, &ana, "Dutch BV", "nl");
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Cookie(&ana),
        Some(&nl),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/fiscal-years",
        As::Cookie(&ana),
        Some(&nl),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.to_string().contains("2026"),
        "a fiscal year opens for the current year: {body}"
    );
}

#[test]
fn the_demo_owner_is_granted_the_demo_company_once() {
    let fx = fixture();
    let demo = "01DEMOFACTORYZERO00000000A";
    pollster::block_on(dispatch::ensure_company(
        &fx.deps,
        demo,
        "Factory Zero (demo)",
        "EUR",
    ))
    .expect("demo company");
    // The home company's owner: the bootstrap owner, signed in with the claim.
    add_member(&fx, HOME, "boot", "owner");
    let owner = session(&fx, "boot", &[]);
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Cookie(&owner),
        None,
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    pollster::block_on(ledgers_server::demo::grant_bootstrap_owner(
        &fx.deps, HOME, demo,
    ))
    .expect("grant the demo owner");
    pollster::block_on(ledgers_server::demo::grant_bootstrap_owner(
        &fx.deps, HOME, demo,
    ))
    .expect("a second grant changes nothing");
    let (_, _, listed) = call(
        &fx,
        Method::GET,
        "/v1/companies",
        As::Cookie(&owner),
        None,
        None,
        None,
    );
    let names: Vec<&str> = listed["companies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Factory Zero (demo)"), "{listed}");
    let (status, _, body) = call(
        &fx,
        Method::GET,
        "/v1/accounts",
        As::Cookie(&owner),
        Some(demo),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
}
