//! CLI error types and exit codes.

use std::fmt;

/// CLI exit codes as defined in the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitCode {
    /// Success
    Ok = 0,
    /// Error (network, server 5xx, invalid input, auth, etc.)
    Error = 1,
    /// Needs approval
    NeedsApproval = 2,
    /// Denied by policy
    PolicyDenied = 3,
}

/// Errors the CLI can produce.
#[derive(Debug)]
pub enum CliError {
    /// Network error.
    Network(String),
    /// HTTP error (status, body).
    Http { status: u16, body: String },
    /// Server returned a problem response (RFC 9457).
    Problem {
        slug: String,
        detail: String,
        status: Option<u16>,
    },
    /// Invalid input or validation.
    InvalidInput(String),
    /// Missing or invalid profile.
    Profile(String),
    /// Keychain error.
    Keychain(String),
    /// Config file error.
    Config(String),
    /// Invalid command argument.
    CommandArg(String),
    /// Server startup error.
    Serve(String),
    /// JSON parsing error.
    Json(String),
    /// Internal CLI error.
    Internal(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Network(msg) => write!(f, "network error: {msg}"),
            CliError::Http { status, body } => {
                write!(f, "HTTP {status}: {body}")
            }
            CliError::Problem { slug, detail, .. } => {
                write!(f, "{slug}: {detail}")
            }
            CliError::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            CliError::Profile(msg) => write!(f, "profile error: {msg}"),
            CliError::Keychain(msg) => write!(f, "keychain error: {msg}"),
            CliError::Config(msg) => write!(f, "config error: {msg}"),
            CliError::CommandArg(msg) => write!(f, "argument error: {msg}"),
            CliError::Serve(msg) => write!(f, "server error: {msg}"),
            CliError::Json(msg) => write!(f, "JSON error: {msg}"),
            CliError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for CliError {}

impl CliError {
    /// Maps this error to an exit code.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            CliError::Http { status, .. } if *status == 403 => ExitCode::PolicyDenied,
            _ => ExitCode::Error,
        }
    }
}

/// Result type for CLI operations.
pub type Result<T> = std::result::Result<T, CliError>;
