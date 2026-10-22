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

/// The keys of the invoice object, as the schema names them.
pub const INVOICE_KEYS: &[&str] = &[
    "vendor_name",
    "vendor_vat_id",
    "vendor_iban",
    "invoice_number",
    "invoice_date",
    "due_date",
    "currency",
    "lines",
    "net_minor",
    "vat_minor",
    "gross_minor",
    "confidence",
];

/// The keys of one invoice line, as the schema names them.
pub const LINE_KEYS: &[&str] = &[
    "description",
    "net_minor",
    "vat_rate_bp",
    "vat_minor",
    "gross_minor",
];

/// Checks a model's reply against the schema before it is read: only the
/// schema's keys are accepted (an unknown key is a malformed reply, not a
/// silently dropped field), lines are objects, and every confidence is an
/// integer from 0 to 100.
///
/// # Errors
/// A message naming the first thing that does not match the schema.
pub fn check_reply_shape(value: &serde_json::Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "the reply is not an object".to_owned())?;
    if let Some(key) = object
        .keys()
        .find(|key| !INVOICE_KEYS.contains(&key.as_str()))
    {
        return Err(format!("unknown field `{key}`"));
    }
    if let Some(lines) = object.get("lines") {
        let lines = lines
            .as_array()
            .ok_or_else(|| "`lines` is not a list".to_owned())?;
        for line in lines {
            let line = line
                .as_object()
                .ok_or_else(|| "a line is not an object".to_owned())?;
            if let Some(key) = line.keys().find(|key| !LINE_KEYS.contains(&key.as_str())) {
                return Err(format!("unknown line field `{key}`"));
            }
        }
    }
    if let Some(confidence) = object.get("confidence") {
        let map = confidence
            .as_object()
            .ok_or_else(|| "`confidence` is not an object".to_owned())?;
        for (field, score) in map {
            match score.as_u64() {
                Some(value) if value <= 100 => {}
                _ => return Err(format!("confidence for `{field}` is not 0 to 100")),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod shape_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_key_lists_are_the_schema() {
        let schema = serde_json::to_value(schemars::schema_for!(ExtractedInvoice))
            .expect("schema serialises");
        let mut invoice: Vec<&str> = schema["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .map(String::as_str)
            .collect();
        invoice.sort_unstable();
        let mut wanted: Vec<&str> = INVOICE_KEYS.to_vec();
        wanted.sort_unstable();
        assert_eq!(invoice, wanted);
        let line = &schema["$defs"]["ExtractedLine"]["properties"];
        let mut line_keys: Vec<&str> = line
            .as_object()
            .expect("line properties")
            .keys()
            .map(String::as_str)
            .collect();
        line_keys.sort_unstable();
        let mut wanted_line: Vec<&str> = LINE_KEYS.to_vec();
        wanted_line.sort_unstable();
        assert_eq!(line_keys, wanted_line);
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let reply = json!({"gross_minor": 1, "approve": true});
        assert!(check_reply_shape(&reply).unwrap_err().contains("approve"));
    }

    #[test]
    fn an_unknown_line_key_is_refused() {
        let reply = json!({"lines": [{"description": "x", "post_to": 1000}]});
        assert!(check_reply_shape(&reply).is_err());
    }

    #[test]
    fn a_confidence_above_100_is_refused() {
        assert!(check_reply_shape(&json!({"confidence": {"gross_minor": 101}})).is_err());
        assert!(check_reply_shape(&json!({"confidence": {"gross_minor": 100}})).is_ok());
    }

    #[test]
    fn a_plain_reply_is_accepted() {
        assert!(check_reply_shape(&json!({"gross_minor": 119_000, "lines": []})).is_ok());
    }
}
