//! Input types for matching: purchase orders, goods receipts, and bills.
//!
//! Quantities use i64 thousandths (0.001) resolution. Prices are stored
//! as `ledgers_money::Amount` to enforce exact decimal arithmetic.

use crate::MatchError;
use ledgers_money::{Amount, Currency};
use serde::{Deserialize, Serialize};

/// Compute line amount with half-even rounding.
///
/// # Errors
///
/// Returns error on overflow in checked arithmetic.
pub fn line_amount(unit: Amount, qty: Quantity) -> Result<Amount, MatchError> {
    let unit_minor = unit.minor();
    let qty_thousandths = i128::from(qty.to_thousandths());

    let product = unit_minor
        .checked_mul(qty_thousandths)
        .ok_or(MatchError::QuantityOverflow)?;

    // Divide by 1000 with half-even rounding.
    let quotient = product / 1000;
    let remainder = product % 1000;
    let rounded = if remainder.abs() > 500 || (remainder.abs() == 500 && quotient.abs() % 2 != 0) {
        quotient
            .checked_add(remainder.signum())
            .ok_or(MatchError::QuantityOverflow)?
    } else {
        quotient
    };

    Ok(Amount::new(rounded, unit.currency()))
}

/// Quantity as i64 thousandths (1000 = 1.0; parse/display in major.fractional).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Quantity(i64);

impl Quantity {
    /// Create from thousandths.
    #[must_use]
    pub const fn from_thousandths(thousandths: i64) -> Self {
        Self(thousandths)
    }

    /// Get internal representation in thousandths.
    #[must_use]
    pub const fn to_thousandths(self) -> i64 {
        self.0
    }

    /// Parse quantity from string (e.g., "12", "2.5", "0.125").
    ///
    /// # Errors
    ///
    /// Returns error on invalid format, too many fractional digits (>3), or overflow.
    ///
    /// # Panics
    ///
    /// Never panics; internal `unwrap()` is safe because padded string contains only ASCII digits.
    pub fn parse(s: &str) -> Result<Self, MatchError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(MatchError::InvalidQuantity("empty string".to_string()));
        }

        // Split on decimal point
        let (major_str, frac_str) = match s.split_once('.') {
            Some((m, f)) => (m, Some(f)),
            None => (s, None),
        };

        // Parse major part (handles negative sign)
        let major: i64 = major_str
            .parse()
            .map_err(|_| MatchError::InvalidQuantity(format!("invalid major part: {major_str}")))?;

        // Parse fractional part, limited to 3 digits
        let frac = match frac_str {
            Some(f) => {
                if f.len() > 3 {
                    return Err(MatchError::InvalidQuantity(format!(
                        "too many fractional digits (max 3): {s}"
                    )));
                }
                if !f.chars().all(|c| c.is_ascii_digit()) {
                    return Err(MatchError::InvalidQuantity(format!(
                        "non-digit in fractional part: {s}"
                    )));
                }
                let padded = format!("{f:0<3}"); // Pad "5" to "500", "25" to "250"
                padded.parse::<i64>().unwrap() // Safe: all digits
            }
            None => 0,
        };

        let thousandths = major
            .checked_mul(1000)
            .and_then(|m| {
                if major < 0 {
                    m.checked_sub(frac) // Negative: -2.5 = -2000 - 500
                } else {
                    m.checked_add(frac)
                }
            })
            .ok_or(MatchError::QuantityOverflow)?;

        Ok(Self(thousandths))
    }

    /// Format as major.fractional (up to 3 digits).
    #[must_use]
    pub fn to_canonical_string(self) -> String {
        let major = self.0 / 1000;
        let frac = (self.0 % 1000).abs();

        if frac == 0 {
            format!("{major}")
        } else {
            format!("{major}.{frac:0>3}")
                .trim_end_matches('0')
                .to_string()
        }
    }
}

impl std::fmt::Display for Quantity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_canonical_string())
    }
}

/// A line item in a purchase order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoLine {
    /// 1-based line number on the PO.
    pub line: u32,
    /// Item code or SKU.
    pub item: String,
    /// Human-readable description.
    pub description: String,
    /// Ordered quantity.
    pub quantity: Quantity,
    /// Unit price (before tax).
    pub unit_price: Amount,
    /// Quantity already invoiced (defaults to 0).
    pub billed: Quantity,
}

/// A purchase order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PurchaseOrder {
    /// PO number (e.g., "PO-1182").
    pub number: String,
    /// Vendor name.
    pub vendor: String,
    /// Currency of the PO.
    pub currency: Currency,
    /// Line items.
    pub lines: Vec<PoLine>,
}

/// A line item in a goods receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptLine {
    /// PO line number (1-based) being received against.
    pub po_line: u32,
    /// Quantity received.
    pub quantity: Quantity,
}

/// A goods receipt or delivery note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoodsReceipt {
    /// Receipt number (e.g., "GR-0931").
    pub number: String,
    /// Vendor name.
    pub vendor: String,
    /// PO number referenced.
    pub po_number: String,
    /// Received line items.
    pub lines: Vec<ReceiptLine>,
}

/// A line item in an invoice being matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchBillLine {
    /// Item code or SKU (optional).
    pub item: Option<String>,
    /// Description.
    pub description: String,
    /// Billed quantity.
    pub quantity: Quantity,
    /// Unit price (before tax).
    pub unit_price: Amount,
}

/// An invoice being matched against POs and receipts.
///
/// Note: This is not the same as `ledgers_bills::Bill`, which does not
/// expose quantity and unit price per line. After matching succeeds, the
/// proposal's line matches are used to enrich the final bill posting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchBill {
    /// Invoice number (e.g., "RE-2026-0418").
    pub number: String,
    /// Vendor name.
    pub vendor: String,
    /// Currency of the invoice.
    pub currency: Currency,
    /// Free-text PO reference from the bill (e.g., "PO-1182", "PO 1182").
    pub po_reference: Option<String>,
    /// Line items.
    pub lines: Vec<MatchBillLine>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_amount_rounding_half_even() {
        let eur = Currency::from_code("EUR").unwrap();

        // 0.05 EUR * 2.5 = 0.125 EUR -> 0.12 EUR (half-even: 0.125 rounds to 0.12 because 12 is even)
        let amount1 = Amount::new(5, eur);
        let qty1 = Quantity::parse("2.5").unwrap();
        let result1 = line_amount(amount1, qty1).unwrap();
        assert_eq!(result1.minor(), 12, "0.05 * 2.5 should round to 0.12");

        // 0.07 EUR * 2.5 = 0.175 EUR -> 0.18 EUR (half-even: 0.175 rounds to 0.18 because 17 is odd)
        let amount2 = Amount::new(7, eur);
        let qty2 = Quantity::parse("2.5").unwrap();
        let result2 = line_amount(amount2, qty2).unwrap();
        assert_eq!(result2.minor(), 18, "0.07 * 2.5 should round to 0.18");
    }
}
