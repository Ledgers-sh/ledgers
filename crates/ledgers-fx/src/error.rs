//! Errors: missing rates, invalid pairs, database failures.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FxError {
    #[error("no {base}/{quote} rate on {date} or in the {window} days before it")]
    NoRateInWindow {
        base: String,
        quote: String,
        date: String,
        window: i64,
    },

    #[error("exchange rate pair must have different currencies: {currency}")]
    SameCurrency { currency: String },

    #[error("unknown currency code: {code}")]
    UnknownCurrency { code: String },

    #[error("invalid rate value: {reason}")]
    InvalidRate { reason: String },

    #[error("CSV import failed at line {line}: {reason}")]
    CsvError { line: usize, reason: String },

    #[error("invalid date: {reason}")]
    InvalidDate { reason: String },

    #[error("duplicate exchange rate key: {base}/{quote} on {date}")]
    DuplicateKey {
        base: String,
        quote: String,
        date: String,
    },

    #[error("database error: {reason}")]
    Database { reason: String },

    #[error(transparent)]
    Money(#[from] ledgers_money::MoneyError),
}
