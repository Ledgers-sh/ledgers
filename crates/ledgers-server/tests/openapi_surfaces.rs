//! The `OpenAPI` document of the real command registry, as the app's command
//! palette reads it (issue #53).
//!
//! Every operation carries its three surface forms (`x-ledgers-cli`,
//! `x-ledgers-mcp`, the HTTP call), so the palette shows what the server
//! registers rather than a list written by hand. The document is also
//! committed as `app/tests/fixtures/openapi.json`, which the app's tests and
//! Playwright mock read. Regenerate it after a command changes with
//! `LEDGERS_WRITE_OPENAPI_FIXTURE=1 cargo test -p ledgers-server --test openapi_surfaces`.

use ledgers_server::{dispatch::registry, openapi};
use serde_json::Value;
use std::fs;
use std::path::Path;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../app/tests/fixtures/openapi.json"
);

fn document() -> Value {
    openapi::document(&registry())
}

#[test]
fn every_operation_has_cli_mcp_and_http_forms() {
    let doc = document();
    let paths = doc["paths"].as_object().expect("paths object");
    let mut operations = 0;
    for (path, item) in paths {
        for (method, op) in item.as_object().expect("path item") {
            operations += 1;
            let name = op["x-ledgers-command"]
                .as_str()
                .unwrap_or_else(|| panic!("{method} {path}: no x-ledgers-command"));
            assert_eq!(
                op["operationId"],
                name.replace('.', "_"),
                "operationId follows the command name"
            );

            let cli = op["x-ledgers-cli"]
                .as_str()
                .unwrap_or_else(|| panic!("{name}: no x-ledgers-cli"));
            assert!(cli.starts_with("ledgers "), "{name}: CLI form is {cli:?}");

            let mcp = &op["x-ledgers-mcp"];
            if op["x-ledgers-human-only"] == Value::Bool(true) {
                assert!(
                    mcp.is_null(),
                    "{name}: human-only commands have no MCP tool"
                );
            } else {
                assert_eq!(
                    mcp.as_str(),
                    Some(name.replace('.', "_").as_str()),
                    "{name}: MCP tool name"
                );
            }

            let curl = op["x-ledgers-curl"]
                .as_str()
                .unwrap_or_else(|| panic!("{name}: no x-ledgers-curl"));
            assert!(
                curl.starts_with(&format!("curl -X {}", method.to_uppercase())),
                "{name}: curl example uses the HTTP method"
            );
            assert!(
                curl.contains(path.as_str()),
                "{name}: curl example names the path"
            );
        }
    }
    assert!(operations > 0, "the registry has commands");
}

#[test]
fn committed_fixture_matches_the_registry() {
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&document()).expect("document serialises")
    );
    if std::env::var("LEDGERS_WRITE_OPENAPI_FIXTURE").as_deref() == Ok("1") {
        let path = Path::new(FIXTURE);
        fs::create_dir_all(path.parent().expect("fixture dir")).expect("create fixtures dir");
        fs::write(path, &rendered).expect("write fixture");
        return;
    }
    let committed = fs::read_to_string(FIXTURE).unwrap_or_default();
    assert!(
        committed == rendered,
        "app/tests/fixtures/openapi.json differs from the registry; regenerate with LEDGERS_WRITE_OPENAPI_FIXTURE=1"
    );
}
