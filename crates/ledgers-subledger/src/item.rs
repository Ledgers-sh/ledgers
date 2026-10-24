//! Open items and settlement logic.
//!
//! An open item represents an invoice, credit note, bill, or on-account
//! deposit in AR or AP, with remaining balance tracking. Settlement applies
//! payments, receipts, offsets and write-offs to reduce the remaining balance.

use ledgers_money::{Amount, Currency, Rounding};
use serde::{Deserialize, Serialize};

/// The ledger: AR (accounts receivable) or AP (accounts payable).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ledger {
    #[serde(rename = "ar")]
    Ar,
    #[serde(rename = "ap")]
    Ap,
}

impl Ledger {
    /// The control account tag for this ledger.
    #[must_use]
    pub fn control_tag(&self) -> &'static str {
        match self {
            Ledger::Ar => "ar",
            Ledger::Ap => "ap",
        }
    }
}

impl std::fmt::Display for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ledger::Ar => write!(f, "ar"),
            Ledger::Ap => write!(f, "ap"),
        }
    }
}

/// The kind of open item: invoice, credit note, bill, or on-account.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    /// AR or AP invoice.
    Invoice,
    /// AR or AP credit note.
    #[serde(rename = "credit-note")]
    CreditNote,
    /// AP bill (synonym for AP invoice).
    Bill,
    /// On-account deposit or prepayment (unapplied receipt or payment).
    #[serde(rename = "on-account")]
    OnAccount,
}

impl std::fmt::Display for ItemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemKind::Invoice => write!(f, "invoice"),
            ItemKind::CreditNote => write!(f, "credit-note"),
            ItemKind::Bill => write!(f, "bill"),
            ItemKind::OnAccount => write!(f, "on-account"),
        }
    }
}

/// An open item: an invoice, bill, credit note or on-account item in AR/AP,
/// mirroring the `open_item` table row. Amounts are signed in the control
/// account's normal direction: AR invoice +, AR credit note −; AP bill +,
/// AP credit note −; on-account items are − (an unapplied receipt/payment).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenItem {
    /// Unique identifier.
    pub id: String,
    /// Company the item belongs to.
    pub company_id: String,
    /// Ledger: ar or ap.
    pub ledger: Ledger,
    /// Kind: invoice, credit-note, bill, or on-account.
    pub kind: ItemKind,
    /// Party (vendor in AP, customer in AR).
    pub party_id: String,
    /// Optional source reference (e.g., invoice number), unique per company+kind when present.
    pub source_id: Option<String>,
    /// The journal entry this item was posted from.
    pub entry_id: String,
    /// The control account number (ar or ap tag).
    pub control_account: String,
    /// The transaction currency.
    pub currency: String,
    /// The FX rate as a scaled i128 (text), or null/1 when currency == base.
    pub rate_scaled: Option<String>,
    /// The posting date (ISO 8601).
    pub posting_date: String,
    /// The due date (ISO 8601), optional.
    pub due_date: Option<String>,
    /// The original amount in transaction currency (minor units, signed).
    pub tx_original_minor: i64,
    /// The original amount in base currency (minor units, signed).
    pub base_original_minor: i64,
    /// The remaining amount in transaction currency (minor units, signed).
    pub tx_remaining_minor: i64,
    /// The remaining amount in base currency (minor units, signed).
    pub base_remaining_minor: i64,
}

/// An error in subledger operations.
#[derive(Clone, Debug, PartialEq)]
pub enum SubledgerError {
    /// The transaction amount applied is zero.
    ZeroAmount,
    /// The transaction amount has the wrong sign (opposite of remaining).
    WrongSign,
    /// The transaction amount exceeds the remaining balance.
    ExceedsRemaining,
    /// The exchange rate is invalid (overflow, parsing error, etc.).
    InvalidRate,
}

impl std::fmt::Display for SubledgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubledgerError::ZeroAmount => write!(f, "transaction amount is zero"),
            SubledgerError::WrongSign => write!(f, "transaction amount has the wrong sign"),
            SubledgerError::ExceedsRemaining => {
                write!(f, "transaction amount exceeds remaining balance")
            }
            SubledgerError::InvalidRate => write!(f, "invalid exchange rate"),
        }
    }
}

impl std::error::Error for SubledgerError {}

/// Settles an open item by a transaction amount, returning the base amount cleared.
///
/// If `tx_applied == item.tx_remaining_minor`, returns exactly `item.base_remaining_minor`,
/// ensuring the final settlement is exact. Otherwise, the base cleared is computed by
/// multiplying `tx_applied` by the rate, rounded to nearest even.
///
/// # Errors
///
/// - `ZeroAmount` if `tx_applied` is zero.
/// - `WrongSign` if `tx_applied` has the opposite sign from the remaining balance.
/// - `ExceedsRemaining` if `|tx_applied|` > `|item.tx_remaining_minor|`.
/// - `InvalidRate` if the rate cannot be parsed or overflows.
pub fn settle(item: &OpenItem, tx_applied_minor: i64) -> Result<i64, SubledgerError> {
    // Zero amount is an error.
    if tx_applied_minor == 0 {
        return Err(SubledgerError::ZeroAmount);
    }

    // Sign must match the remaining balance (both positive or both negative).
    let remaining_sign = if item.tx_remaining_minor > 0 { 1 } else { -1 };
    let applied_sign = if tx_applied_minor > 0 { 1 } else { -1 };
    if remaining_sign != applied_sign {
        return Err(SubledgerError::WrongSign);
    }

    // Magnitude must not exceed remaining.
    if tx_applied_minor.abs() > item.tx_remaining_minor.abs() {
        return Err(SubledgerError::ExceedsRemaining);
    }

    // If settling the full remaining amount, return exactly the base remaining.
    if tx_applied_minor == item.tx_remaining_minor {
        return Ok(item.base_remaining_minor);
    }

    // Otherwise, apply the rate.
    let rate = if item.currency.to_uppercase() == "EUR" || item.rate_scaled.is_none() {
        // Base currency: rate is implicitly 1.
        ledgers_money::Rate::ONE
    } else {
        // The rate_scaled is already scaled as an i128 value (text), so parse as a number and create a Rate from it.
        let rate_str = item
            .rate_scaled
            .as_ref()
            .ok_or(SubledgerError::InvalidRate)?;
        let scaled: i128 = rate_str.parse().map_err(|_| SubledgerError::InvalidRate)?;
        ledgers_money::Rate::from_scaled(scaled)
    };

    // Parse currency for the Amount
    let currency = Currency::from_code(&item.currency).map_err(|_| SubledgerError::InvalidRate)?;

    // Multiply: base_cleared = tx_applied * rate, rounded to nearest even.
    let tx_amount = Amount::new(i128::from(tx_applied_minor), currency);
    let base_cleared = tx_amount
        .mul_rate(rate, Rounding::HalfEven)
        .map_err(|_| SubledgerError::InvalidRate)?;

    i64::try_from(base_cleared.minor()).map_err(|_| SubledgerError::InvalidRate)
}

/// Computes the realised FX difference (gain or loss) when settling, in minor units.
///
/// For AR (receivables):
/// - If `paid_base > base_cleared`: you collected more base than the item was worth → FX gain (positive).
/// - If `paid_base < base_cleared`: you collected less base than the item was worth → FX loss (negative).
///
/// For AP (payables):
/// - If `paid_base > base_cleared`: you paid more base than the item cost → FX loss (negative).
/// - If `paid_base < base_cleared`: you paid less base than the item cost → FX gain (positive).
///
/// In both cases, the formula is: `fx_difference = paid_base - base_cleared` for AR,
/// and `fx_difference = base_cleared - paid_base` for AP.
#[must_use]
pub fn fx_difference(ledger: Ledger, paid_base_minor: i64, base_cleared_minor: i64) -> i64 {
    match ledger {
        Ledger::Ar => paid_base_minor - base_cleared_minor, // positive = gain
        Ledger::Ap => base_cleared_minor - paid_base_minor, // positive = gain (opposite sign)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_full_amount_returns_exact_base() {
        let item = OpenItem {
            id: "item1".to_owned(),
            company_id: "co1".to_owned(),
            ledger: Ledger::Ar,
            kind: ItemKind::Invoice,
            party_id: "cust1".to_owned(),
            source_id: None,
            entry_id: "entry1".to_owned(),
            control_account: "1400".to_owned(),
            currency: "USD".to_owned(),
            rate_scaled: Some("900000000000".to_owned()), // 0.9
            posting_date: "2026-10-01".to_owned(),
            due_date: None,
            tx_original_minor: 1000,
            base_original_minor: 900,
            tx_remaining_minor: 1000,
            base_remaining_minor: 900,
        };

        let base_cleared = settle(&item, 1000).unwrap();
        assert_eq!(base_cleared, 900);
    }

    #[test]
    fn settle_partial_amount_uses_rate() {
        let item = OpenItem {
            id: "item2".to_owned(),
            company_id: "co1".to_owned(),
            ledger: Ledger::Ar,
            kind: ItemKind::Invoice,
            party_id: "cust1".to_owned(),
            source_id: None,
            entry_id: "entry2".to_owned(),
            control_account: "1400".to_owned(),
            currency: "USD".to_owned(),
            rate_scaled: Some("900000000000".to_owned()), // 0.9
            posting_date: "2026-10-01".to_owned(),
            due_date: None,
            tx_original_minor: 1000,
            base_original_minor: 900,
            tx_remaining_minor: 1000,
            base_remaining_minor: 900,
        };

        let base_cleared = settle(&item, 500).unwrap(); // Half
        // 500 * 0.9 = 450
        assert_eq!(base_cleared, 450);
    }

    #[test]
    fn settle_zero_amount_errors() {
        let item = OpenItem {
            id: "item3".to_owned(),
            company_id: "co1".to_owned(),
            ledger: Ledger::Ar,
            kind: ItemKind::Invoice,
            party_id: "cust1".to_owned(),
            source_id: None,
            entry_id: "entry3".to_owned(),
            control_account: "1400".to_owned(),
            currency: "EUR".to_owned(),
            rate_scaled: None,
            posting_date: "2026-10-01".to_owned(),
            due_date: None,
            tx_original_minor: 1000,
            base_original_minor: 1000,
            tx_remaining_minor: 1000,
            base_remaining_minor: 1000,
        };

        assert_eq!(settle(&item, 0), Err(SubledgerError::ZeroAmount));
    }

    #[test]
    fn settle_wrong_sign_errors() {
        let item = OpenItem {
            id: "item4".to_owned(),
            company_id: "co1".to_owned(),
            ledger: Ledger::Ar,
            kind: ItemKind::Invoice,
            party_id: "cust1".to_owned(),
            source_id: None,
            entry_id: "entry4".to_owned(),
            control_account: "1400".to_owned(),
            currency: "EUR".to_owned(),
            rate_scaled: None,
            posting_date: "2026-10-01".to_owned(),
            due_date: None,
            tx_original_minor: 1000,
            base_original_minor: 1000,
            tx_remaining_minor: 1000,
            base_remaining_minor: 1000,
        };

        assert_eq!(settle(&item, -500), Err(SubledgerError::WrongSign)); // Wrong sign
    }

    #[test]
    fn settle_exceeds_remaining_errors() {
        let item = OpenItem {
            id: "item5".to_owned(),
            company_id: "co1".to_owned(),
            ledger: Ledger::Ar,
            kind: ItemKind::Invoice,
            party_id: "cust1".to_owned(),
            source_id: None,
            entry_id: "entry5".to_owned(),
            control_account: "1400".to_owned(),
            currency: "EUR".to_owned(),
            rate_scaled: None,
            posting_date: "2026-10-01".to_owned(),
            due_date: None,
            tx_original_minor: 1000,
            base_original_minor: 1000,
            tx_remaining_minor: 1000,
            base_remaining_minor: 1000,
        };

        assert_eq!(settle(&item, 1500), Err(SubledgerError::ExceedsRemaining)); // Exceeds 1000
    }

    #[test]
    fn ar_fx_difference_gain() {
        let diff = fx_difference(Ledger::Ar, 950, 900);
        // AR: paid_base - base_cleared = 950 - 900 = 50 (gain)
        assert_eq!(diff, 50);
    }

    #[test]
    fn ar_fx_difference_loss() {
        let diff = fx_difference(Ledger::Ar, 850, 900);
        // AR: paid_base - base_cleared = 850 - 900 = -50 (loss)
        assert_eq!(diff, -50);
    }

    #[test]
    fn ap_fx_difference_gain() {
        let diff = fx_difference(Ledger::Ap, 850, 900);
        // AP: base_cleared - paid_base = 900 - 850 = 50 (gain)
        assert_eq!(diff, 50);
    }

    #[test]
    fn ap_fx_difference_loss() {
        let diff = fx_difference(Ledger::Ap, 950, 900);
        // AP: base_cleared - paid_base = 900 - 950 = -50 (loss)
        assert_eq!(diff, -50);
    }

    #[test]
    fn credit_note_negative_sign() {
        let item = OpenItem {
            id: "cn1".to_owned(),
            company_id: "co1".to_owned(),
            ledger: Ledger::Ar,
            kind: ItemKind::CreditNote,
            party_id: "cust1".to_owned(),
            source_id: None,
            entry_id: "entry6".to_owned(),
            control_account: "1400".to_owned(),
            currency: "EUR".to_owned(),
            rate_scaled: None,
            posting_date: "2026-10-01".to_owned(),
            due_date: None,
            tx_original_minor: -500, // Credit note is negative
            base_original_minor: -500,
            tx_remaining_minor: -500,
            base_remaining_minor: -500,
        };

        let base_cleared = settle(&item, -500).unwrap();
        assert_eq!(base_cleared, -500);
    }
}
