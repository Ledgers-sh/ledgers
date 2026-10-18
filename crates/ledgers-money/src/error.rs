//! The error type for every fallible operation in this crate.
//!
//! Money operations never panic: malformed input, mixing currencies and
//! overflow all come back as [`Err`](`Result::Err`)([`MoneyError`]). The
//! `reason` strings of [`MoneyError::ParseError`] and
//! [`MoneyError::InvalidAllocation`] are stable-ish diagnostics (for
//! example `"empty input"`, `"invalid digit"`,
//! `"fraction longer than currency exponent"`, `"misplaced group
//! separator"`, `"fraction longer than 12 digits"`, `"no parts"`,
//! `"weights must be positive"`, `"weights sum overflows"`); match on the
//! variant, not the string, when you need to react.

use crate::currency::Currency;
use thiserror::Error;

/// Every way a money operation can fail.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MoneyError {
    /// Two operands of different currencies were combined. Mixing
    /// currencies is always an error, never an implicit conversion.
    #[error("currency mismatch: expected {}, got {}", expected.code(), actual.code())]
    CurrencyMismatch {
        /// The currency the operation required.
        expected: Currency,
        /// The currency the other operand actually carried.
        actual: Currency,
    },

    /// A value does not fit the integer type it was computed in or
    /// converted to. Checked arithmetic returned `None`.
    #[error("amount overflow")]
    Overflow,

    /// A well-formed three-letter code that is not in the currency table.
    /// The code is reported as normalised (uppercase).
    #[error("unknown currency: {code}")]
    UnknownCurrency {
        /// The normalised code that was looked up.
        code: String,
    },

    /// Input that cannot be a currency code at all (wrong length or
    /// non-ASCII-letter characters).
    #[error("invalid currency code: {input:?}")]
    InvalidCurrencyCode {
        /// The input as it was given.
        input: String,
    },

    /// A numeric code that is not in the currency table.
    #[error("unknown numeric currency code: {numeric}")]
    UnknownNumericCode {
        /// The numeric code that was looked up.
        numeric: u16,
    },

    /// A string could not be parsed as an amount or a rate. Used for both
    /// amount parsing ([`Amount::parse_canonical`] and
    /// [`Amount::parse_locale`]) and rate parsing ([`Rate::parse`]).
    ///
    /// [`Amount::parse_canonical`]: crate::Amount::parse_canonical
    /// [`Amount::parse_locale`]: crate::Amount::parse_locale
    /// [`Rate::parse`]: crate::Rate::parse
    #[error("cannot parse {input:?}: {reason}")]
    ParseError {
        /// The input that failed to parse.
        input: String,
        /// A short, static description of what was wrong.
        reason: &'static str,
    },

    /// Allocation weights were malformed.
    #[error("invalid allocation: {reason}")]
    InvalidAllocation {
        /// A short, static description of what was wrong.
        reason: &'static str,
    },
}
