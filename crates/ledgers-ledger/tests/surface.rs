//! Golden snapshot test for the command surface registry.
//!
//! This test ensures that any change to a command's surface (name, tier, schemas,
//! HTTP method/path, CLI form, MCP tool name, curl example, input/output schemas)
//! is visible in code review.
//!
//! To update the snapshot after an intentional change:
//! - Run `INSTA_UPDATE=always cargo test -p ledgers-ledger --test surface`
//! - Review the `.snap.new` file before accepting with `cargo insta review`
//! - Or simply `INSTA_UPDATE=always cargo test -p ledgers-ledger --test surface --`

#[test]
fn golden_surface_registry() {
    let registry = ledgers_ledger::surface::registry();
    insta::assert_json_snapshot!("surface", registry.to_json());
}

#[test]
fn golden_mcp_tools() {
    use ledgers_commands::Tier;
    use ledgers_commands::mcp::{ToolScope, tools};

    let registry = ledgers_ledger::surface::registry();
    let scope = ToolScope {
        tier: Tier::PostWithinLimits,
        read_only: false,
    };
    let tool_list = tools(&registry, scope);
    insta::assert_json_snapshot!("mcp_tools", tool_list);
}
