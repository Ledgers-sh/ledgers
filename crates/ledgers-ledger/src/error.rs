//! The error type for every fallible operation in this crate.
//!
//! Account operations never panic: malformed input, unknown references
//! and rejected templates all come back as `Err`([`AccountsError`]). Every
//! `Display` string is written to be read by an agent (docs/PLAN.md §4:
//! one command surface whose failures must be self-describing).

use crate::lint::{Violation, format_violations};
use crate::mapping::{MappingViolation, format_mapping_violations};
use thiserror::Error;

/// Every way an account, chart or mapping operation can fail.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AccountsError {
    /// No account with this number exists in the company.
    #[error("account not found: {number}")]
    AccountNotFound {
        /// The number that was looked up.
        number: String,
    },

    /// An account with this number already exists (the chart is keyed by
    /// `(company_id, number)`).
    #[error("account already exists: {number}")]
    AccountExists {
        /// The conflicting number.
        number: String,
    },

    /// The parent named by a draft or import does not exist in the
    /// company — parents are created before children, always.
    #[error("parent account not found: {parent}")]
    ParentNotFound {
        /// The parent number that was referenced.
        parent: String,
    },

    /// A malformed account number.
    #[error("invalid account number {number:?}: {reason}")]
    InvalidNumber {
        /// The number as it was given.
        number: String,
        /// A short, static description of what was wrong.
        reason: &'static str,
    },

    /// A malformed tag (not non-empty lowercase kebab-case, a duplicate,
    /// or too many tags on one account).
    #[error("invalid tag {tag:?}")]
    InvalidTag {
        /// The offending tag (or a description, for the too-many case).
        tag: String,
    },

    /// A name key outside the `account.` namespace.
    #[error("invalid account name key {key:?}: must start with \"account.\"")]
    InvalidNameKey {
        /// The key as it was given.
        key: String,
    },

    /// A currency code `ledgers-money` does not know. Codes are validated
    /// with `Currency::from_code`; this carries the code for agents.
    #[error("unknown currency: {code}")]
    UnknownCurrency {
        /// The code that failed validation.
        code: String,
    },

    /// An account type string that is not one of the five classes.
    #[error("unknown account type: {value:?} (expected asset|liability|equity|income|expense)")]
    UnknownAccountType {
        /// The offending value.
        value: String,
    },

    /// A normal-balance string that is not debit or credit.
    #[error("unknown normal balance: {value:?} (expected debit|credit)")]
    UnknownNormalBalance {
        /// The offending value.
        value: String,
    },

    /// An `ebilanz` key was present but empty; present keys must be
    /// non-empty position identifiers.
    #[error("empty ebilanz position on account {number}")]
    EmptyEbilanzPosition {
        /// The account carrying the empty key.
        number: String,
    },

    /// A chart template failed lint; the violations list every finding so
    /// a data author can fix the file in one pass.
    #[error("chart template rejected: {}", format_violations(violations))]
    TemplateRejected {
        /// Every violation the linter found.
        violations: Vec<Violation>,
    },

    /// An SKR number mapping failed validation against its target chart.
    #[error("number mapping rejected: {}", format_mapping_violations(violations))]
    MappingRejected {
        /// Every violation the validator found.
        violations: Vec<MappingViolation>,
    },

    /// Accounts are never deleted — posted entries are immutable, so the
    /// account they reference must outlive them; archive instead.
    #[error("accounts are never deleted; archive instead (docs/PLAN.md §3 rule 3)")]
    DeleteUnsupported,

    /// A statement failed on the `Database` port.
    #[error(transparent)]
    Database(#[from] cratefield_core::DbError),

    /// A bundled or supplied TOML file could not be parsed at all.
    #[error(transparent)]
    Template(#[from] toml::de::Error),
}
