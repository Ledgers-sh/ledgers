//! The error type for every fallible operation in this crate.
//!
//! Tax operations never panic: invalid rates, malformed codes, and
//! calculation failures all come back as `Err`([`TaxError`]).

use thiserror::Error;

/// Every way a tax operation can fail.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaxError {
    /// A tax code with this id already exists for the jurisdiction.
    #[error("tax code already exists: {code}")]
    CodeExists {
        /// The code that was duplicated.
        code: String,
    },

    /// A tax code with this id was not found.
    #[error("tax code not found: {code}")]
    CodeNotFound {
        /// The code that was looked up.
        code: String,
    },

    /// A rate for the jurisdiction, category, and date was not found —
    /// rate table has a gap.
    #[error("no rate found for {jurisdiction} {category} on {date}")]
    RateNotFound {
        /// The jurisdiction code.
        jurisdiction: String,
        /// The rate category.
        category: String,
        /// The date the rate was needed for.
        date: String,
    },

    /// A rate table would have overlapping or gapped date ranges.
    #[error("rate table has gaps or overlaps: {reason}")]
    RateTableInvalid {
        /// Details of what was wrong.
        reason: String,
    },

    /// A tax code or rate has malformed or invalid data.
    #[error("invalid tax data: {reason}")]
    Invalid {
        /// What was wrong.
        reason: String,
    },

    /// A non-deductible share is outside [0, 1].
    #[error("non-deductible share must be 0..=1, got {value}")]
    InvalidNonDeductible {
        /// The value that was provided.
        value: String,
    },

    /// A required account is missing for the tax code kind.
    #[error("tax code {code} requires {kind:?} but {missing} account is missing")]
    MissingAccount {
        /// The code that was invalid.
        code: String,
        /// The kind that requires the account.
        kind: String,
        /// Which account (output or input).
        missing: String,
    },

    /// An exact-money operation failed.
    #[error(transparent)]
    Money(#[from] ledgers_money::MoneyError),
}
