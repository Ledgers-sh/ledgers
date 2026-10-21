//! Errors from the approvals service.

use ledgers_commands::CommandError;

/// Errors from the approvals service.
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    /// The approval request has been superseded (command or reads changed).
    #[error("approval superseded")]
    Superseded,

    /// The approval request has expired.
    #[error("approval expired")]
    Expired,

    /// The approval is not in pending status.
    #[error("approval status is {status}, not pending")]
    NotPending {
        /// The actual status.
        status: String,
    },

    /// Step-up (passkey) is required but not provided.
    #[error("passkey step-up required")]
    StepUpRequired,

    /// The passkey assertion is invalid or verification failed, or a guard assertion failed.
    #[error("invalid assertion")]
    InvalidAssertion,

    /// The principal is not allowed to approve (e.g., an agent).
    #[error("not authorized to approve")]
    Forbidden,

    /// A command execution error (wrapped from ledgers-commands).
    #[error(transparent)]
    Command(#[from] CommandError),

    /// A JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A database error.
    #[error("database error: {0}")]
    Database(String),

    /// A general error.
    #[error("{0}")]
    Other(String),
}
