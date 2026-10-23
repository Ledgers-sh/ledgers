#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
//! 2-way and 3-way purchase order, goods receipt, and invoice matching with
//! confidence scoring.
//!
//! **2-way:** PO + invoice.
//! **3-way:** PO + goods receipts + invoice (when receipts exist).
//!
//! Tolerances are per-vendor with separate quantity and price variance rules
//! (absolute and percent limits, both must be satisfied). Confidence is computed as:
//!
//! - 30 (vendor and currency match)
//! - +20 if PO reference matches
//! - +`30×matched_lines/n` (integer floor)
//! - +`10×quantity_ok/n` (integer floor)
//! - +`10×price_ok/n` (integer floor)
//!
//! where n = max(1, total bill lines). Perfect match with PO reference scores 100;
//! without reference, max is 80. Auto-accept when no blockers and confidence ≥ 90.
//! Output sorted by confidence descending, then PO number.

pub mod csv;
pub mod matching;
pub mod model;
pub mod tolerance;

use thiserror::Error;

/// Errors in matching, CSV import, or tolerance checking.
#[derive(Debug, Error)]
pub enum MatchError {
    #[error("CSV parse error at line {row}: {message}")]
    Csv { row: usize, message: String },

    #[error("No POs found matching vendor and currency")]
    NoPosFound,

    #[error("Invalid quantity: {0}")]
    InvalidQuantity(String),

    #[error("Invalid unit price: {0}")]
    InvalidPrice(String),

    #[error("Quantity overflow")]
    QuantityOverflow,

    #[error("Money arithmetic error: {0}")]
    MoneyError(String),
}

impl From<ledgers_money::MoneyError> for MatchError {
    fn from(err: ledgers_money::MoneyError) -> Self {
        MatchError::MoneyError(err.to_string())
    }
}

pub use matching::{AUTO_ACCEPT_CONFIDENCE, Proposal, propose};
pub use model::{GoodsReceipt, MatchBill, MatchBillLine, PurchaseOrder, Quantity, line_amount};
pub use tolerance::{Tolerance, Tolerances};
