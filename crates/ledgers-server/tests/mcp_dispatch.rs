//! MCP `tools/call` through the same dispatch as HTTP: one read and one write.

mod common;

use common::{COMPANY, fixture_with, json};
use cratefield_core::Module;
use cratefield_testing::request_as;
use ledgers_server::dispatch::ServerDispatch;
use ledgers_server::{McpModule, dispatch::registry};
use std::sync::Arc;

fn rpc(name: &str, arguments: &serde_json::Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    })
    .to_string()
}

/// The text of the one content block of a `tools/call` result, parsed.
fn tool_result(body: &serde_json::Value) -> (bool, serde_json::Value) {
    let result = &body["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or("null");
    (
        result["isError"].as_bool().unwrap_or(true),
        serde_json::from_str(text).unwrap_or(serde_json::Value::String(text.to_owned())),
    )
}

#[test]
fn mcp_tools_call_reads_and_writes_through_the_same_dispatch() {
    let mcp = McpModule::new(
        Arc::new(registry()),
        Arc::new(ServerDispatch::new(COMPANY)),
        "https://ledgers.example".to_owned(),
    );
    let fx = fixture_with(vec![Arc::new(mcp) as Arc<dyn Module>]);

    // Write: post a balanced entry as a tool call.
    let entry = common::entry_json("mcp-1", "5010", "4000", "250000");
    let write = pollster::block_on(request_as(
        fx.router(),
        cratefield_core::axum::http::Method::POST,
        "/v1/mcp",
        &fx.keys.full,
        Some(&rpc("entries_post", &entry)),
    ));
    assert_eq!(write.status.as_u16(), 200, "{}", write.json());
    let (is_error, posted) = tool_result(&json(&write));
    assert!(!is_error, "{posted}");
    assert_eq!(posted["entry"]["number"], 1);

    // Read: the trial balance of the same period, as a tool call.
    let read = pollster::block_on(request_as(
        fx.router(),
        cratefield_core::axum::http::Method::POST,
        "/v1/mcp",
        &fx.keys.read,
        Some(&rpc(
            "reports_trial_balance",
            &serde_json::json!({"period": "2026-10"}),
        )),
    ));
    let (is_error, report) = tool_result(&json(&read));
    assert!(!is_error, "{report}");
    assert_eq!(report["period"], "2026-10");
    assert_eq!(report["rows"].as_array().map(Vec::len), Some(2));
}

#[test]
fn mcp_tools_call_write_with_a_draft_key_is_refused() {
    let mcp = McpModule::new(
        Arc::new(registry()),
        Arc::new(ServerDispatch::new(COMPANY)),
        "https://ledgers.example".to_owned(),
    );
    let fx = fixture_with(vec![Arc::new(mcp) as Arc<dyn Module>]);
    let entry = common::entry_json("mcp-2", "5010", "4000", "100");
    let refused = pollster::block_on(request_as(
        fx.router(),
        cratefield_core::axum::http::Method::POST,
        "/v1/mcp",
        &fx.keys.draft,
        Some(&rpc("entries_post", &entry)),
    ));
    let (is_error, _) = tool_result(&json(&refused));
    assert!(is_error, "a draft key cannot post through MCP either");
}
