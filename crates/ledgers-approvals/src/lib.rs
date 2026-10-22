#![forbid(unsafe_code)]
//! Approval requests with journal preview and optional passkey step-up.
//!
//! An approval request captures what a command would do (the preview),
//! which objects it reads (for change detection), and optionally requires
//! a passkey assertion bound to the request hash so the approval cannot
//! be replayed with different data.
//!
//! Extension points (ADR 0002):
//! - `ApprovalRules`: decide who may approve, and whether passkey is needed
//!   (open-source: one owner approves, passkey for high-risk; cloud: chains/thresholds)
//! - `AssertionVerifier`: verify `WebAuthn` assertions
//!   (open-source: basic P-256 ES256 verifier; cloud: cratefield-auth-passkeys)

pub mod approval;
pub mod config;
pub mod error;
pub mod module;
pub mod rules;
pub mod service;
pub mod stepup;

pub use approval::{ApprovalRequest, Risk, Status};
pub use config::ApprovalConfig;
pub use error::ApprovalError;
pub use module::ApprovalsModule;
pub use rules::{ApprovalRules, OwnerApprovesAll, StepUp};
pub use service::{
    Approvals, FRESH_SIGN_IN_SECS, Listing, NewRequest, check_bulk, get_request, list_requests,
    request_statement, risk_for_effect,
};
pub use stepup::{
    AssertionVerifier, CredentialKeys, Es256Verifier, NoCredentialKeys, PasskeyProof,
};

/// The migrations for the approvals module: nonce and approval request tables.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "approvals",
        include_str!("../migrations/sqlite/0001_approvals.sql"),
    )];

/// The Postgres migrations for the approvals module.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "approvals",
        include_str!("../migrations/postgres/0001_approvals.sql"),
    )];

/// The approval inbox columns (risk, counterparty, decision and result
/// references). A module of their own, registered after every other module,
/// so the D1 files of the modules before it keep their numbers.
pub const INBOX_SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "approvals_inbox",
        include_str!("../migrations/sqlite/inbox/0001_approvals_inbox.sql"),
    )];

/// The Postgres migrations for the approval inbox columns.
pub const INBOX_POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "approvals_inbox",
        include_str!("../migrations/postgres/inbox/0001_approvals_inbox.sql"),
    )];

const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(INBOX_SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(INBOX_POSTGRES_MIGRATIONS);
