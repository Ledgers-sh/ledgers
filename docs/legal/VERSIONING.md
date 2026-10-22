# Versioning and compatibility

**Draft, pending review (Ledgers-sh/ledgers#54).**

Books must stay readable for the legal retention period (10 years in Germany),
so Ledgers versions what other people and other software depend on.

| Surface | Promise |
|---|---|
| Database schema | Forward-only migrations, numbered (e.g. `0001_accounts`, `0002_journal`), run on startup under a lock per crate. No migration deletes, rewrites or UPDATEs posted entries, audit events, or documents once committed. Migrations exist in both SQLite and Postgres dialects; CI tests they produce identical schemas. A downgrade is not supported; restore a backup instead. (crates/ledgers-{ledger,db,approvals,policy,tax,fx}/migrations/) |
| Export format | Planned (issue #30, M3). Versioned; manifest will include `format_version`. Every release will be able to import every earlier format. Breaking changes bump major version and ship a converter. Not wired in yet. |
| Audit log format | The canonical JSON form and hash algorithm are fixed per `audit_version` in each event (docs/audit-format.md). Verifiers keep checking old versions forever. Genesis hash is `"0"` (ledgers_db::chain::GENESIS_HASH); SHA-256 chains are unambiguous. |
| HTTP API (`/v1`) | Additive changes only within `/v1`. Removals or breaking changes go to `/v2`, with `/v1` served for at least 12 months after `/v2` ships. Routes are auto-generated from the command registry (crates/ledgers-commands/src/registry.rs); tool names match command names. |
| MCP tools and CLI | Tools and commands are stable once released (crates/ledgers-commands/src/registry.rs). Protocol version is hardcoded in initialize response (`crates/ledgers-commands/src/mcp.rs:284`). Input schemas change additively; tool names are stable aliases. |
| Rust crates | Semantic versioning; before 1.0, a minor bump may break the Rust API but never the export, audit or HTTP `/v1` contracts above. |
