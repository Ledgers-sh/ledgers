//! Preview of what a command would do: the debit and credit lines, and
//! computed per-currency totals.

use ledgers_money::{Amount, MoneyError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One line in a preview: an account, side, and amounts.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreviewLine {
    /// The account's number or identifier.
    pub account: String,
    /// Debit or credit.
    pub side: String, // "debit" or "credit"
    /// The amount in the base currency.
    pub base: String, // canonical amount string
    /// The amount in the transaction currency, if different from base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<String>, // canonical amount string
}

/// The journal lines a command would post: debits and credits, and
/// computed per-currency totals (sum of debit-side amounts per currency).
///
/// An empty preview means the command moves no money and cannot exceed
/// an agent's limits.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JournalPreview {
    /// The debit and credit lines.
    pub lines: Vec<PreviewLine>,
    /// Per-currency totals: debit-side base amounts summed per base currency,
    /// and debit-side transaction amounts summed per transaction currency
    /// (when they differ from base). Empty for an empty preview.
    pub totals: BTreeMap<String, String>, // currency code => canonical amount string
}

impl JournalPreview {
    /// Creates an empty preview (no lines, no movement).
    #[must_use]
    pub fn empty() -> Self {
        JournalPreview {
            lines: vec![],
            totals: BTreeMap::new(),
        }
    }

    /// Adds a line to the preview, computing totals as we go.
    ///
    /// The `base` amount is mandatory; `transaction` is optional and
    /// represents an amount in a different currency, converted at some rate.
    ///
    /// # Errors
    /// Returns a `MoneyError` if totals cannot be computed (e.g., overflow).
    pub fn add_line(
        &mut self,
        account: String,
        side: &str,
        base: Amount,
        transaction: Option<Amount>,
    ) -> Result<(), MoneyError> {
        self.lines.push(PreviewLine {
            account,
            side: side.to_string(),
            base: base.to_canonical_string(),
            transaction: transaction.map(ledgers_money::Amount::to_canonical_string),
        });

        // Only debit-side amounts count toward totals.
        if side != "debit" {
            return Ok(());
        }

        // Add base amount.
        let base_code = base.currency().code().to_owned();
        let base_entry = self
            .totals
            .entry(base_code.clone())
            .or_insert_with(|| Amount::new(0, base.currency()).to_canonical_string());

        let current_base = Amount::parse_canonical(base_entry, base.currency())?;
        let new_base = current_base.checked_add(base)?;
        self.totals
            .insert(base_code, new_base.to_canonical_string());

        // Add transaction amount if present.
        if let Some(txn) = transaction {
            let txn_code = txn.currency().code().to_owned();
            let txn_entry = self
                .totals
                .entry(txn_code.clone())
                .or_insert_with(|| Amount::new(0, txn.currency()).to_canonical_string());

            let current_txn = Amount::parse_canonical(txn_entry, txn.currency())?;
            let new_txn = current_txn.checked_add(txn)?;
            self.totals.insert(txn_code, new_txn.to_canonical_string());
        }

        Ok(())
    }

    /// Returns true if this preview has no lines (the command moves no money).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_preview() {
        let p = JournalPreview::empty();
        assert!(p.is_empty());
        assert_eq!(p.lines.len(), 0);
        assert_eq!(p.totals.len(), 0);
    }
}
