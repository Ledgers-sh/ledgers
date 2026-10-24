//! Data models for invoices and credit notes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input for creating and issuing an invoice.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct IssueInvoiceInput {
    /// The customer ID (must have role customer/both in parties).
    pub customer_id: String,
    /// Invoice series (e.g. "RE", "INV"). Defaults to "RE".
    #[serde(default = "default_series")]
    pub series: String,
    /// Issue date (ISO 8601).
    pub issue_date: String,
    /// Supply date / Leistungsdatum (ISO 8601, defaults to `issue_date`).
    #[serde(default)]
    pub supply_date: Option<String>,
    /// Payment terms in days (defaults to customer's terms, then 14).
    #[serde(default)]
    pub payment_terms_days: Option<u32>,
    /// Currency code (defaults to company base currency).
    #[serde(default)]
    pub currency: Option<String>,
    /// FX rate as a decimal string (required if `currency != base`).
    /// Parsed as a `ledgers_money::Rate`.
    #[serde(default)]
    pub rate: Option<String>,
    /// Invoice lines.
    pub lines: Vec<InvoiceLineInput>,
}

fn default_series() -> String {
    "RE".to_string()
}

/// A single line in an invoice.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct InvoiceLineInput {
    /// Text description (escaped in output; document text is data).
    pub description: String,
    /// Quantity as a decimal string.
    pub quantity: String,
    /// Unit of measure (e.g. "h", "kg").
    pub unit: Option<String>,
    /// Unit price in minor units (optional if `net_minor` provided).
    pub unit_price_minor: Option<i64>,
    /// Net line amount in minor units (optional if `unit_price_minor` provided).
    pub net_minor: Option<i64>,
    /// Tax code (e.g. "VAT-19-DE", "VAT-0-INTRA-EU", "VAT-0-REVERSE").
    pub tax_code: String,
    /// Revenue account number (optional: defaults to customer's `default_account`).
    pub revenue_account: Option<String>,
}

/// Output of a successful invoice issue.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct IssueInvoiceOutput {
    /// Invoice ID.
    pub invoice_id: String,
    /// Invoice number (display: `{series}-{fiscal_year}-{number:04d}`).
    pub invoice_number: String,
    /// Posted entry ID.
    pub entry_id: String,
    /// Totals in the invoice currency.
    pub totals: InvoiceTotals,
    /// Due date (ISO 8601).
    pub due_date: String,
}

/// Invoice totals.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct InvoiceTotals {
    pub net_minor: i64,
    pub tax_minor: i64,
    pub gross_minor: i64,
    pub currency: String,
}

/// Input for creating a credit note.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreditNoteInput {
    /// The invoice ID to credit.
    pub invoice_id: String,
    /// Credit note series (defaults to "CN").
    #[serde(default = "default_cn_series")]
    pub series: String,
    /// Issue date for the credit note (its own posting period).
    pub issue_date: String,
    /// Reason for the credit note.
    #[serde(default)]
    pub reason: Option<String>,
    /// Either lines (specific line credits) or amount (gross amount spread pro-rata).
    #[serde(flatten)]
    pub credit_spec: CreditSpec,
}

fn default_cn_series() -> String {
    "CN".to_string()
}

/// Credit specification: either specific lines or a gross amount.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CreditSpec {
    /// Credit specific invoice lines with amounts.
    Lines { lines: Vec<CreditLineInput> },
    /// Credit a gross amount, spread pro-rata across lines.
    Amount { amount_minor: i64 },
}

/// A credit line specification.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreditLineInput {
    /// Invoice line ID.
    pub invoice_line_id: String,
    /// Net amount to credit in minor units (must not exceed line net).
    pub net_minor: i64,
}

/// Output of a successful credit note.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreditNoteOutput {
    /// Credit note ID.
    pub credit_note_id: String,
    /// Credit note number.
    pub credit_note_number: String,
    /// Posted entry ID.
    pub entry_id: String,
    /// Total amount credited in the invoice currency.
    pub total_credited_minor: i64,
    pub currency: String,
}

/// An invoice for rendering (HTML template).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvoiceDocument {
    pub seller_name: String,
    pub seller_address: String,
    pub seller_tax_number: Option<String>,
    pub seller_vat_id: Option<String>,
    pub buyer_name: String,
    pub buyer_address: String,
    pub buyer_vat_id: Option<String>,
    pub invoice_number: String,
    pub issue_date: String,
    pub supply_date: String,
    pub lines: Vec<DocumentLine>,
    pub tax_breakdown: Vec<TaxBreakdown>,
    pub currency: String,
    pub totals: InvoiceTotals,
    pub due_date: Option<String>,
    pub note: Option<String>,
    /// Legal note for intra-EU zero-rated supplies (§ 4 Nr. 1b i.V.m. § 6a `UStG`).
    pub intra_eu_note: Option<String>,
    /// Legal note for reverse-charge supplies (Steuerschuldnerschaft des
    /// Leistungsempfängers).
    pub reverse_charge_note: Option<String>,
}

/// A line in an invoice document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentLine {
    pub sequence: u32,
    pub description: String,
    pub quantity: String,
    pub unit: Option<String>,
    pub net_minor: i64,
}

/// Tax breakdown per rate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaxBreakdown {
    pub tax_rate_percent: String,
    pub net_minor: i64,
    pub tax_minor: i64,
    pub kind: String, // "standard", "intra-eu", "reverse-charge", etc.
}
