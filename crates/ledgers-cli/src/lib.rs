#![forbid(unsafe_code)]
//! Ledgers CLI: command-line interface for Ledgers accounting.
//!
//! This crate provides:
//! - **ledgers <resource> [<sub>] <verb>** — registry-driven subcommands
//! - **ledgers serve** — native server
//! - **ledgers completions <shell>** — shell completion generation
//!
//! # Command Execution
//!
//! Every command in the server's registry is available as a CLI subcommand:
//! - Input comes from `--input <JSON|@file|->` or `--<kebab-prop>` flags
//! - `--dry-run` is available when the command supports it
//! - `--json` outputs machine JSON; human mode outputs readable text
//!
//! # Profiles and Authentication
//!
//! Configuration file: `$LEDGERS_CONFIG_DIR/config.toml` or `~/.config/ledgers/config.toml`.
//!
//! ```toml
//! [profiles.default]
//! url = "http://localhost:3000"
//! key = "keychain"  # or "env:VAR_NAME" or a literal key (rejected)
//! ```
//!
//! Profile selection: `--profile`, then `LEDGERS_PROFILE`, then `default`.
//! Env overrides: `LEDGERS_URL`, `LEDGERS_KEY` (both are literals, not env references).
//!
//! # Exit Codes
//!
//! - `0`: Success
//! - `1`: Error (network, server 5xx, invalid input, etc.)
//! - `2`: Needs approval (command returned NeedsApproval; approval URL on stdout)
//! - `3`: Denied by policy (command returned Denied; error on stderr)

pub mod app;
pub mod error;
pub mod http;
pub mod mcp;
pub mod output;
pub mod profile;
pub mod serve;

pub use error::{CliError, ExitCode, Result};
pub use http::Session;

/// A reusable helper for calling a command and handling its outcome.
/// Used by CLI and will be reused by login/mcp in later issues.
pub async fn call(
    session: &http::Session,
    descriptor: &ledgers_commands::CommandDescriptor,
    id: Option<&str>,
    input: &serde_json::Value,
    dry_run: bool,
) -> Result<CallOutcome> {
    let url = session.url_for_command(descriptor, id)?;
    let body = input.clone();

    // Build query params for dry-run
    let params = if dry_run && descriptor.supports_dry_run {
        vec![("dry_run", "true")]
    } else {
        vec![]
    };

    // Make the request
    let response = session
        .send_request(&url, &descriptor.http_method, &params, &body)
        .await?;

    Ok(response)
}

/// The outcome of calling a command: Done, DryRun, NeedsApproval, or Denied.
/// Extension point for login and mcp subcommands.
#[derive(Clone, Debug)]
pub enum CallOutcome {
    /// Command executed successfully.
    Done(serde_json::Value),
    /// Dry-run preview.
    DryRun(serde_json::Value),
    /// Policy approval needed: approval ID and URL.
    NeedsApproval {
        approval_id: String,
        url: String,
        reason: String,
        preview: serde_json::Value,
    },
    /// Policy denied.
    Denied(String),
}
