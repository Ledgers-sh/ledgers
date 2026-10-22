//! MCP HTTP endpoint acceptance tests: auth, origin checks, tool filtering, and dispatch.

use cratefield_core::Problem;
use cratefield_core::axum;
use cratefield_core::axum::body::Body;
use cratefield_core::axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request};
use ledgers_commands::{CommandDescriptor, Limits, Principal, Registry, Tier};
use ledgers_server::routes::Presented;
use ledgers_server::{Deps, Dispatch, McpModule};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

fn test_registry() -> Arc<Registry> {
    let mut registry = Registry::new();
    registry
        .register(CommandDescriptor {
            name: "approvals.status".to_string(),
            tier: "read".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: true,
            http_method: "GET".to_string(),
            http_path: "/v1/approvals/status".to_string(),
            cli: "ledgers approvals status".to_string(),
            mcp: Some("approvals_status".to_string()),
            curl: "curl https://ledgers.example/v1/approvals/status".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        })
        .unwrap();
    registry
        .register(CommandDescriptor {
            name: "bills.match".to_string(),
            tier: "draft".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
            http_method: "POST".to_string(),
            http_path: "/v1/bills/match".to_string(),
            cli: "ledgers bills match".to_string(),
            mcp: Some("bills_match".to_string()),
            curl: "curl -X POST https://ledgers.example/v1/bills/match".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        })
        .unwrap();
    Arc::new(registry)
}

struct TestDispatch;

#[async_trait::async_trait]
impl Dispatch for TestDispatch {
    async fn dispatch(
        &self,
        _: &Deps,
        command: &str,
        _: Value,
        _: &str,
        _: Option<Presented>,
    ) -> Result<Value, Problem> {
        if command == "bills.match" {
            return Ok(
                json!({"status": "needs-approval", "approval": {"approval_id": "apr_01", "url": "https://ledgers.example/approvals/apr_01", "reason": "above the agent limit", "preview": {"lines": [{"account": "1000", "side": "debit", "base": "1200.00"}], "totals": {"EUR": "1200.00"}}, "counterparty": "Acme GmbH", "period": "2026-10"}}),
            );
        }
        Ok(json!({"result": "ok"}))
    }

    async fn principal(&self, _: &Deps, credential: &str) -> Result<Principal, Problem> {
        match credential {
            "lk_test_read" => Ok(Principal::Agent {
                key_id: "test_read_key".to_string(),
                tier: Tier::Read,
                limits: Limits::new(),
            }),
            "lk_test_post" => Ok(Principal::Agent {
                key_id: "test_post_key".to_string(),
                tier: Tier::PostWithinLimits,
                limits: Limits::new(),
            }),
            "sess_person" => Ok(Principal::Person {
                person_id: "user_123".to_string(),
            }),
            _ => Err(Problem::validation_failed("unauthorized")),
        }
    }
}

fn send(
    router: &axum::Router,
    method: &str,
    path: &str,
    key: Option<&str>,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, HeaderMap, String) {
    let mut req_headers = HeaderMap::new();
    if let Some(token) = key {
        req_headers.insert(
            HeaderName::from_static("authorization"),
            format!("Bearer {token}").parse().unwrap(),
        );
    }
    for (name, value) in headers {
        req_headers.insert(
            HeaderName::try_from(*name).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    for (name, value) in &req_headers {
        if let Ok(v) = value.to_str() {
            builder = builder.header(name, v);
        }
    }
    let request = builder.body(Body::from(body.to_string())).expect("build");
    let response = pollster::block_on(router.clone().oneshot(request)).expect("dispatch");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body_bytes = pollster::block_on(cratefield_core::axum::body::to_bytes(
        response.into_body(),
        usize::MAX,
    ))
    .expect("read");
    (
        status,
        headers,
        String::from_utf8_lossy(&body_bytes).to_string(),
    )
}

fn harness() -> cratefield_testing::TestHarness {
    let mcp = McpModule::new(
        test_registry(),
        Arc::new(TestDispatch),
        "https://ledgers.example".to_string(),
    );
    cratefield_testing::TestHarness::new(vec![Box::new(mcp)])
}

#[test]
fn test_1_auth_and_origin() {
    let h = harness();
    let cases = vec![
        (None, None, "{}", 401),
        (
            Some("lk_unknown_key"),
            None,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            401,
        ),
        (None, None, "garbage", 401),
        (Some("lk_test_read"), None, "garbage", 400),
        (
            Some("lk_test_read"),
            Some("https://evil.example"),
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            403,
        ),
        (
            Some("lk_test_read"),
            Some("https://ledgers.example"),
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            200,
        ),
    ];
    for (key, origin, body, expected_status) in cases {
        let hdrs = origin.map(|o| vec![("origin", o)]).unwrap_or_default();
        let (s, resp_hdrs, b) = send(&h.router, "POST", "/v1/mcp", key, &hdrs, body);
        assert_eq!(s, expected_status);
        if s == 400 {
            assert_eq!(
                serde_json::from_str::<Value>(&b)
                    .unwrap()
                    .get("error")
                    .unwrap()
                    .get("code")
                    .unwrap()
                    .as_i64(),
                Some(-32700)
            );
        }
        if s == 401 || s == 403 {
            assert!(resp_hdrs.get("www-authenticate").is_some());
        }
    }
}

#[test]
fn test_2_well_known() {
    let h = harness();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/.well-known/oauth-protected-resource")
        .body(Body::empty())
        .unwrap();
    let response = pollster::block_on(h.router.clone().oneshot(request)).unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body = pollster::block_on(cratefield_core::axum::body::to_bytes(
        response.into_body(),
        usize::MAX,
    ))
    .unwrap();
    let json: Value = serde_json::from_str(&String::from_utf8_lossy(&body)).unwrap();
    assert_eq!(
        json.get("resource").and_then(Value::as_str),
        Some("https://ledgers.example/v1/mcp")
    );
}

#[test]
fn test_3_tools_list_matrix() {
    let h = harness();
    let cases = vec![
        ("lk_test_read", "", &[][..], vec!["approvals_status"]),
        (
            "lk_test_post",
            "",
            &[],
            vec!["approvals_status", "bills_match"],
        ),
        (
            "lk_test_post",
            "read_only=true",
            &[],
            vec!["approvals_status"],
        ),
        (
            "lk_test_post",
            "",
            &[("ledgers-read-only", "true")],
            vec!["approvals_status"],
        ),
        ("sess_person", "", &[], vec!["approvals_status"]),
    ];
    for (key, query, hdrs, expected) in cases {
        let uri = if query.is_empty() {
            "/v1/mcp".to_string()
        } else {
            format!("/v1/mcp?{query}")
        };
        let (s, _, b) = send(
            &h.router,
            "POST",
            &uri,
            Some(key),
            hdrs,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        assert_eq!(s, 200);
        let json: Value = serde_json::from_str(&b).unwrap();
        let tools: Vec<&str> = json
            .get("result")
            .unwrap()
            .get("tools")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(tools, expected);
    }
}

#[test]
fn test_4_tools_call_approval_card() {
    let h = harness();
    let (s, _, b) = send(
        &h.router,
        "POST",
        "/v1/mcp",
        Some("lk_test_post"),
        &[],
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"bills_match","arguments":{}}}"#,
    );
    assert_eq!(s, 200);
    let json: Value = serde_json::from_str(&b).unwrap();
    let result = json.get("result").unwrap();
    assert_eq!(result.get("isError").and_then(Value::as_bool), Some(false));
    let approval = result
        .get("structuredContent")
        .unwrap()
        .get("approval")
        .unwrap();
    assert_eq!(
        approval.get("approval_id").and_then(Value::as_str),
        Some("apr_01")
    );
    assert_eq!(
        approval.get("link").and_then(Value::as_str),
        Some("https://ledgers.example/approvals/apr_01")
    );
    let text = result
        .get("content")
        .unwrap()
        .as_array()
        .unwrap()
        .first()
        .unwrap()
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        text.contains("1200.00")
            && text.contains("https://ledgers.example/approvals/apr_01")
            && text.contains("Acme GmbH")
    );
}

#[test]
fn test_5_read_key_cannot_call_draft() {
    let h = harness();
    let (s, _, b) = send(
        &h.router,
        "POST",
        "/v1/mcp",
        Some("lk_test_read"),
        &[],
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"bills_match","arguments":{}}}"#,
    );
    assert_eq!(s, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&b)
            .unwrap()
            .get("error")
            .unwrap()
            .get("code")
            .and_then(Value::as_i64),
        Some(-32602)
    );
}

#[test]
fn test_6_notification_202() {
    let h = harness();
    let (s, _, b) = send(
        &h.router,
        "POST",
        "/v1/mcp",
        Some("lk_test_read"),
        &[],
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert_eq!(s, 202);
    assert!(b.is_empty());
}

#[test]
fn test_7_get_405() {
    let h = harness();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/mcp")
        .body(Body::empty())
        .unwrap();
    let response = pollster::block_on(h.router.clone().oneshot(request)).unwrap();
    assert_eq!(response.status().as_u16(), 405);
}
