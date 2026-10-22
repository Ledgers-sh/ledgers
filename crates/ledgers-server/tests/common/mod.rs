//! Shared fixture for the dispatch tests: the real registry, the real
//! domain modules and the real `ServerDispatch`, over the harness's
//! migrated SQLite database. Only the clock and the id generator are fixed,
//! so periods and ids are deterministic.
#![allow(dead_code)]

use cratefield_core::{IdGen, Module, Ports};
use cratefield_testing::{MemoryBlob, TestHarness, request, request_as};
use ledgers_commands::auth::Tier;
use ledgers_ledger::LedgerModule;
use ledgers_policy::key::CurrencyLimit;
use ledgers_server::dispatch::{self, ApiKeysModule, Deps, ServerDispatch};
use ledgers_server::people::{PersonClaims, store_session};
use ledgers_server::routes::{self, Dispatch};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub mod http;

pub use cratefield_core::axum::http::Method;
pub use cratefield_testing::TestResponse;

/// The download-link key every fixture signs with (invented, test only).
pub const DOCUMENT_KEY: &[u8] = b"fixture-document-signing-key-0123456789";
/// The company every fixture serves: the dev bootstrap's fixture id.
pub const COMPANY: &str = dispatch::DEV_COMPANY_ID;
/// The fixed clock: a date inside the bootstrap's fiscal year.
pub const NOW: &str = "2026-10-22T10:00:00Z";

/// Deterministic ids: a counter behind a fixed prefix.
#[derive(Default)]
pub struct Ids(AtomicU64);

impl IdGen for Ids {
    fn ulid(&self) -> String {
        let n = self.0.fetch_add(1, Ordering::Relaxed) + 1;
        format!("01TESTID{n:018X}")
    }
}

/// The keys a fixture issues.
pub struct Keys {
    /// The dev key: full tier, EUR limits (what `LEDGERS_DEV=1` prints).
    pub full: String,
    /// A draft-tier key: may read and draft, may not post.
    pub draft: String,
    /// A read-tier key: may only read.
    pub read: String,
    /// A post-tier key with no limit in any currency: every post is denied.
    pub unlimited: String,
}

/// One running fixture.
pub struct Fixture {
    /// The router the HTTP tests send requests through.
    pub kit: TestHarness,
    /// The blob store the documents are kept in.
    pub blob: Arc<MemoryBlob>,
    /// The ports the dispatch runs on (same database as the router).
    pub deps: Deps,
    /// The issued keys.
    pub keys: Keys,
}

impl Fixture {
    pub fn router(&self) -> &cratefield_core::axum::Router {
        &self.kit.router
    }

    /// Sends `path` with the bearer `key`.
    pub fn send(&self, method: Method, path: &str, key: &str, json: Option<&str>) -> TestResponse {
        pollster::block_on(request_as(self.router(), method, path, key, json))
    }

    /// Sends `path` with no credential at all.
    pub fn send_anonymous(&self, method: Method, path: &str, json: Option<&str>) -> TestResponse {
        pollster::block_on(request(self.router(), method, path, json))
    }

    /// Posts a balanced entry of `amount` minor units, debit `debit` and
    /// credit `credit`, under `external_id`, as the full key.
    pub fn post(&self, external_id: &str, debit: &str, credit: &str, minor: &str) -> TestResponse {
        self.send(
            Method::POST,
            "/v1/entries",
            &self.keys.full,
            Some(&entry_json(external_id, debit, credit, minor).to_string()),
        )
    }
}

/// The JSON body of a two-line EUR entry dated in the fixture's period.
pub fn entry_json(external_id: &str, debit: &str, credit: &str, minor: &str) -> serde_json::Value {
    serde_json::json!({
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
                {"account_number": debit, "side": "debit", "base": {"minor": minor, "currency": "EUR"},
                 "fx": null, "tax_code": null, "dimensions": {}, "memo": null},
                {"account_number": credit, "side": "credit", "base": {"minor": minor, "currency": "EUR"},
                 "fx": null, "tax_code": null, "dimensions": {}, "memo": null}
            ]
        }
    })
}

/// Builds the fixture: the harness, the bootstrap, and the four keys.
pub fn fixture() -> Fixture {
    fixture_with(Vec::new())
}

/// [`fixture`] with extra modules mounted beside the domain modules.
pub fn fixture_with(extra: Vec<Arc<dyn Module>>) -> Fixture {
    fixture_with_options(extra, None, Vec::new())
}

/// [`fixture`] with a rate limiter on the ports (agent keys are admitted
/// through it) and the app origins cookie sessions may come from.
pub fn fixture_with_options(
    extra: Vec<Arc<dyn Module>>,
    limiter: Option<Arc<dyn cratefield_core::RateLimiter>>,
    app_origins: Vec<String>,
) -> Fixture {
    let registry = dispatch::registry();
    let server_dispatch = ServerDispatch::new(COMPANY)
        .with_app_origins(app_origins)
        .with_document_key(DOCUMENT_KEY.to_vec());
    let dispatch: Arc<dyn Dispatch> = Arc::new(server_dispatch);
    let mut domain = routes::domain_modules(&registry, &dispatch).expect("domain modules");
    // The documents module mounts the `documents` command routes and the upload
    // and download routes.
    let document_routes = routes::domain_routes(&registry, "documents");
    domain.push(Arc::new(ledgers_server::documents::DocumentsModule::new(
        COMPANY,
        DOCUMENT_KEY.to_vec(),
        dispatch.clone(),
        document_routes,
    )));
    domain.extend(extra);

    let clock = Arc::new(cratefield_testing::FixedClock(
        time::OffsetDateTime::parse(NOW, &time::format_description::well_known::Rfc3339)
            .expect("fixed clock time"),
    ));
    let ids = Arc::new(Ids::default());
    let builtin: Vec<Box<dyn Module>> = vec![
        Box::new(LedgerModule),
        Box::new(ledgers_db::module::DbModule),
        Box::new(ledgers_policy::module::PolicyModule),
        Box::new(ledgers_storage::StorageModule),
        Box::new(ledgers_approvals::module::ApprovalsModule),
        Box::new(ledgers_approvals::module::ApprovalsInboxModule),
        Box::new(ledgers_approvals::module::ApprovalsNotifyModule),
        Box::new(ApiKeysModule),
        // Carries the session table the owner sessions are stored in.
        Box::new(ledgers_server::people::PeopleModule::new(None, COMPANY)),
        Box::new(ledgers_server::members::MembersModule::new(vec![
            "https://app.test".to_owned(),
        ])),
        Box::new(ledgers_documents::DocumentsStoreModule),
        Box::new(ledgers_bills::BillsModule),
        Box::new(ledgers_bank::BankModule),
        Box::new(ledgers_tax::module::TaxModule),
    ];
    let blob = Arc::new(MemoryBlob::new());
    let (patch_clock, patch_ids) = (clock.clone(), ids.clone());
    let patch_limiter = limiter.clone();
    let patch_clock_for_auth: Arc<dyn cratefield_core::Clock> = clock.clone();
    let patch_blob = blob.clone();
    let kit = TestHarness::with_builder(
        builtin,
        move |builder| {
            domain
                .into_iter()
                .fold(builder, cratefield_core::HarnessBuilder::module_arc)
        },
        move |ports: &mut Ports| {
            ports.clock = Some(patch_clock as Arc<dyn cratefield_core::Clock>);
            ports.id_gen = Some(patch_ids as Arc<dyn IdGen>);
            ports.rate_limiter = patch_limiter;
            ports.blob = Some(patch_blob as Arc<dyn cratefield_core::Blob>);
            // The organization module identifies the person by the session cookie.
            ports.auth = ports.db.clone().map(|db| {
                Arc::new(ledgers_server::members::SessionIdentity::new(
                    db,
                    patch_clock_for_auth.clone(),
                )) as Arc<dyn cratefield_core::Auth>
            });
        },
    );
    let deps = Deps {
        db: kit.db.clone(),
        clock,
        id_gen: ids,
        rate_limiter: limiter,
        blob: Some(blob.clone() as Arc<dyn cratefield_core::Blob>),
        http: None,
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
            sea_query::Value::from(NOW),
            sea_query::Value::from(NOW),
        ],
    )))
    .expect("company row");
    let full = pollster::block_on(dispatch::bootstrap_dev(&deps, COMPANY)).expect("bootstrap");
    let draft = issue(&deps, Tier::Draft, eur_limits());
    let read = issue(&deps, Tier::Read, eur_limits());
    let unlimited = issue(&deps, Tier::PostWithinLimits, BTreeMap::new());
    Fixture {
        kit,
        blob,
        deps,
        keys: Keys {
            full,
            draft,
            read,
            unlimited,
        },
    }
}

/// Generous EUR limits, as the dev bootstrap grants.
pub fn eur_limits() -> BTreeMap<String, CurrencyLimit> {
    BTreeMap::from([(
        "EUR".to_owned(),
        CurrencyLimit {
            per_entry: 100_000_000_000_000,
            per_day: 1_000_000_000_000_000,
        },
    )])
}

fn issue(deps: &Deps, tier: Tier, limits: BTreeMap<String, CurrencyLimit>) -> String {
    pollster::block_on(dispatch::issue_key(
        deps,
        COMPANY,
        "fixture key",
        tier,
        limits,
    ))
    .expect("issue key")
}

/// The body of a response, parsed.
pub fn json(response: &TestResponse) -> serde_json::Value {
    response.json()
}

/// The session id the owner test session is stored under.
pub const OWNER_SESSION: &str = "owner-session-0001";
/// The owner's person id, as a session records it.
pub const OWNER_PERSON: &str = "person-owner";

/// Stores a signed-in owner session in the fixture's database. The session
/// is what a passkey-less owner sign-in leaves behind; it expires after the
/// fixture's clock says so, which the fixed clock never does.
pub fn owner_session(fx: &Fixture) {
    let now = fx.deps.clock.now().unix_timestamp();
    let claims = PersonClaims {
        person_id: OWNER_PERSON.to_owned(),
        email: Some("owner@example.test".to_owned()),
        roles: vec!["owner".to_owned()],
        auth_time: Some(now),
    };
    pollster::block_on(store_session(
        fx.deps.db.as_ref(),
        OWNER_SESSION,
        &claims,
        now,
    ))
    .expect("store owner session");
}
