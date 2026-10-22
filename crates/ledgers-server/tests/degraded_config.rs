//! Optional configuration never stops the server. A composition built with a
//! named reader that has no key, an unknown reader, a missing or short
//! document signing key, or a wrong sign-in issuer still builds, serves the
//! rest of the API, and answers 503 only on the routes that feature owns.
//! Required configuration (the company id outside development) still fails.

use cratefield_adapter_sqlite::SqliteDatabase;
use cratefield_core::Config;
use cratefield_core::Database;
use cratefield_core::axum::http::Method;
use cratefield_runtime_native::{DirBlob, Native};
use cratefield_testing::request;
use ledgers_server::dispatch::{self, CompanyBootstrap, Deps, registry};
use ledgers_server::{Composition, compose};
use std::collections::BTreeMap;
use std::sync::Arc;

const COMPANY: &str = "01DEGRADEDCOMPANY000000A";
const DOCUMENT_KEY: &str = "degraded-test-document-signing-key-0123";
const MINIMAX_KEY: &str = "test-minimax-key-not-real";

struct MapConfig(BTreeMap<&'static str, &'static str>);

impl Config for MapConfig {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).map(|value| (*value).to_owned())
    }
}

/// The base configuration every test starts from: a company, and a signing
/// key that is long enough. Each test overrides the variable it is about.
fn base(overrides: &[(&'static str, &'static str)]) -> MapConfig {
    let mut values = BTreeMap::from([
        ("LEDGERS_COMPANY_ID", COMPANY),
        ("LEDGERS_DOCUMENT_SIGNING_KEY", DOCUMENT_KEY),
    ]);
    for (key, value) in overrides {
        values.insert(key, value);
    }
    MapConfig(values)
}

fn native() -> (Arc<SqliteDatabase>, Native) {
    let db = Arc::new(SqliteDatabase::in_memory().expect("in-memory sqlite"));
    let native = Native::new()
        .db_arc(db.clone() as Arc<dyn Database>)
        .auth_arc(Arc::new(ledgers_server::members::SessionIdentity::new(
            db.clone() as Arc<dyn Database>,
            Arc::new(cratefield_core::SystemClock),
        )))
        .blob_arc(DirBlob::arc(concat!(
            env!("CARGO_TARGET_TMPDIR"),
            "/degraded-blobs"
        )));
    (db, native)
}

/// Composes the harness over a migrated, empty database.
fn compose_migrated(cfg: &MapConfig) -> (Composition, Native, Arc<SqliteDatabase>) {
    let (db, native) = native();
    let composition = compose(&registry(), cfg, native.clone()).expect("the server composes");
    for module in composition.harness.modules() {
        db.apply_migrations(module.name(), module.migrations().sqlite)
            .expect("migrations apply");
    }
    (composition, native, db)
}

/// Bootstraps the company and returns its one full-tier key.
fn bootstrap_key(native: &Native) -> String {
    let deps = Deps::from_ports(&native.ports()).expect("db, clock and id generator");
    pollster::block_on(dispatch::bootstrap_company(
        &deps,
        COMPANY,
        &CompanyBootstrap {
            company: "Halden Works GmbH".to_owned(),
            currency: "EUR".to_owned(),
        },
    ))
    .expect("the company bootstraps")
}

fn send(
    composition: &Composition,
    native: &Native,
    method: Method,
    path: &str,
) -> (u16, serde_json::Value) {
    let router = composition.harness.router(native.ports());
    let response = pollster::block_on(request(&router, method, path, None));
    let status = response.status.as_u16();
    let body = if response.status.is_success() {
        response.json()
    } else {
        serde_json::Value::Null
    };
    (status, body)
}

fn send_with_key(composition: &Composition, native: &Native, path: &str, key: &str) -> u16 {
    let router = composition.harness.router(native.ports());
    let response = pollster::block_on(cratefield_testing::request_as(
        &router,
        Method::GET,
        path,
        key,
        None,
    ));
    response.status.as_u16()
}

#[test]
fn a_named_reader_without_its_key_builds_and_serves_the_api() {
    let cfg = base(&[("LEDGERS_EXTRACT_PROVIDER", "minimax")]);
    let (composition, native, _db) = compose_migrated(&cfg);
    let key = bootstrap_key(&native);

    assert_eq!(
        send(&composition, &native, Method::GET, "/v1/openapi").0,
        200
    );
    assert_eq!(
        send_with_key(&composition, &native, "/v1/accounts", &key),
        200
    );

    let (status, body) = send(&composition, &native, Method::GET, "/v1/status");
    assert_eq!(status, 200);
    assert_eq!(body["extraction"]["provider"], "rules");
    assert_eq!(body["extraction"]["configured"], "minimax");
    assert_eq!(body["extraction"]["reason"], "missing_key");
}

#[test]
fn the_fallback_warns_once_and_names_the_variable_not_a_value() {
    let cfg = base(&[("LEDGERS_EXTRACT_PROVIDER", "minimax")]);
    let (composition, _native, _db) = compose_migrated(&cfg);
    let warnings = composition.warnings();
    assert_eq!(
        warnings.len(),
        1,
        "one warning for the fallback: {warnings:?}"
    );
    assert!(warnings[0].contains("LEDGERS_EXTRACT_PROVIDER"));
    assert!(warnings[0].contains("minimax"));
}

#[test]
fn a_named_reader_with_its_key_is_in_effect_and_nothing_is_warned() {
    let cfg = base(&[
        ("LEDGERS_EXTRACT_PROVIDER", "minimax"),
        ("LEDGERS_MINIMAX_API_KEY", MINIMAX_KEY),
    ]);
    let (composition, native, _db) = compose_migrated(&cfg);
    assert!(composition.warnings().is_empty());
    let (_, body) = send(&composition, &native, Method::GET, "/v1/status");
    assert_eq!(body["extraction"]["provider"], "minimax");
    assert_eq!(body["extraction"]["reason"], serde_json::Value::Null);
    let serialized = body.to_string();
    assert!(
        !serialized.contains(MINIMAX_KEY),
        "the key never reaches the status read"
    );
}

#[test]
fn an_unknown_reader_falls_back_to_the_rules_and_serves_the_api() {
    let cfg = base(&[("LEDGERS_EXTRACT_PROVIDER", "openai")]);
    let (composition, native, _db) = compose_migrated(&cfg);
    let key = bootstrap_key(&native);
    assert_eq!(
        send_with_key(&composition, &native, "/v1/accounts", &key),
        200
    );
    let (_, body) = send(&composition, &native, Method::GET, "/v1/status");
    assert_eq!(body["extraction"]["provider"], "rules");
    assert_eq!(body["extraction"]["configured"], "openai");
    assert_eq!(body["extraction"]["reason"], "unknown_provider");
}

#[test]
fn a_missing_document_key_takes_only_the_download_routes_down() {
    let cfg = MapConfig(BTreeMap::from([("LEDGERS_COMPANY_ID", COMPANY)]));
    let (composition, native, _db) = compose_migrated(&cfg);
    let key = bootstrap_key(&native);

    assert_eq!(
        send_with_key(&composition, &native, "/v1/accounts", &key),
        200
    );
    let download = "/v1/documents/01DOC/file?expires=4102444800&signature=deadbeef";
    assert_eq!(send(&composition, &native, Method::GET, download).0, 503);

    let (_, body) = send(&composition, &native, Method::GET, "/v1/status");
    let unavailable = body["unavailable"].as_array().expect("a list");
    assert!(unavailable.iter().any(|item| {
        item["feature"] == "document_links"
            && item["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("LEDGERS_DOCUMENT_SIGNING_KEY")
    }));
}

#[test]
fn an_empty_key_never_verifies_a_download_link() {
    // An HMAC under an empty key is forgeable, so a link signed with one must
    // not download anything while the key is off.
    let cfg = MapConfig(BTreeMap::from([("LEDGERS_COMPANY_ID", COMPANY)]));
    let (composition, native, _db) = compose_migrated(&cfg);
    let expires = 4_102_444_800_i64;
    let forged = ledgers_documents::sign::sign(&[], COMPANY, "01DOC", "abc", expires);
    let path =
        format!("/v1/documents/01DOC/file?company={COMPANY}&expires={expires}&signature={forged}");
    assert_eq!(send(&composition, &native, Method::GET, &path).0, 503);
}

#[test]
fn a_short_document_key_is_refused_by_name_not_by_crash() {
    let cfg = base(&[("LEDGERS_DOCUMENT_SIGNING_KEY", "too-short")]);
    let (composition, native, _db) = compose_migrated(&cfg);
    let key = bootstrap_key(&native);
    assert_eq!(
        send_with_key(&composition, &native, "/v1/accounts", &key),
        200
    );
    assert!(
        composition
            .warnings()
            .iter()
            .any(|w| w.contains("LEDGERS_DOCUMENT_SIGNING_KEY must be at least 32 characters"))
    );
}

#[test]
fn a_wrong_sign_in_issuer_turns_sign_in_off_and_nothing_else() {
    let cfg = base(&[("LEDGERS_AUTH_ISSUER", "ftp://not-a-web-issuer")]);
    let (composition, native, _db) = compose_migrated(&cfg);
    let key = bootstrap_key(&native);

    assert_eq!(
        send(&composition, &native, Method::GET, "/v1/auth/login").0,
        503
    );
    assert_eq!(
        send_with_key(&composition, &native, "/v1/accounts", &key),
        200
    );
    assert!(
        composition
            .warnings()
            .iter()
            .any(|w| w.starts_with("sign_in is off"))
    );
}

#[test]
fn a_missing_company_is_still_refused_and_names_the_variable() {
    let cfg = MapConfig(BTreeMap::new());
    let (_db, native) = native();
    let Err(err) = compose(&registry(), &cfg, native) else {
        panic!("required configuration still fails startup");
    };
    assert!(
        err.problems
            .iter()
            .any(|p| p.contains("LEDGERS_COMPANY_ID"))
    );
}

#[test]
fn the_status_read_reports_a_healthy_server_with_nothing_off() {
    let cfg = base(&[]);
    let (composition, native, _db) = compose_migrated(&cfg);
    let (status, body) = send(&composition, &native, Method::GET, "/v1/status");
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);
    assert_eq!(body["extraction"]["provider"], "rules");
    assert!(body["unavailable"].as_array().expect("a list").is_empty());
}
