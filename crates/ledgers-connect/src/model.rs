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
}
