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

    // Snapshot the full registry as JSON, which includes all descriptors
    // with every field: name, tier, human_only, supports_dry_run, idempotent,
    // http_method, http_path, cli, mcp, curl, input_schema, output_schema.
    insta::assert_json_snapshot!("surface", registry.to_json());
}
