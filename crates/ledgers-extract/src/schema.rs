//! The extracted-invoice schema. Money is integer minor units of the invoice
//! currency, VAT rates are basis points (19% is 1900), dates are `YYYY-MM-DD`.
//! Every field is optional: a missing field is absent, never a guessed value.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::{Date, Month};

/// One invoice line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ExtractedLine {
    /// The line description, as printed.
    pub description: Option<String>,
    /// The net amount of the line, in minor units.
    pub net_minor: Option<i64>,
    /// The VAT rate in basis points (19% is 1900).
    pub vat_rate_bp: Option<i64>,
    /// The VAT amount of the line, in minor units.
    pub vat_minor: Option<i64>,
    /// The gross amount of the line, in minor units.
    pub gross_minor: Option<i64>,
}

/// The fields of a vendor invoice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ExtractedInvoice {
    /// The vendor's legal name.
    pub vendor_name: Option<String>,
    /// The vendor's VAT identification number, as printed.
    pub vendor_vat_id: Option<String>,
    /// The vendor's IBAN for payment, without spaces.
    pub vendor_iban: Option<String>,
    /// The vendor's invoice number.
    pub invoice_number: Option<String>,
    /// The invoice date, `YYYY-MM-DD`.
    pub invoice_date: Option<String>,
    /// The payment due date, `YYYY-MM-DD`.
    pub due_date: Option<String>,
    /// The ISO 4217 currency code.
    pub currency: Option<String>,
    /// The invoice lines.
    pub lines: Vec<ExtractedLine>,
    /// The net total, in minor units.
    pub net_minor: Option<i64>,
    /// The VAT total, in minor units.
    pub vat_minor: Option<i64>,
    /// The gross total, in minor units.
    pub gross_minor: Option<i64>,
    /// Confidence per field name, 0 to 100.
    pub confidence: BTreeMap<String, u8>,
}

/// Parses `YYYY-MM-DD` into a date.
#[must_use]
pub fn parse_iso_date(text: &str) -> Option<Date> {
    let mut parts = text.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let day: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || text.len() != 10 {
        return None;
    }
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

/// Formats a date as `YYYY-MM-DD`.
#[must_use]
pub fn format_iso_date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}
