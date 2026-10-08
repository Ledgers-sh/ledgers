# Versioning and compatibility

**Draft, pending review (Ledgers-sh/ledgers#54).**

Books must stay readable for the legal retention period (10 years in Germany),
so Ledgers versions what other people and other software depend on.

| Surface | Promise |
|---|---|
| Database schema | Forward-only migrations, run on start under a lock. No migration deletes or rewrites posted entries, audit events or documents. A downgrade is not supported; restore a backup instead. |
| Export format | Versioned (`format_version` in the manifest). Every release can import every earlier format version. Breaking changes bump the major version and ship a converter. |
| Audit log format | The canonical JSON form and hash algorithm are fixed per `audit_version`; verifiers keep checking old versions forever. |
| HTTP API (`/v1`) | Additive changes only within `/v1`. Removals or breaking changes go to `/v2`, with `/v1` served for at least 12 months after `/v2` ships. |
| MCP tools and CLI | Tool and command names are stable once released; a rename keeps the old name as an alias for at least one minor release with a deprecation notice. |
| Rust crates | Semantic versioning; before 1.0, a minor bump may break the Rust API but never the export, audit or `/v1` contracts above. |
