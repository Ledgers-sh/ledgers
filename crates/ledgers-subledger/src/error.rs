//! Subledger errors and problem definitions.

use cratefield_core::{ProblemDef, axum::http::StatusCode};
use thiserror::Error;

/// Subledger command errors.
#[derive(Debug, Error)]
pub enum SubledgerCommandError {
    /// Allocation exceeds the open item's remaining balance.
    #[error("allocation exceeds open item")]
    AllocationExceedsOpenItem,

    /// Open item not found.
    #[error("open item not found")]
    OpenItemNotFound,

    /// Journal entry not found.
    #[error("entry not found")]
    EntryNotFound,

    /// Control account not found or ambiguous.
    #[error("control account not found or ambiguous")]
    ControlAccountNotFound,

    /// Party not found.
    #[error("party not found")]
    PartyNotFound,

    /// Duplicate `source_id` for the same company and kind.
    #[error("source_id already used")]
    DuplicateSourceId,

    /// Invalid rate specification.
    #[error("invalid rate")]
    InvalidRate,

    /// Invalid amount (zero, wrong sign, etc.).
    #[error("invalid amount")]
    InvalidAmount,

    /// Ledger journal error (wrapped from ledgers-ledger).
    #[error("journal error: {0}")]
    Journal(String),

    /// Database error.
    #[error("database error: {0}")]
    Database(String),

    /// Validation error.
    #[error("{0}")]
    Validation(String),
}

/// Allocation exceeds the open item's remaining balance.
pub const ALLOCATION_EXCEEDS_OPEN_ITEM: ProblemDef = ProblemDef {
    slug: "allocation-exceeds-open-item",
    status: StatusCode::BAD_REQUEST,
    title: "Allocation exceeds open item balance",
    description: "The allocation amount exceeds the remaining balance on the open item.",
};

/// Open item not found.
pub const OPEN_ITEM_NOT_FOUND: ProblemDef = ProblemDef {
    slug: "open-item-not-found",
    status: StatusCode::NOT_FOUND,
    title: "Open item not found",
    description: "The requested open item does not exist.",
};

/// Control account not found or ambiguous.
pub const CONTROL_ACCOUNT_NOT_FOUND: ProblemDef = ProblemDef {
    slug: "control-account-not-found",
    status: StatusCode::BAD_REQUEST,
    title: "Control account not found",
    description: "The control account (ar or ap) does not exist or is ambiguous.",
};

/// Duplicate `source_id` for the same company and kind.
pub const DUPLICATE_SOURCE_ID: ProblemDef = ProblemDef {
    slug: "duplicate-source-id",
    status: StatusCode::CONFLICT,
    title: "Source ID already exists",
    description: "An open item with this source_id already exists for this company and kind.",
};

/// Invalid rate specification.
pub const INVALID_RATE: ProblemDef = ProblemDef {
    slug: "invalid-rate",
    status: StatusCode::BAD_REQUEST,
    title: "Invalid exchange rate",
    description: "The exchange rate is invalid or cannot be parsed.",
};

/// Invalid amount specification.
pub const INVALID_AMOUNT: ProblemDef = ProblemDef {
    slug: "invalid-amount",
    status: StatusCode::BAD_REQUEST,
    title: "Invalid amount",
    description: "The amount is invalid (zero, wrong sign, or exceeds limits).",
};

/// Write-off amount requires person (exceeds agent ceiling).
pub const WRITE_OFF_REQUIRES_PERSON: ProblemDef = ProblemDef {
    slug: "write-off-requires-person",
    status: StatusCode::FORBIDDEN,
    title: "Write-off amount exceeds agent limit",
    description: "Agents can only write off amounts up to the configured ceiling. Larger amounts require human approval.",
};
