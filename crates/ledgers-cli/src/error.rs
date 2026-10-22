//! CLI errors and exit codes.
//!
//! Exit codes are a contract for agents and scripts:
//! `0` ok, `1` any error, `2` needs approval (the approval URL is printed),
//! `3` denied by policy (HTTP 403).

use serde_json::{Value, json};
use std::fmt;

/// Process exit codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitCode {
    /// The command completed (or a dry run previewed).
    Ok = 0,
    /// Any other error: network, server error, invalid input, auth, config.
    Error = 1,
    /// Policy says the command needs approval; the approval URL is printed.
    NeedsApproval = 2,
    /// Policy denied the command (HTTP 403).
    PolicyDenied = 3,
}

impl ExitCode {
    /// The numeric process exit status.
    #[must_use]
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// Errors the CLI can produce before or instead of a command outcome.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The server could not be reached.
    #[error("network error: {0}")]
    Network(String),
    /// A non-JSON HTTP error response.
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    /// An RFC 9457 problem response other than a 403 denial.
    #[error("{slug}: {detail}")]
    Problem {
        slug: String,
        detail: String,
        status: u16,
    },
    /// Bad command input (JSON, schema fields, missing path ids).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Unknown profile, missing URL or missing key.
    #[error("profile error: {0}")]
    Profile(String),
    /// The OS keychain could not provide a key.
    #[error("keychain error: {0}")]
    Keychain(String),
    /// The config file is unreadable or invalid.
    #[error("config error: {0}")]
    Config(String),
    /// A response body that is not the expected JSON.
    #[error("JSON error: {0}")]
    Json(String),
    /// `ledgers serve` could not start.
    #[error("server error: {0}")]
    Serve(String),
    /// Anything else.
    #[error("internal error: {0}")]
    Internal(String),
}

impl CliError {
    /// Maps this error to an exit code. Every error is `1`; a 403 is
    /// reported as a policy denial by the HTTP layer, not as an error.
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        ExitCode::Error
    }

    /// The error as a JSON object for `--json` output.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            CliError::Problem {
                slug,
                detail,
                status,
            } => json!({"slug": slug, "message": detail, "status": status}),
            CliError::Http { status, body } => {
                json!({"message": self.to_string(), "status": status, "body": body})
            }
            other => json!({"message": other.to_string()}),
        }
    }
}

/// Result type for CLI operations.
pub type Result<T> = std::result::Result<T, CliError>;

impl From<fmt::Error> for CliError {
    fn from(e: fmt::Error) -> Self {
        CliError::Internal(e.to_string())
    }
}
