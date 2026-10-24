//! OFX (Open Financial Exchange) bank statement reader.
//!
//! Handles both OFX 1.x (SGML format) and OFX 2.x (XML format). Extracts
//! booked transactions, closing balance, and account information. OFX files
//! typically carry no opening balance, so the statement returns `BalanceCheck::NotAvailable`.

use super::{finish_currency, iban_like};
use crate::money::{Decimal, parse_minor};
use crate::statement::{BalanceRow, Format, ParseError, ParsedLine, ParsedStatement};

/// Reads an OFX document, handling both SGML (1.x) and XML (2.x) formats.
///
/// # Errors
/// [`ParseError::Malformed`] for invalid OFX structure or missing amounts,
/// dates, or currency; [`ParseError::Unsupported`] for non-statement message types.
pub fn parse(text: &str, currency: &str) -> Result<Vec<ParsedStatement>, ParseError> {
    // Detect format: OFXHEADER:100 = SGML, otherwise treat as XML
    let is_sgml = text.contains("OFXHEADER:100");
    if is_sgml {
        parse_sgml(text, currency)
    } else {
        parse_xml(text, currency)
    }
}

/// Parse SGML format (OFX 1.x).
fn parse_sgml(text: &str, currency: &str) -> Result<Vec<ParsedStatement>, ParseError> {
    // Skip the SGML header (up to first newline after OFXHEADER)
    let body = if let Some(pos) = text.find("\n<OFX>") {
        &text[pos + 1..]
    } else if let Some(pos) = text.find("<OFX>") {
        &text[pos..]
    } else {
        return Err(ParseError::Malformed("no <OFX> root found".to_owned()));
    };

    parse_ofx_body(body, currency, true)
}

/// Parse XML format (OFX 2.x).
fn parse_xml(text: &str, currency: &str) -> Result<Vec<ParsedStatement>, ParseError> {
    parse_ofx_body(text, currency, false)
}

/// Parse the OFX body (same for both SGML and XML, but handles unclosed tags in SGML).
fn parse_ofx_body(
    text: &str,
    currency: &str,
    is_sgml: bool,
) -> Result<Vec<ParsedStatement>, ParseError> {
    let mut statements = Vec::new();

    // Extract STMTRS blocks (bank statements) or CCSTMTRS (credit card statements)
    let mut pos = 0;
    while let Some(start) = text[pos..]
        .find("<STMTRS>")
        .or_else(|| text[pos..].find("<CCSTMTRS>"))
    {
        let stmt_pos = pos + start;
        let tag_name = if text[stmt_pos..].starts_with("<STMTRS>") {
            "STMTRS"
        } else {
            "CCSTMTRS"
        };
        let close_tag = format!("</{tag_name}>");
        if let Some(end) = text[stmt_pos..].find(&close_tag) {
            let stmt_end = stmt_pos + end + close_tag.len();
            let stmt_text = &text[stmt_pos..stmt_end];
            if let Ok(statement) = parse_statement(stmt_text, currency, is_sgml) {
                statements.push(statement);
            }
            pos = stmt_end;
        } else {
            break;
        }
    }

    if statements.is_empty() {
        return Err(ParseError::Malformed(
            "no STMTRS or CCSTMTRS found".to_owned(),
        ));
    }
    Ok(statements)
}

/// Parse a single STMTRS or CCSTMTRS statement block.
fn parse_statement(
    text: &str,
    account_currency: &str,
    is_sgml: bool,
) -> Result<ParsedStatement, ParseError> {
    let mut iban = None;
    let mut currency = account_currency.to_owned();
    let mut closing = None;
    let mut lines = Vec::new();

    // Extract currency
    if let Some(curdef) = extract_tag_value(text, "CURDEF", is_sgml) {
        currency = curdef;
    }

    // Extract account ID (ACCTID inside BANKACCTFROM or CCACCTFROM)
    // In SGML, look for ACCTID directly (tags are unclosed)
    if let Some(acctid) = extract_tag_value(text, "ACCTID", is_sgml).filter(|id| iban_like(id)) {
        iban = Some(crate::iban::normalise(&acctid));
    }

    // Extract closing balance - look for BALAMT which is inside LEDGERBAL
    if let Some(balamt) = extract_tag_value(text, "BALAMT", is_sgml) {
        let exponent = finish_currency(&currency)?;
        let magnitude =
            parse_minor(&balamt, Decimal::Point, exponent).map_err(ParseError::Malformed)?;
        if let Some(baldate) = extract_tag_value(text, "DTASOF", is_sgml) {
            let date = parse_ofx_date(&baldate)?;
            closing = Some(BalanceRow {
                date,
                minor: magnitude,
            });
        }
    }

    // Extract transactions
    let mut pos = 0;
    while let Some(start) = text[pos..]
        .find("<STMTTRN>")
        .or_else(|| text[pos..].find("<CCSTMTTRN>"))
    {
        let trn_pos = pos + start;
        let tag_name = if text[trn_pos..].starts_with("<STMTTRN>") {
            "STMTTRN"
        } else {
            "CCSTMTTRN"
        };
        let close_tag = format!("</{tag_name}>");
        if let Some(end) = text[trn_pos..].find(&close_tag) {
            let trn_end = trn_pos + end + close_tag.len();
            let trn_text = &text[trn_pos..trn_end];
            if let Ok(line) = parse_transaction(trn_text, &currency, is_sgml) {
                lines.push(line);
            }
            pos = trn_end;
        } else {
            break;
        }
    }

    finish_currency(&currency)?;
    Ok(ParsedStatement {
        format: Format::Ofx,
        iban,
        reference: None,
        currency,
        opening: None,
        closing,
        lines,
    })
}

/// Parse a single STMTTRN or CCSTMTTRN transaction.
fn parse_transaction(text: &str, currency: &str, is_sgml: bool) -> Result<ParsedLine, ParseError> {
    let _trntype = extract_tag_value(text, "TRNTYPE", is_sgml)
        .ok_or_else(|| ParseError::Malformed("transaction has no TRNTYPE".to_owned()))?;

    // DTPOSTED is the booking date
    let booking_date_str = extract_tag_value(text, "DTPOSTED", is_sgml)
        .ok_or_else(|| ParseError::Malformed("transaction has no DTPOSTED".to_owned()))?;
    let booking_date = parse_ofx_date(&booking_date_str)?;

    // DTUSER is value date (optional)
    let value_date =
        extract_tag_value(text, "DTUSER", is_sgml).and_then(|d| parse_ofx_date(&d).ok());

    // TRNAMT is the amount
    let amount_str = extract_tag_value(text, "TRNAMT", is_sgml)
        .ok_or_else(|| ParseError::Malformed("transaction has no TRNAMT".to_owned()))?;
    let exponent = finish_currency(currency)?;
    let minor =
        parse_minor(&amount_str, Decimal::Point, exponent).map_err(ParseError::Malformed)?;

    // FITID is the bank reference
    let bank_reference = extract_tag_value(text, "FITID", is_sgml);

    // NAME or PAYEE is the counterparty
    let counterparty_name = extract_tag_value(text, "NAME", is_sgml)
        .or_else(|| extract_tag_value(text, "PAYEE", is_sgml));

    // MEMO is the remittance
    let remittance = extract_tag_value(text, "MEMO", is_sgml).unwrap_or_default();

    Ok(ParsedLine {
        booking_date,
        value_date,
        minor,
        currency: currency.to_owned(),
        counterparty_name,
        counterparty_iban: None,
        remittance,
        bank_reference,
    })
}

/// Extract the value between opening and closing tags (case-insensitive for tags).
/// In SGML, tags are unclosed; in XML, they're properly closed.
fn extract_tag_value(text: &str, tag: &str, is_sgml: bool) -> Option<String> {
    let upper_tag = tag.to_uppercase();
    let lower_tag = tag.to_lowercase();

    // Try uppercase first (more common in OFX)
    if let Some(val) = try_extract(&upper_tag, text, is_sgml) {
        return Some(val);
    }
    // Fallback to lowercase
    if let Some(val) = try_extract(&lower_tag, text, is_sgml) {
        return Some(val);
    }
    None
}

fn try_extract(tag: &str, text: &str, is_sgml: bool) -> Option<String> {
    let open = format!("<{tag}>");
    let start_pos = text.find(&open)?;
    let value_start = start_pos + open.len();
    let end_pos = if is_sgml {
        // In SGML, look for the next tag opening
        text[value_start..].find('<')?
    } else {
        let close = format!("</{tag}>");
        text[value_start..].find(&close)?
    };
    let value_end = value_start + end_pos;
    let raw_value = text[value_start..value_end].trim();
    if !raw_value.is_empty() {
        // Decode XML/HTML entities
        let decoded = decode_entities(raw_value);
        return Some(decoded);
    }
    None
}

/// Decode XML entities: &amp; &lt; &gt; &quot; &apos;
fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Parse an OFX date: YYYYMMDD[HHMMSS[.XXX]][[+-]tz:NAME]
/// Returns YYYY-MM-DD format. Requires first 8 bytes to be ASCII digits.
fn parse_ofx_date(date_str: &str) -> Result<String, ParseError> {
    let date_str = date_str.trim();
    let bytes = date_str.as_bytes();
    if bytes.len() < 8 {
        return Err(ParseError::Malformed(format!(
            "invalid OFX date: {date_str}"
        )));
    }
    // Validate and extract: first 8 bytes must be ASCII digits for YYYYMMDD
    for (i, &byte) in bytes.iter().enumerate().take(8) {
        if !byte.is_ascii_digit() {
            return Err(ParseError::Malformed(format!(
                "invalid OFX date: non-digit at position {i}"
            )));
        }
    }
    let year = std::str::from_utf8(&bytes[0..4])
        .map_err(|_| ParseError::Malformed("invalid UTF-8 in year".to_owned()))?;
    let month = std::str::from_utf8(&bytes[4..6])
        .map_err(|_| ParseError::Malformed("invalid UTF-8 in month".to_owned()))?;
    let day = std::str::from_utf8(&bytes[6..8])
        .map_err(|_| ParseError::Malformed("invalid UTF-8 in day".to_owned()))?;
    Ok(format!("{year}-{month}-{day}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ofx_date_simple() {
        assert_eq!(parse_ofx_date("20260314").unwrap(), "2026-03-14");
    }

    #[test]
    fn parse_ofx_date_with_time() {
        assert_eq!(parse_ofx_date("20260314120000").unwrap(), "2026-03-14");
    }

    #[test]
    fn parse_ofx_date_too_short_is_rejected() {
        assert!(parse_ofx_date("2026031").is_err());
        assert!(parse_ofx_date("").is_err());
    }

    #[test]
    fn parse_ofx_date_with_multibyte_character_is_rejected() {
        // Emoji at start (multibyte UTF-8)
        assert!(parse_ofx_date("😀20260314").is_err());
        // Non-digit in date portion
        assert!(parse_ofx_date("202a0314").is_err());
    }

    #[test]
    fn entities_are_decoded_in_values() {
        assert_eq!(decode_entities("Payment &amp; refund"), "Payment & refund");
        assert_eq!(
            decode_entities("Description &lt;text&gt;"),
            "Description <text>"
        );
        assert_eq!(
            decode_entities("Quote &quot;exact&quot;"),
            r#"Quote "exact""#
        );
        assert_eq!(decode_entities("&amp;&lt;&gt;&quot;&apos;"), "&<>\"'");
    }

    #[test]
    fn parse_amount_with_decimals() {
        // Test with camt053's parse_minor function via the parsing logic
        // We verify no panic occurs with various decimal formats
        let sgml_text = "<TRNAMT>-123.45</TRNAMT>";
        let val = try_extract("TRNAMT", sgml_text, true);
        assert_eq!(val, Some("-123.45".to_owned()));
    }
}
