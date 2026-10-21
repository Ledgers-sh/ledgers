#![forbid(unsafe_code)]
//! HTTP surface tests for the Ledgers server.

use cratefield_core::{Module, axum::http::Method};
use cratefield_testing::{TestHarness, request};
use ledgers_commands::Registry;
use ledgers_server::openapi;

#[test]
fn test_openapi_doc_exists() {
    let registry = Registry::new();
    let openapi_module = openapi::OpenApiModule::new(&registry);
    let modules: Vec<Box<dyn Module>> = vec![Box::new(openapi_module)];
    let kit = TestHarness::new(modules);

    let response = pollster::block_on(request(&kit.router, Method::GET, "/v1/openapi", None));

    assert_eq!(
        response.status.as_u16(),
        200,
        "expected 200 OK for OpenAPI doc"
    );
}

#[test]
fn test_openapi_version() {
    let registry = Registry::new();
    let openapi_module = openapi::OpenApiModule::new(&registry);
    let modules: Vec<Box<dyn Module>> = vec![Box::new(openapi_module)];
    let kit = TestHarness::new(modules);

    let response = pollster::block_on(request(&kit.router, Method::GET, "/v1/openapi", None));

    assert_eq!(response.status.as_u16(), 200);

    let body = String::from_utf8(response.body().to_vec()).expect("invalid UTF-8");
    let json: serde_json::Value =
        serde_json::from_str(&body).expect("failed to parse JSON response");

    assert_eq!(
        json.get("openapi").and_then(|v| v.as_str()),
        Some("3.1.0"),
        "expected OpenAPI version 3.1.0"
    );
}

#[test]
fn test_openapi_title() {
    let registry = Registry::new();
    let openapi_module = openapi::OpenApiModule::new(&registry);
    let modules: Vec<Box<dyn Module>> = vec![Box::new(openapi_module)];
    let kit = TestHarness::new(modules);

    let response = pollster::block_on(request(&kit.router, Method::GET, "/v1/openapi", None));

    assert_eq!(response.status.as_u16(), 200);

    let body = String::from_utf8(response.body().to_vec()).expect("invalid UTF-8");
    let json: serde_json::Value =
        serde_json::from_str(&body).expect("failed to parse JSON response");

    assert_eq!(
        json.get("info")
            .and_then(|v| v.get("title"))
            .and_then(|v| v.as_str()),
        Some("Ledgers"),
        "expected title 'Ledgers'"
    );
}

#[test]
fn test_openapi_has_paths() {
    let registry = Registry::new();
    let openapi_module = openapi::OpenApiModule::new(&registry);
    let modules: Vec<Box<dyn Module>> = vec![Box::new(openapi_module)];
    let kit = TestHarness::new(modules);

    let response = pollster::block_on(request(&kit.router, Method::GET, "/v1/openapi", None));

    assert_eq!(response.status.as_u16(), 200);

    let body = String::from_utf8(response.body().to_vec()).expect("invalid UTF-8");
    let json: serde_json::Value =
        serde_json::from_str(&body).expect("failed to parse JSON response");

    // Even with an empty registry, the doc should have a paths section
    assert!(
        json.get("paths").is_some(),
        "expected 'paths' section in OpenAPI doc"
    );
}

#[test]
fn test_openapi_refs_resolved() {
    let registry = Registry::new();
    let openapi_module = openapi::OpenApiModule::new(&registry);
    let modules: Vec<Box<dyn Module>> = vec![Box::new(openapi_module)];
    let kit = TestHarness::new(modules);

    let response = pollster::block_on(request(&kit.router, Method::GET, "/v1/openapi", None));

    assert_eq!(response.status.as_u16(), 200);

    let body = String::from_utf8(response.body().to_vec()).expect("invalid UTF-8");
    let json: serde_json::Value =
        serde_json::from_str(&body).expect("failed to parse JSON response");

    // Check that all $refs resolve
    check_refs_resolved(&json, &json);
}

#[test]
fn test_openapi_has_components() {
    let registry = Registry::new();
    let openapi_module = openapi::OpenApiModule::new(&registry);
    let modules: Vec<Box<dyn Module>> = vec![Box::new(openapi_module)];
    let kit = TestHarness::new(modules);

    let response = pollster::block_on(request(&kit.router, Method::GET, "/v1/openapi", None));

    assert_eq!(response.status.as_u16(), 200);

    let body = String::from_utf8(response.body().to_vec()).expect("invalid UTF-8");
    let json: serde_json::Value =
        serde_json::from_str(&body).expect("failed to parse JSON response");

    // OpenAPI docs should have components section
    assert!(
        json.get("components").is_some(),
        "expected 'components' section in OpenAPI doc"
    );
}

#[test]
fn test_openapi_doc_is_valid_json() {
    let registry = Registry::new();
    let openapi_module = openapi::OpenApiModule::new(&registry);
    let modules: Vec<Box<dyn Module>> = vec![Box::new(openapi_module)];
    let kit = TestHarness::new(modules);

    let response = pollster::block_on(request(&kit.router, Method::GET, "/v1/openapi", None));

    assert_eq!(response.status.as_u16(), 200);
    let body = String::from_utf8(response.body().to_vec()).expect("invalid UTF-8");

    // If we got here, the body was successfully parsed as JSON
    let json: serde_json::Value =
        serde_json::from_str(&body).expect("failed to parse JSON response");
    assert!(json.is_object(), "expected root to be an object");
}

#[test]
fn test_health_probe_exists() {
    // Note: The health and ready probes are managed by the harness, not individual modules
    // So we test them through the root router
    let modules: Vec<Box<dyn Module>> = vec![];
    let kit = TestHarness::new(modules);

    let response = pollster::block_on(request(&kit.router, Method::GET, "/__health", None));

    // TestHarness includes the harness-level routes
    assert_eq!(
        response.status.as_u16(),
        200,
        "expected 200 OK for health probe"
    );
}

#[test]
fn test_ready_probe_exists() {
    let modules: Vec<Box<dyn Module>> = vec![];
    let kit = TestHarness::new(modules);

    let response = pollster::block_on(request(&kit.router, Method::GET, "/__ready", None));

    assert_eq!(
        response.status.as_u16(),
        200,
        "expected 200 OK for readiness probe"
    );
}

/// Helper function to recursively check that all $refs in a value resolve to existing components.
fn check_refs_resolved(value: &serde_json::Value, root: &serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                if key == "$ref" {
                    if let Some(ref_str) = val.as_str() {
                        if ref_str.starts_with("#/components/schemas/") {
                            let schema_name = &ref_str[21..]; // Skip "#/components/schemas/"
                            let exists = root
                                .get("components")
                                .and_then(|c| c.get("schemas"))
                                .and_then(|s| s.get(schema_name))
                                .is_some();
                            assert!(exists, "ref {} does not resolve to a component", ref_str);
                        }
                    }
                } else {
                    check_refs_resolved(val, root);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                check_refs_resolved(item, root);
            }
        }
        _ => {}
    }
}
