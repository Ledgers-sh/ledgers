//! The statement readers: one module per file format, each pure and
//! returning [`ParsedStatement`]s. Nothing here touches the database.
//!
//! Remittance and counterparty text is copied as data. No reader interprets
//! it, and none of it changes how a line is parsed.

pub mod camt053;
pub mod csv;
pub mod mt940;
pub mod ofx;

use crate::statement::{Format, ParseError, ParsedStatement};
use ledgers_money::Currency;

pub use csv::{ColumnRef, CsvMapping};

/// The exponent of a currency code, or a malformed-statement error.
pub(crate) fn finish_currency(code: &str) -> Result<u32, ParseError> {
    Currency::from_code(code)
        .map(Currency::exponent)
        .map_err(|_| ParseError::Malformed(format!("unknown currency {code:?}")))
}

/// `+1` for the credit indicator, `-1` for the debit indicator.
pub(crate) fn indicator_sign(
    value: Option<&str>,
    credit: &str,
    debit: &str,
) -> Result<i64, ParseError> {
    match value {
        Some(text) if text == credit => Ok(1),
        Some(text) if text == debit => Ok(-1),
        other => Err(ParseError::Malformed(format!(
            "expected {credit} or {debit}, found {other:?}"
        ))),
    }
}

/// Whether text has the shape of an IBAN: two letters, two digits, then
/// alphanumerics. The check digits are verified later, by [`crate::iban`].
pub(crate) fn iban_like(text: &str) -> bool {
    let normalised = crate::iban::normalise(text);
    let bytes = normalised.as_bytes();
    bytes.len() >= 15
        && bytes[..2].iter().all(u8::is_ascii_uppercase)
        && bytes[2..4].iter().all(u8::is_ascii_digit)
        && bytes[4..].iter().all(u8::is_ascii_alphanumeric)
}

/// The formats the import can recognise from the bytes.
#[must_use]
pub fn detect(bytes: &[u8]) -> Option<Format> {
    let window = &bytes[..bytes.len().min(4096)];
    let head = match std::str::from_utf8(window) {
        Ok(head) => head,
        // The window may end inside a multibyte character: sniff the whole
        // characters before it. Bytes that are not UTF-8 are not a statement.
        Err(err) if err.error_len().is_none() => {
            std::str::from_utf8(&window[..err.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    let trimmed = head.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with('<') {
        if trimmed.contains("camt.053") || trimmed.contains("BkToCstmrStmt") {
            return Some(Format::Camt053);
        }
        if trimmed.contains("camt.052") || trimmed.contains("BkToCstmrAcctRpt") {
            return Some(Format::Camt052);
        }
        if trimmed.starts_with("<?OFX") || trimmed.contains("<OFX>") {
            return Some(Format::Ofx);
        }
    }
    if trimmed.starts_with("OFXHEADER:") {
        return Some(Format::Ofx);
    }
    if trimmed.starts_with("{1:") || trimmed.starts_with(":20:") || trimmed.contains("\n:20:") {
        return Some(Format::Mt940);
    }
    None
}

/// Reads a file in the given format.
///
/// `csv_mapping` is required for CSV and ignored otherwise; `currency` is the
/// account's currency, which a CSV without a currency column takes.
///
/// # Errors
/// The format's [`ParseError`]; for CSV without a mapping, a malformed error.
pub fn parse(
    format: Format,
    bytes: &[u8],
    csv_mapping: Option<&CsvMapping>,
    currency: &str,
) -> Result<Vec<ParsedStatement>, ParseError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ParseError::Malformed("the file is not UTF-8 text".to_owned()))?;
    // A byte-order mark is not part of the content, so it never reaches a reader.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match format {
        Format::Camt053 | Format::Camt052 => camt053::parse(text),
        Format::Mt940 => mt940::parse(text),
        Format::Ofx => ofx::parse(text, currency),
        Format::Csv => {
            let mapping = csv_mapping.ok_or_else(|| {
                ParseError::Malformed("a CSV import needs a column mapping".to_owned())
            })?;
            csv::parse(text, mapping, currency).map(|statement| vec![statement])
        }
    }
}
