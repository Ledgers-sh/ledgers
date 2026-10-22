//! The company's people (docs/permissions.md): the owner bootstrap, the
//! membership a signed-in person needs, invitations (invite, accept, revoke),
//! roles, the last-owner rule, ownership transfer, the role matrix over HTTP,
//! segregation of duties on approvals, agent keys across membership changes,
//! and the constant statement count of the member list.
//!
//! Sessions are stored directly (what a finished sign-in leaves behind), so
//! these tests do not repeat the OIDC flow `people.rs` covers.
#![forbid(unsafe_code)]
#![allow(dead_code)]

use cratefield_core::axum::body::to_bytes;
use cratefield_core::axum::http::{HeaderMap, Method, Request, StatusCode, header};
use cratefield_core::{Clock, IdGen, Module, Ports, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::module::DbModule;
use ledgers_ledger::LedgerModule;
use ledgers_policy::module::PolicyModule;
use ledgers_server::dispatch::{self, ApiKeysModule, Deps, ServerDispatch};
use ledgers_server::members::MembersModule;
use ledgers_server::people::{self, PeopleConfig, PeopleModule, PersonClaims};
use ledgers_server::routes::{self, Dispatch};
use ledgers_server::timing::{CountingDb, DbTiming};
use serde_json::{Value, json};
use std::sync::Arc;
use time::OffsetDateTime;
use tower::ServiceExt;

const COMPANY: &str = dispatch::DEV_COMPANY_ID;
const APP: &str = "https://app.test";
const API: &str = "https://api.test";
const START: i64 = 1_792_674_000; // 2026-10-22T10:00:00Z
const SESSION_COOKIE: &str = "__Host-ledgers-session";

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
        format!("01MEMBERS{n:016}")
    }
}

struct Fx {
    router: cratefield_core::axum::Router,
    deps: Deps,
    timing: Arc<DbTiming>,
    /// The bearer token of the development key the fixture bootstraps.
    dev_key: String,
    /// The fake mailer: the invitation mail is read from here.
    mailer: cratefield_testing::FakeMailer,
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
        Arc::new(ServerDispatch::new(COMPANY).with_app_origins(vec![APP.to_owned()]));
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
        Box::new(PeopleModule::new(Some(config()), COMPANY)),
        Box::new(MembersModule::new(vec![APP.to_owned()])),
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
            // Every statement the router runs is counted, so a test can say what
            // a request costs.
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
    let mailer = kit.mailer.clone();
    let db = kit.db.clone();
    let deps = Deps {
        db: db.clone(),
        clock: clock.clone(),
        id_gen: ids,
        rate_limiter: None,
        blob: None,
        http: None,
    };
    pollster::block_on(db.execute(&Statement::with_values(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT (id) DO NOTHING".to_owned(),
        vec![
            sea_query::Value::from(COMPANY),
            sea_query::Value::from("Halden Works GmbH"),
            sea_query::Value::from("EUR"),
            sea_query::Value::from(START),
            sea_query::Value::from(START),
        ],
    )))
    .expect("company row");
    let dev_key = pollster::block_on(dispatch::bootstrap_dev(&deps, COMPANY)).expect("bootstrap");
    Fx {
        router: kit.router.clone(),
        deps,
        timing,
        dev_key,
        mailer,
    }
}

/// One request through the router. Returns the status, headers and body.
fn send(
    fx: &Fx,
    method: Method,
    path: &str,
    session: Option<&str>,
    origin: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(id) = session {
        builder = builder.header(header::COOKIE, format!("{SESSION_COOKIE}={id}"));
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

/// A signed-in session for `sub`, stored the way a finished sign-in stores it.
fn session(fx: &Fx, sub: &str, email: Option<&str>, claims: &[&str]) -> String {
    let id = format!("sess-{sub}");
    let now = fx.deps.clock.now().unix_timestamp();
    let person = PersonClaims {
        person_id: sub.to_owned(),
        email: email.map(str::to_owned),
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

/// Gives `sub` a membership with `role` directly (the setup a finished invite
/// leaves behind), so a matrix test does not depend on the invitation flow.
fn add_member(fx: &Fx, sub: &str, role: &str) {
    let now = fx
        .deps
        .clock
        .now()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    pollster::block_on(fx.deps.db.execute(&Statement::with_values(
        "INSERT INTO org_members (org_id, user_sub, role, invited_by, created_at) VALUES (?, ?, ?, NULL, ?)",
        vec![
            sea_query::Value::from(COMPANY),
            sea_query::Value::from(sub),
            sea_query::Value::from(role),
            sea_query::Value::from(now),
        ],
    )))
    .expect("insert membership");
}

/// The owner: the first person whose sign-in carries the owner claim makes the
/// company's organization and becomes its owner.
fn owner(fx: &Fx) -> String {
    let id = session(fx, "owner-a", Some("owner@halden.test"), &["owner"]);
    let (status, _, body) = send(fx, Method::GET, "/v1/auth/session", Some(&id), None, None);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["role"], "owner");
    id
}

/// Invites `email` as `role` from the session `from`, then accepts it as `sub`
/// (a session for the invitee with that address). Returns the invitee's session.
fn invite_and_accept(fx: &Fx, from: &str, sub: &str, email: &str, role: &str) -> String {
    let (status, _, body) = send(
        fx,
        Method::POST,
        &format!("/v1/orgs/{COMPANY}/invitations"),
        Some(from),
        Some(APP),
        Some(json!({ "email": email, "role": role })),
    );
    assert!(status.is_success(), "invite: {status} {body}");
    let token = invitation_token(fx);
    let id = session(fx, sub, Some(email), &[]);
    let (status, _, body) = send(
        fx,
        Method::POST,
        "/v1/orgs/invitations/accept",
        Some(&id),
        Some(APP),
        Some(json!({ "token": token })),
    );
    assert!(status.is_success(), "accept: {status} {body}");
    id
}

/// The raw invitation token from the last mail the fake mailer sent: the
/// accept link is `...accept?token=<token>` in the message body.
fn invitation_token(fx: &Fx) -> String {
    let mail = fx.mailer.last_message().expect("an invitation was mailed");
    let start = mail.text.find("token=").expect("the mail carries a token") + "token=".len();
    mail.text[start..]
        .chars()
        .take_while(|c| !matches!(c, ' ' | '\n' | '\r' | '"' | '<' | '>' | '&' | ')' | ']'))
        .collect()
}

fn people_of(fx: &Fx, session: &str) -> Value {
    let (status, _, body) = send(
        fx,
        Method::GET,
        "/v1/auth/people",
        Some(session),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn problem_type(body: &Value) -> String {
    body["type"].as_str().unwrap_or_default().to_owned()
}

/// The entry body the dispatch tests post (a balanced EUR entry).
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

#[test]
fn the_first_owner_sign_in_makes_the_company_owner_and_no_one_else_does() {
    let fx = fixture();
    let first = owner(&fx);
    let second = session(&fx, "owner-b", Some("other@halden.test"), &["owner"]);
    let (status, _, body) = send(
        &fx,
        Method::GET,
        "/v1/auth/session",
        Some(&second),
        None,
        None,
    );
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(problem_type(&body).ends_with("not-a-member"));
    // Signing in again changes nothing: one owner, still the first.
    let (status, _, _) = send(
        &fx,
        Method::GET,
        "/v1/auth/session",
        Some(&first),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK);
    let owners = people_of(&fx, &first)["members"]
        .as_array()
        .expect("members")
        .iter()
        .filter(|m| m["role"] == "owner")
        .count();
    assert_eq!(owners, 1);
}

#[test]
fn a_signed_in_person_without_a_membership_is_told_so_on_every_people_route() {
    let fx = fixture();
    owner(&fx);
    let stranger = session(&fx, "stranger", Some("stranger@x.test"), &[]);
    for path in [
        "/v1/auth/session",
        "/v1/auth/people",
        "/v1/auth/roles",
        "/v1/auth/invitations",
        "/v1/entries",
    ] {
        let (status, _, body) = send(&fx, Method::GET, path, Some(&stranger), None, None);
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
        assert!(
            problem_type(&body).ends_with("not-a-member"),
            "{path}: {body}"
        );
    }
}

#[test]
fn an_invitation_is_mailed_accepted_and_grants_the_invited_role() {
    let fx = fixture();
    let o = owner(&fx);
    let ann = invite_and_accept(&fx, &o, "ann", "ann@halden.test", "accountant");
    let (status, _, body) = send(&fx, Method::GET, "/v1/auth/session", Some(&ann), None, None);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["role"], "accountant");
    let people = people_of(&fx, &o);
    let ann_row = people["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|m| m["personId"] == "ann")
        .cloned()
        .expect("ann is listed");
    assert_eq!(ann_row["role"], "accountant");
    assert_eq!(ann_row["invitedBy"], "owner-a");
}

#[test]
fn accepting_needs_the_invited_address_and_the_link_works_once() {
    let fx = fixture();
    let o = owner(&fx);
    let (status, _, body) = send(
        &fx,
        Method::POST,
        &format!("/v1/orgs/{COMPANY}/invitations"),
        Some(&o),
        Some(APP),
        Some(json!({ "email": "ann@halden.test", "role": "viewer" })),
    );
    assert!(status.is_success(), "{body}");
    let token = invitation_token(&fx);
    let bob = session(&fx, "bob", Some("bob@halden.test"), &[]);
    let (status, _, _) = send(
        &fx,
        Method::POST,
        "/v1/orgs/invitations/accept",
        Some(&bob),
        Some(APP),
        Some(json!({ "token": token })),
    );
    assert!(!status.is_success(), "a different address cannot accept");
    let (status, _, _) = send(&fx, Method::GET, "/v1/auth/session", Some(&bob), None, None);
    assert_eq!(status, StatusCode::FORBIDDEN, "bob gained nothing");

    let ann = session(&fx, "ann", Some("ann@halden.test"), &[]);
    let accept = || {
        send(
            &fx,
            Method::POST,
            "/v1/orgs/invitations/accept",
            Some(&ann),
            Some(APP),
            Some(json!({ "token": token })),
        )
    };
    let (status, _, body) = accept();
    assert!(status.is_success(), "{body}");
    let (status, _, _) = accept();
    assert!(
        !status.is_success(),
        "a spent invitation cannot be spent again"
    );
}

#[test]
fn an_invitation_can_be_listed_revoked_and_its_link_stops_working() {
    let fx = fixture();
    let o = owner(&fx);
    let (status, _, _) = send(
        &fx,
        Method::POST,
        &format!("/v1/orgs/{COMPANY}/invitations"),
        Some(&o),
        Some(APP),
        Some(json!({ "email": "carl@halden.test", "role": "auditor" })),
    );
    assert!(status.is_success());
    let token = invitation_token(&fx);
    let (status, _, body) = send(
        &fx,
        Method::GET,
        "/v1/auth/invitations",
        Some(&o),
        None,
        None,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    let open = body["invitations"].as_array().expect("invitations");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["role"], "auditor");
    assert!(open[0].get("email").is_none(), "the address is not listed");
    let id = open[0]["id"].as_str().expect("id").to_owned();

    let (status, _, _) = send(
        &fx,
        Method::DELETE,
        &format!("/v1/auth/invitations/{id}"),
        Some(&o),
        Some(APP),
        None,
    );
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, body) = send(
        &fx,
        Method::DELETE,
        &format!("/v1/auth/invitations/{id}"),
        Some(&o),
        Some(APP),
        None,
    );
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(problem_type(&body).ends_with("invitation-not-found"));
    let (_, _, body) = send(
        &fx,
        Method::GET,
        "/v1/auth/invitations",
        Some(&o),
        None,
        None,
    );
    assert!(body["invitations"].as_array().expect("list").is_empty());

    let carl = session(&fx, "carl", Some("carl@halden.test"), &[]);
    let (status, _, _) = send(
        &fx,
        Method::POST,
        "/v1/orgs/invitations/accept",
        Some(&carl),
        Some(APP),
        Some(json!({ "token": token })),
    );
    assert!(!status.is_success(), "a revoked link does not accept");
}

#[test]
fn only_owners_and_admins_manage_invitations_and_people() {
    let fx = fixture();
    let o = owner(&fx);
    let viewer = invite_and_accept(&fx, &o, "vee", "vee@halden.test", "viewer");
    let (status, _, body) = send(
        &fx,
        Method::GET,
        "/v1/auth/invitations",
        Some(&viewer),
        None,
        None,
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(problem_type(&body).ends_with("people-manage-forbidden"));
    let (status, _, _) = send(
        &fx,
        Method::DELETE,
        "/v1/auth/invitations/any",
        Some(&viewer),
        Some(APP),
        None,
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = send(
        &fx,
        Method::POST,
        &format!("/v1/orgs/{COMPANY}/invitations"),
        Some(&viewer),
        Some(APP),
        Some(json!({ "email": "x@halden.test", "role": "viewer" })),
    );
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the module refuses a non-manager"
    );
}

#[test]
fn the_last_owner_cannot_be_removed_or_demoted() {
    let fx = fixture();
    let o = owner(&fx);
    let (status, _, body) = send(
        &fx,
        Method::DELETE,
        &format!("/v1/orgs/{COMPANY}/members/owner-a"),
        Some(&o),
        Some(APP),
        None,
    );
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(problem_type(&body).ends_with("orgs-last-owner"));
    let (status, _, body) = send(
        &fx,
        Method::PATCH,
        &format!("/v1/orgs/{COMPANY}/members/owner-a"),
        Some(&o),
        Some(APP),
        Some(json!({ "role": "admin" })),
    );
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(problem_type(&body).ends_with("orgs-last-owner"));
    let (_, _, body) = send(&fx, Method::GET, "/v1/auth/session", Some(&o), None, None);
    assert_eq!(body["role"], "owner", "still the owner");
}

#[test]
fn ownership_moves_by_promoting_the_new_owner_before_demoting_yourself() {
    let fx = fixture();
    let o = owner(&fx);
    let b = invite_and_accept(&fx, &o, "owner-b", "other@halden.test", "owner");
    let (status, _, body) = send(
        &fx,
        Method::PATCH,
        &format!("/v1/orgs/{COMPANY}/members/owner-a"),
        Some(&o),
        Some(APP),
        Some(json!({ "role": "admin" })),
    );
    assert!(status.is_success(), "{body}");
    let (_, _, body) = send(&fx, Method::GET, "/v1/auth/session", Some(&o), None, None);
    assert_eq!(body["role"], "admin");
    let (_, _, body) = send(&fx, Method::GET, "/v1/auth/session", Some(&b), None, None);
    assert_eq!(body["role"], "owner");
}

#[test]
fn an_admin_manages_members_but_cannot_grant_or_change_an_owner() {
    let fx = fixture();
    let o = owner(&fx);
    let ann = invite_and_accept(&fx, &o, "ann", "ann@halden.test", "admin");
    let ben = invite_and_accept(&fx, &ann, "ben", "ben@halden.test", "viewer");
    let _ = ben;
    let patch = |role: &str, sub: &str| {
        send(
            &fx,
            Method::PATCH,
            &format!("/v1/orgs/{COMPANY}/members/{sub}"),
            Some(&ann),
            Some(APP),
            Some(json!({ "role": role })),
        )
    };
    let (status, _, body) = patch("accountant", "ben");
    assert!(status.is_success(), "an admin re-roles a viewer: {body}");
    let (status, _, _) = patch("owner", "ben");
    assert_eq!(status, StatusCode::FORBIDDEN, "an admin cannot grant owner");
    let (status, _, _) = send(
        &fx,
        Method::PATCH,
        &format!("/v1/orgs/{COMPANY}/members/owner-a"),
        Some(&ann),
        Some(APP),
        Some(json!({ "role": "viewer" })),
    );
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an admin cannot demote an owner"
    );
    let (status, _, _) = send(
        &fx,
        Method::DELETE,
        &format!("/v1/orgs/{COMPANY}/members/ben"),
        Some(&ann),
        Some(APP),
        None,
    );
    assert!(status.is_success(), "an admin removes a member");
}

#[test]
fn a_removed_member_loses_access_on_the_next_request() {
    let fx = fixture();
    let o = owner(&fx);
    let ann = invite_and_accept(&fx, &o, "ann", "ann@halden.test", "accountant");
    let (status, _, _) = send(&fx, Method::GET, "/v1/auth/session", Some(&ann), None, None);
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = send(
        &fx,
        Method::DELETE,
        &format!("/v1/orgs/{COMPANY}/members/ann"),
        Some(&o),
        Some(APP),
        None,
    );
    assert!(status.is_success(), "{body}");
    // The session is still live; the membership is what is checked.
    let (status, _, body) = send(&fx, Method::GET, "/v1/entries", Some(&ann), None, None);
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(problem_type(&body).ends_with("not-a-member"));
}

#[test]
fn the_role_matrix_is_enforced_over_http_for_every_role() {
    let fx = fixture();
    let o = owner(&fx);
    // (method, path, body, roles that may run it). The owner and admin may run
    // everything; the rest is the matrix in ledgers-policy.
    let cases: [(Method, &str, Option<Value>, &[&str]); 5] = [
        (
            Method::GET,
            "/v1/entries",
            None,
            &[
                "owner",
                "admin",
                "accountant",
                "approver",
                "viewer",
                "auditor",
            ],
        ),
        (
            Method::POST,
            "/v1/entries",
            Some(entry_json("matrix-1")),
            &["owner", "admin", "accountant"],
        ),
        (Method::GET, "/v1/keys", None, &["owner", "admin"]),
        (
            Method::GET,
            "/v1/audit-events",
            None,
            &["owner", "admin", "auditor"],
        ),
        (
            Method::GET,
            "/v1/approval-requests",
            None,
            &["owner", "admin", "approver", "viewer", "auditor"],
        ),
    ];
    let roles = [
        "owner",
        "admin",
        "accountant",
        "approver",
        "viewer",
        "auditor",
    ];
    for role in roles {
        let sub = format!("m-{role}");
        let who = if role == "owner" {
            o.clone()
        } else {
            add_member(&fx, &sub, role);
            session(&fx, &sub, Some(&format!("{sub}@halden.test")), &[])
        };
        for (method, path, body, allowed) in &cases {
            let (status, _, reply) = send(
                &fx,
                method.clone(),
                path,
                Some(&who),
                Some(APP),
                body.clone(),
            );
            if allowed.contains(&role) {
                assert_ne!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{role} {method} {path}: {reply}"
                );
            } else {
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{role} {method} {path}: {reply}"
                );
                assert!(
                    problem_type(&reply).ends_with("policy-denied"),
                    "{role} {method} {path}: {reply}"
                );
            }
        }
    }
}

#[test]
fn a_person_cannot_decide_an_approval_their_own_agent_key_raised() {
    let fx = fixture();
    let o = owner(&fx);
    let (status, _, key) = send(
        &fx,
        Method::POST,
        "/v1/keys",
        Some(&o),
        Some(APP),
        Some(json!({ "name": "ops-agent", "tier": "post-within-limits", "limits": {} })),
    );
    assert!(status.is_success(), "{key}");
    let token = key["token"]
        .as_str()
        .expect("the token is returned once")
        .to_owned();
    let (status, _, raised) = send_bearer(&fx, "/v1/entries", &token, &entry_json("sod-1"));
    assert!(status.is_success(), "{raised}");
    assert_eq!(raised["status"], "needs-approval", "{raised}");
    let id = raised["approval"]["id"]
        .as_str()
        .expect("approval id")
        .to_owned();

    let (status, _, body) = send(
        &fx,
        Method::POST,
        &format!("/v1/approval-requests/{id}/decision"),
        Some(&o),
        Some(APP),
        Some(json!({ "decision": "approve" })),
    );
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        problem_type(&body).ends_with("segregation-of-duties"),
        "{body}"
    );

    // Another approver may decide it, and the approved entry posts for them.
    add_member(&fx, "ap", "approver");
    let ap = session(&fx, "ap", Some("ap@halden.test"), &[]);
    let (status, _, body) = send(
        &fx,
        Method::POST,
        &format!("/v1/approval-requests/{id}/decision"),
        Some(&ap),
        Some(APP),
        Some(json!({ "decision": "approve" })),
    );
    assert!(status.is_success(), "{body}");
    assert_eq!(body["approval"]["status"], "approved", "{body}");
}

/// A request with a bearer token and a JSON body.
fn send_bearer(fx: &Fx, path: &str, token: &str, body: &Value) -> (StatusCode, HeaderMap, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(cratefield_core::axum::body::Body::from(body.to_string()))
        .expect("request");
    pollster::block_on(async {
        let response = fx.router.clone().oneshot(request).await.expect("router");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    })
}

#[test]
fn agent_keys_keep_working_when_the_person_who_made_them_leaves() {
    let fx = fixture();
    let o = owner(&fx);
    let _second = invite_and_accept(&fx, &o, "owner-b", "other@halden.test", "owner");
    let (status, _, key) = send(
        &fx,
        Method::POST,
        "/v1/keys",
        Some(&o),
        Some(APP),
        Some(json!({ "name": "reader", "tier": "read", "limits": {} })),
    );
    assert!(status.is_success(), "{key}");
    let token = key["token"].as_str().expect("token").to_owned();
    let (status, _, body) = send(
        &fx,
        Method::POST,
        &format!("/v1/orgs/{COMPANY}/leave"),
        Some(&o),
        Some(APP),
        None,
    );
    assert!(status.is_success(), "the owner leaves: {body}");
    let (status, _, body) = send_get_bearer(&fx, "/v1/entries", &token);
    assert_eq!(status, StatusCode::OK, "the key still works: {body}");
}

fn send_get_bearer(fx: &Fx, path: &str, token: &str) -> (StatusCode, HeaderMap, Value) {
    let request = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(cratefield_core::axum::body::Body::empty())
        .expect("request");
    pollster::block_on(async {
        let response = fx.router.clone().oneshot(request).await.expect("router");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    })
}

#[test]
fn the_member_list_costs_the_same_statements_however_many_members_it_has() {
    let fx = fixture();
    let o = owner(&fx);
    let cost = |fx: &Fx| {
        let before = fx.timing.snapshot().statements;
        let (status, _, body) = send(fx, Method::GET, "/v1/auth/people", Some(&o), None, None);
        assert_eq!(status, StatusCode::OK, "{body}");
        fx.timing.snapshot().statements - before
    };
    let small = cost(&fx);
    for n in 0..25 {
        add_member(&fx, &format!("bulk-{n}"), "viewer");
    }
    let large = cost(&fx);
    assert_eq!(
        small, large,
        "the member list runs the same statements for 1 member or 26"
    );
    assert!(
        large <= 8,
        "the member list is a fixed set of queries, got {large}"
    );
}

#[test]
fn a_person_cannot_create_an_organization_because_the_company_has_one() {
    let fx = fixture();
    let o = owner(&fx);
    let (status, _, body) = send(
        &fx,
        Method::POST,
        "/v1/orgs",
        Some(&o),
        Some(APP),
        Some(json!({ "name": "Second company" })),
    );
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(problem_type(&body).ends_with("org-create-not-offered"));
}

#[test]
fn a_cookie_change_to_the_organization_needs_an_app_origin() {
    let fx = fixture();
    let o = owner(&fx);
    let body = Some(json!({ "role": "admin" }));
    let (status, _, _) = send(
        &fx,
        Method::PATCH,
        &format!("/v1/orgs/{COMPANY}/members/owner-a"),
        Some(&o),
        None,
        body.clone(),
    );
    assert_eq!(status, StatusCode::FORBIDDEN, "no origin");
    let (status, _, reply) = send(
        &fx,
        Method::PATCH,
        &format!("/v1/orgs/{COMPANY}/members/owner-a"),
        Some(&o),
        Some("https://evil.test"),
        body,
    );
    assert_eq!(status, StatusCode::FORBIDDEN, "{reply}");
    assert!(problem_type(&reply).ends_with("session-origin-forbidden"));
}

#[test]
fn the_roles_panel_lists_each_role_with_its_classes() {
    let fx = fixture();
    let o = owner(&fx);
    let (status, _, body) = send(&fx, Method::GET, "/v1/auth/roles", Some(&o), None, None);
    assert_eq!(status, StatusCode::OK, "{body}");
    let roles = body["roles"].as_array().expect("roles");
    let names: Vec<&str> = roles.iter().map(|r| r["role"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "owner",
            "admin",
            "accountant",
            "approver",
            "viewer",
            "auditor"
        ]
    );
    let auditor = roles
        .iter()
        .find(|r| r["role"] == "auditor")
        .expect("auditor");
    let classes: Vec<&str> = auditor["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(classes.contains(&"audit"));
    assert!(!classes.contains(&"post"));
}

#[test]
fn a_member_row_shows_the_address_and_when_the_person_was_last_active() {
    let fx = fixture();
    let o = owner(&fx);
    let ann = invite_and_accept(&fx, &o, "ann", "ann@halden.test", "viewer");
    let now = fx.deps.clock.now().unix_timestamp();
    pollster::block_on(ledgers_server::members::touch_profile(
        fx.deps.db.as_ref(),
        "ann",
        Some("ann@halden.test"),
        now,
    ))
    .expect("profile");
    let people = people_of(&fx, &ann);
    let row = people["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["personId"] == "ann")
        .cloned()
        .expect("ann");
    assert_eq!(row["email"], "ann@halden.test");
    assert!(row["lastActiveAt"].as_str().is_some(), "{row}");
    assert_eq!(
        people["canManage"], false,
        "a viewer does not manage people"
    );
    assert_eq!(people["me"]["role"], "viewer");
}

#[test]
fn every_registered_command_has_a_role_rule() {
    // A command with no class is refused for every role, so a new command that
    // is registered without one would be unusable. This keeps the matrix whole.
    for descriptor in dispatch::registry().iter() {
        assert!(
            ledgers_policy::roles::class_of(&descriptor.name).is_some(),
            "{} has no role rule in ledgers-policy/src/roles.rs",
            descriptor.name
        );
    }
}
