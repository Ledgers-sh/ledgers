//! What a connector hands back from a sync (issue #142): invoices, usage
//! lines and notes, already normalised to integer minor units and sanitised
//! text. The server passes this to the import command; the command validates
//! it again before anything is written.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One vendor invoice (or billing line that stands for one) as a bill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvoiceRecord {
    /// The vendor's stable id for this invoice. Re-syncing the same id is a no-op.
    pub external_id: String,
    /// The vendor name as the vendor printed it (becomes the party name).
    pub vendor_name: String,
    /// The invoice number, when the vendor gives one.
    pub invoice_number: Option<String>,
    /// The issue date, `YYYY-MM-DD`.
    pub issued_on: String,
    /// The billing period the invoice covers, `YYYY-MM`. Allocation uses it.
    pub period: String,
    /// The currency code the amount is in (ISO 4217, upper case).
    pub currency: String,
    /// The gross total in minor units of `currency`.
    pub total_minor: i64,
    /// A one-line description, sanitised.
    pub description: String,
    /// A link the vendor gives for the invoice. Never fetched by Ledgers.
    pub hosted_url: Option<String>,
}

/// Usage of one resource in one billing period (the allocation key).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageRecord {
    /// The billing period, `YYYY-MM`.
    pub period: String,
    /// What kind of resource: `worker`, `d1-database`, `api-key`, `organization`.
    pub resource_kind: String,
    /// The vendor's id for the resource (the mapping key).
    pub resource_id: String,
    /// A human name for the resource (suggestions match on it).
    pub resource_name: String,
    /// The weight the resource carries: requests, queries or tokens.
    pub quantity: i64,
}

/// What one sync produced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyncOutput {
    /// The invoices found in the sync window.
    pub invoices: Vec<InvoiceRecord>,
    /// The usage lines the allocation reads.
    pub usage: Vec<UsageRecord>,
    /// Things the sync could not read, as plain sentences for the run log.
    pub notes: Vec<String>,
    /// Journal entries a ledger connector plans from its provider's transactions
    /// (Stripe balance transactions). Each one balances; the server posts them.
    #[serde(default)]
    pub entries: Vec<PlannedEntry>,
}

/// One line of a planned journal entry. `credit` is `false` for a debit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedLine {
    /// The ledger account number.
    pub account: String,
    /// Whether the line is a debit (`true`) or a credit (`false`).
    pub debit: bool,
    /// The amount in minor units of the entry's currency, positive.
    pub minor: i64,
    /// The resource the line is allocated by (a product, an account id); the
    /// server maps it to a business unit through the connection's rules.
    pub resource: Option<String>,
}

/// A journal entry a ledger connector plans from one provider transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedEntry {
    /// The provider's id for the transaction. Idempotency key (per company).
    pub provider_id: String,
    /// The posting date, `YYYY-MM-DD`.
    pub posting_date: String,
    /// The settlement currency code (ISO 4217, upper case). Entries post in it only.
    pub currency: String,
    /// A sanitised one-line description, with the presentment currency and amount.
    pub description: String,
    /// The lines. Debits equal credits.
    pub lines: Vec<PlannedLine>,
    /// Whether the entry went to the suspense account because its category is not known.
    pub suspense: bool,
}

impl PlannedEntry {
    /// Whether the debits equal the credits, in minor units.
    #[must_use]
    pub fn balances(&self) -> bool {
        let debits: i128 = self
            .lines
            .iter()
            .filter(|l| l.debit)
            .map(|l| i128::from(l.minor))
            .sum();
        let credits: i128 = self
            .lines
            .iter()
            .filter(|l| !l.debit)
            .map(|l| i128::from(l.minor))
            .sum();
        debits == credits
    }
}
