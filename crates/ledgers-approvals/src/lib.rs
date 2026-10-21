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

pub use approval::{ApprovalRequest, Status};
pub use config::ApprovalConfig;
pub use error::ApprovalError;
pub use module::ApprovalsModule;
pub use rules::{ApprovalRules, OwnerApprovesAll, StepUp};
pub use service::Approvals;
pub use stepup::{AssertionVerifier, CredentialKeys, Es256Verifier, PasskeyProof};

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

const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
