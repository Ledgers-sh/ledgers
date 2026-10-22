//! Development mode is the only thing that enables the local web origin and
//! the fallback company. Production composition requires its own company.

use cratefield_adapter_sqlite::SqliteDatabase;
use cratefield_core::Config;
use cratefield_runtime_native::{DirBlob, Native};
use ledgers_server::{DEV_WEB_ORIGIN, dispatch::registry, harness};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A native runtime with an in-memory database and a blob directory: the ports
/// the composition requires.
fn runtime() -> Native {
    Native::new()
        .db_arc(Arc::new(
            SqliteDatabase::in_memory().expect("in-memory sqlite"),
        ))
        .blob_arc(DirBlob::arc(concat!(env!("CARGO_TARGET_TMPDIR"), "/blobs")))
}

struct MapConfig(BTreeMap<&'static str, &'static str>);

impl Config for MapConfig {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).map(|v| (*v).to_owned())
    }
}

#[test]
fn dev_mode_allows_the_local_web_origin() {
    let cfg = MapConfig(BTreeMap::from([("LEDGERS_DEV", "1")]));
    let built = harness(&registry(), &cfg, runtime()).expect("dev composes without a company");
    assert!(
        built
            .venture()
            .cors_origins
            .iter()
            .any(|o| o == DEV_WEB_ORIGIN)
    );
}

#[test]
fn outside_dev_mode_the_local_web_origin_is_not_allowed() {
    let cfg = MapConfig(BTreeMap::from([
        ("LEDGERS_COMPANY_ID", "01PRODCOMPANY000000000000A"),
        (
            "LEDGERS_DOCUMENT_SIGNING_KEY",
            "prod-test-document-signing-key-0123456789",
        ),
    ]));
    let built = harness(&registry(), &cfg, runtime()).expect("production composes");
    assert!(
        !built
            .venture()
            .cors_origins
            .iter()
            .any(|o| o == DEV_WEB_ORIGIN)
    );
}

#[test]
fn outside_dev_mode_a_company_is_required() {
    let cfg = MapConfig(BTreeMap::new());
    assert!(harness(&registry(), &cfg, runtime()).is_err());
}
