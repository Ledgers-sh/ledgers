//! MCP tests: tool listing by tier, JSON-RPC protocol, approval cards.

use ledgers_commands::mcp::{
    ApprovalAmount, ApprovalCard, ApprovalCardLine, ToolCaller, ToolOutcome, ToolScope,
    UNTRUSTED_DATA_RULE, handle, tools,
};
use ledgers_commands::{CommandDescriptor, Registry, Tier};
use serde_json::{Value, json};

#[test]
fn test_tier_from_str() {
    assert_eq!("read".parse::<Tier>().unwrap(), Tier::Read);
    assert_eq!(
        "post-within-limits".parse::<Tier>().unwrap(),
        Tier::PostWithinLimits
    );
    assert!("unknown".parse::<Tier>().is_err());
}

#[test]
fn test_tools_tier_filtering() {
    let registry = Registry::new();
    let read_tools = tools(
        &registry,
        ToolScope {
            tier: Tier::Read,
            read_only: false,
        },
    );
    let draft_tools = tools(
        &registry,
        ToolScope {
            tier: Tier::Draft,
            read_only: false,
        },
    );
    assert!(draft_tools.len() >= read_tools.len());
}

#[test]
fn test_approval_card() {
    let card = ApprovalCard {
        approval_id: "req-1".to_string(),
        link: "http://example.com/1".to_string(),
        reason: "Over limit".to_string(),
        amounts: vec![ApprovalAmount {
            currency: "EUR".to_string(),
            amount: "1000.00".to_string(),
        }],
        counterparty: Some("Acme Inc".to_string()),
        period: Some("2025-01".to_string()),
        lines: vec![ApprovalCardLine {
            account: "1000".to_string(),
            side: "debit".to_string(),
            amount: "1000.00".to_string(),
        }],
    };
    let text = card.text();
    assert!(text.contains("Over limit") && text.contains("Acme Inc") && text.contains("EUR"));
}

fn fixture_registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .register(CommandDescriptor {
            name: "test.read_cmd".to_string(),
            tier: "read".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: true,
            http_method: "GET".to_string(),
            http_path: "/test".to_string(),
            cli: "test read".to_string(),
            mcp: Some("test_read_cmd".to_string()),
            curl: "curl test".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        })
        .unwrap();
    registry
        .register(CommandDescriptor {
            name: "test.draft_cmd".to_string(),
            tier: "draft".to_string(),
            human_only: false,
            supports_dry_run: true,
            idempotent: false,
            http_method: "POST".to_string(),
            http_path: "/test".to_string(),
            cli: "test draft".to_string(),
            mcp: Some("test_draft_cmd".to_string()),
            curl: "curl test".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        })
        .unwrap();
    registry
        .register(CommandDescriptor {
            name: "test.human_only".to_string(),
            tier: "read".to_string(),
            human_only: true,
            supports_dry_run: false,
            idempotent: true,
            http_method: "GET".to_string(),
            http_path: "/test".to_string(),
            cli: "test human".to_string(),
            mcp: Some("test_human_only".to_string()),
            curl: "curl test".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        })
        .unwrap();
    registry
}

#[test]
fn test_jsonrpc_methods() {
    pollster::block_on(async {
        let registry = Registry::new();
        let scope = ToolScope {
            tier: Tier::PostWithinLimits,
            read_only: false,
        };

        let init = handle(
            &registry,
            scope,
            &Mock,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        )
        .await
        .expect("init");
        assert_eq!(init.get("id").and_then(Value::as_i64), Some(1));
        assert!(init.get("result").is_some());

        let ping = handle(
            &registry,
            scope,
            &Mock,
            json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
        )
        .await
        .expect("ping");
        assert_eq!(ping.get("id").and_then(Value::as_i64), Some(2));

        let list = handle(
            &registry,
            scope,
            &Mock,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
        )
        .await
        .expect("list");
        assert!(list.get("result").and_then(|r| r.get("tools")).is_some());

        let unknown = handle(
            &registry,
            scope,
            &Mock,
            json!({"jsonrpc": "2.0", "id": 4, "method": "unknown"}),
        )
        .await
        .expect("unknown");
        assert_eq!(
            unknown
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Value::as_i64),
            Some(-32601)
        );

        let notif = handle(
            &registry,
            scope,
            &Mock,
            json!({"jsonrpc": "2.0", "method": "ping"}),
        )
        .await;
        assert!(notif.is_none());
    });
}

#[test]
fn test_tools_no_human_only_and_have_untrusted_rule() {
    let registry = fixture_registry();
    for scope in [
        ToolScope {
            tier: Tier::Read,
            read_only: false,
        },
        ToolScope {
            tier: Tier::Draft,
            read_only: false,
        },
        ToolScope {
            tier: Tier::PostWithinLimits,
            read_only: false,
        },
    ] {
        let tool_list = tools(&registry, scope);
        for tool in &tool_list {
            let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
            assert!(!name.contains("human_only"), "human-only should not appear");
            let desc = tool
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            assert!(
                desc.contains(UNTRUSTED_DATA_RULE),
                "desc should have UNTRUSTED_DATA_RULE"
            );
        }
    }
}

#[test]
fn test_tools_tier_and_read_only_filtering() {
    let registry = fixture_registry();

    let read_scope = ToolScope {
        tier: Tier::Read,
        read_only: false,
    };
    let read_tools = tools(&registry, read_scope);
    let read_names: Vec<&str> = read_tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();
    assert_eq!(read_names, vec!["test_read_cmd"]);

    let ro_post = ToolScope {
        tier: Tier::PostWithinLimits,
        read_only: true,
    };
    let ro_tools = tools(&registry, ro_post);
    let ro_names: Vec<&str> = ro_tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();
    assert_eq!(ro_names, vec!["test_read_cmd"]);
}

struct Mock;

#[async_trait::async_trait]
impl ToolCaller for Mock {
    async fn call(&self, _d: &CommandDescriptor, _i: Value, _dr: bool) -> ToolOutcome {
        ToolOutcome::Done(json!({}))
    }
}
