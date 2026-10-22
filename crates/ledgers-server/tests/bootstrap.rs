//! The deployed-environment bootstrap: one company, its chart and journal, and
//! exactly one full-tier key, refused once any key exists.

use cratefield_adapter_sqlite::SqliteDatabase;
use cratefield_core::{Config, Database, Statement};
use cratefield_runtime_native::{DirBlob, Native};
use ledgers_server::dispatch::{self, BootstrapError, CompanyBootstrap, Deps, registry};
use ledgers_server::harness;
use std::sync::Arc;

struct CompanyConfig;

impl Config for CompanyConfig {
    fn get(&self, key: &str) -> Option<String> {
        (key == "LEDGERS_COMPANY_ID").then(|| "01BOOTSTRAPCOMPANY0000000".to_owned())
    }
}

const COMPANY: &str = "01BOOTSTRAPCOMPANY0000000";

/// A migrated, empty database and the ports the bootstrap needs.
fn fresh() -> (Arc<SqliteDatabase>, Deps) {
    let db = Arc::new(SqliteDatabase::in_memory().expect("in-memory sqlite"));
    let native = Native::new()
        .db_arc(db.clone() as Arc<dyn Database>)
        .blob_arc(DirBlob::arc(concat!(
            env!("CARGO_TARGET_TMPDIR"),
            "/bootstrap-blobs"
        )));
    let composition = harness(&registry(), &CompanyConfig, native.clone()).expect("composition");
    for module in composition.modules() {
        db.apply_migrations(module.name(), module.migrations().sqlite)
            .expect("migrations apply");
    }
    let deps = Deps::from_ports(&native.ports()).expect("db, clock and id generator");
    (db, deps)
}

fn input(company: &str, currency: &str) -> CompanyBootstrap {
    CompanyBootstrap {
        company: company.to_owned(),
        currency: currency.to_owned(),
    }
}

fn count(db: &dyn Database, table: &str) -> i64 {
    let rows = pollster::block_on(db.query(&Statement::new(format!(
        "SELECT COUNT(*) AS n FROM {table}"
    ))))
    .expect("count query");
    rows.rows[0].get::<i64>("n").expect("count value")
}

#[test]
fn bootstrap_creates_the_company_chart_and_one_full_tier_key() {
    let (db, deps) = fresh();
    let key = pollster::block_on(dispatch::bootstrap_company(
        &deps,
        COMPANY,
        &input("Demo GmbH", "EUR"),
    ))
    .expect("first bootstrap runs");
    assert!(!key.is_empty(), "the key secret is returned");

    let rows = pollster::block_on(db.query(&Statement::new(
        "SELECT name, base_currency FROM company".to_owned(),
    )))
    .expect("company rows");
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(
        rows.rows[0].get::<String>("name").as_deref(),
        Some("Demo GmbH")
    );
    assert_eq!(
        rows.rows[0].get::<String>("base_currency").as_deref(),
        Some("EUR")
    );

    assert_eq!(count(db.as_ref(), "api_keys"), 1, "exactly one key");
    let policy =
        pollster::block_on(db.query(&Statement::new("SELECT policy FROM agent_key".to_owned())))
            .expect("agent key rows");
    assert_eq!(policy.rows.len(), 1);
    let policy_text = policy.rows[0].get::<String>("policy").expect("policy text");
    assert!(
        policy_text.contains("post-within-limits"),
        "the key is full tier: {policy_text}"
    );

    let journals = pollster::block_on(db.query(&Statement::new(
        "SELECT id FROM journal WHERE id = 'general'".to_owned(),
    )))
    .expect("journal rows");
    assert_eq!(journals.rows.len(), 1, "the general journal exists");
    assert!(
        count(db.as_ref(), "account") > 0,
        "the generic SME chart is imported"
    );
}

#[test]
fn a_second_bootstrap_is_refused_and_mints_nothing() {
    let (db, deps) = fresh();
    pollster::block_on(dispatch::bootstrap_company(
        &deps,
        COMPANY,
        &input("Demo GmbH", "EUR"),
    ))
    .expect("first bootstrap runs");
    let again = pollster::block_on(dispatch::bootstrap_company(
        &deps,
        COMPANY,
        &input("Other GmbH", "EUR"),
    ));
    assert_eq!(again, Err(BootstrapError::AlreadyBootstrapped));
    assert_eq!(count(db.as_ref(), "api_keys"), 1);
    assert_eq!(count(db.as_ref(), "company"), 1);
}

#[test]
fn two_racing_bootstraps_mint_one_key() {
    let (db, deps) = fresh();
    let request = input("Demo GmbH", "EUR");
    let first = dispatch::bootstrap_company(&deps, COMPANY, &request);
    let second = dispatch::bootstrap_company(&deps, COMPANY, &request);
    let (a, b) = pollster::block_on(futures_util::future::join(first, second));
    let results = [a, b];
    let minted = results.iter().filter(|r| r.is_ok()).count();
    let refused = results
        .iter()
        .filter(|r| matches!(r, Err(BootstrapError::AlreadyBootstrapped)))
        .count();
    assert_eq!(
        (minted, refused),
        (1, 1),
        "exactly one bootstrap wins: {results:?}"
    );
    assert_eq!(count(db.as_ref(), "agent_key"), 1);
    assert_eq!(count(db.as_ref(), "api_keys"), 1);
    assert_eq!(
        count(db.as_ref(), "ledger_guard"),
        0,
        "the claim's probe row is released"
    );
}

#[test]
fn a_development_key_also_closes_the_bootstrap() {
    let (_db, deps) = fresh();
    pollster::block_on(dispatch::bootstrap_dev(&deps, COMPANY)).expect("dev bootstrap");
    let result = pollster::block_on(dispatch::bootstrap_company(
        &deps,
        COMPANY,
        &input("Demo GmbH", "EUR"),
    ));
    assert_eq!(result, Err(BootstrapError::AlreadyBootstrapped));
}

#[test]
fn invalid_input_is_refused_before_anything_is_written() {
    let (db, deps) = fresh();
    let long_name = "x".repeat(dispatch::BOOTSTRAP_NAME_MAX + 1);
    for bad in [
        input("", "EUR"),
        input("   ", "EUR"),
        input(&long_name, "EUR"),
        input("Demo GmbH", "eur"),
        input("Demo GmbH", "EURO"),
        input("Demo GmbH", ""),
    ] {
        let result = pollster::block_on(dispatch::bootstrap_company(&deps, COMPANY, &bad));
        assert!(
            matches!(result, Err(BootstrapError::Invalid(_))),
            "{bad:?} must be refused, got {result:?}"
        );
    }
    assert_eq!(count(db.as_ref(), "company"), 0);
    assert_eq!(count(db.as_ref(), "api_keys"), 0);
}

#[test]
fn a_missing_company_id_is_a_failed_bootstrap() {
    let (db, deps) = fresh();
    let result = pollster::block_on(dispatch::bootstrap_company(
        &deps,
        "  ",
        &input("Demo GmbH", "EUR"),
    ));
    assert!(matches!(result, Err(BootstrapError::Failed(_))));
    assert_eq!(count(db.as_ref(), "api_keys"), 0);
}
