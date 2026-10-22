//! The rule-based extractor: fixed labels (English and German) over the PDF
//! text layer. It finds the header fields and the totals. It does not read
//! line items, so every draft it makes fails the line checks and needs a
//! person; that is the intended behaviour, not a gap to hide.

use crate::money::{exponent_of, parse_minor};
use crate::schema::{ExtractedInvoice, format_iso_date};
use crate::{DocumentInput, ExtractError, Extraction, Extractor, Source};
use time::{Date, Month};

/// Confidence given to a field found by its label.
const LABELLED: u8 = 70;
/// Confidence given to a guessed field (the first line as vendor name).
const GUESSED: u8 = 30;

/// The rule extractor (implementation 1 of issue #21).
pub struct RuleExtractor;

#[async_trait::async_trait]
impl Extractor for RuleExtractor {
    fn source(&self) -> Source {
        Source::Rules
    }

    async fn extract(&self, document: &DocumentInput<'_>) -> Result<Extraction, ExtractError> {
        let text = document.text_layer.ok_or_else(|| {
            ExtractError::NotReadable(
                "no text layer: this is a scan or an image, which needs the model extractor"
                    .to_owned(),
            )
        })?;
        Ok(Extraction {
            invoice: parse_text(text),
            source: Source::Rules,
        })
    }
}

/// Reads the header fields and totals from `text`.
#[must_use]
#[allow(clippy::too_many_lines, clippy::field_reassign_with_default)]
pub fn parse_text(text: &str) -> ExtractedInvoice {
    let lines: Vec<String> = text
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect();
    let mut invoice = ExtractedInvoice::default();

    invoice.invoice_number = after_label(
        &lines,
        &[
            "invoice number",
            "invoice no",
            "invoice #",
            "invoice nr",
            "invoice:",
            "rechnungsnummer",
            "rechnungs-nr",
            "rechnungsnr",
            "rechnung nr",
            "rechnung:",
        ],
    )
    .and_then(|rest| first_token(&rest))
    .filter(|token| token.chars().any(|c| c.is_ascii_digit()));
    invoice.invoice_date = after_label(
        &lines,
        &[
            "invoice date",
            "date of issue",
            "issue date",
            "rechnungsdatum",
            "datum:",
        ],
    )
    .and_then(|rest| date_token(&rest));
    invoice.due_date = after_label(
        &lines,
        &[
            "due date",
            "payment due",
            "due:",
            "zahlbar bis",
            "fällig am",
            "faellig am",
        ],
    )
    .and_then(|rest| date_token(&rest));
    invoice.vendor_vat_id = after_label(
        &lines,
        &[
            "vat id",
            "vat no",
            "vat number",
            "ust-idnr",
            "ust-id",
            "ustid",
            "tax id",
        ],
    )
    .and_then(|rest| {
        rest.split_whitespace()
            .map(|t| {
                t.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                    .to_owned()
            })
            .find(|t| is_vat_id(t))
    });
    invoice.vendor_iban = after_label(&lines, &["iban"]).and_then(|rest| iban_from(&rest));
    invoice.currency = currency_of(text);
    let exponent = invoice
        .currency
        .as_deref()
        .and_then(exponent_of)
        .unwrap_or(2);
    invoice.gross_minor = amount_after(
        &lines,
        &[
            "amount due",
            "grand total",
            "total due",
            "gesamtbetrag",
            "rechnungsbetrag",
            "total",
            "gesamt",
            "summe",
        ],
        &["sub", "net", "netto", "zwischen"],
        exponent,
    );
    invoice.net_minor = amount_after(
        &lines,
        &[
            "subtotal",
            "net total",
            "net amount",
            "zwischensumme",
            "netto",
        ],
        &[],
        exponent,
    );
    invoice.vat_minor = amount_after(
        &lines,
        &["vat", "mwst", "umsatzsteuer", "tax"],
        &["id", "no", "nr", "total", "gesamt"],
        exponent,
    );
    for (field, present) in [
        ("invoice_number", invoice.invoice_number.is_some()),
        ("invoice_date", invoice.invoice_date.is_some()),
        ("due_date", invoice.due_date.is_some()),
        ("vendor_vat_id", invoice.vendor_vat_id.is_some()),
        ("vendor_iban", invoice.vendor_iban.is_some()),
        ("gross_minor", invoice.gross_minor.is_some()),
        ("net_minor", invoice.net_minor.is_some()),
        ("vat_minor", invoice.vat_minor.is_some()),
    ] {
        if present {
            invoice.confidence.insert(field.to_owned(), LABELLED);
        }
    }
    invoice.vendor_name = lines
        .iter()
        .find(|line| line.chars().any(char::is_alphabetic) && line.len() <= 80)
        .cloned();
    if invoice.vendor_name.is_some() {
        invoice.confidence.insert("vendor_name".to_owned(), GUESSED);
    }
    invoice
}

/// The text after the first line that contains one of `labels` (ASCII
/// case-insensitive), with the label removed. Searches the whole text.
fn after_label(lines: &[String], labels: &[&str]) -> Option<String> {
    for line in lines {
        let lower = line.to_ascii_lowercase();
        for label in labels {
            if let Some(index) = lower.find(label) {
                let rest = &line[index + label.len()..];
                let rest = rest.trim_start_matches(|c: char| {
                    c == ':' || c == '#' || c == '.' || c == '-' || c.is_whitespace()
                });
                if !rest.is_empty() {
                    return Some(rest.to_owned());
                }
            }
        }
    }
    None
}

fn first_token(text: &str) -> Option<String> {
    text.split_whitespace()
        .next()
        .map(|t| t.trim_end_matches(['.', ',', ';', ':']).to_owned())
        .filter(|t| !t.is_empty())
}

/// The first date in `text`, as `YYYY-MM-DD`, in ISO, `DD.MM.YYYY` or `DD/MM/YYYY`.
fn date_token(text: &str) -> Option<String> {
    text.split_whitespace()
        .find_map(|token| {
            parse_date_token(
                token.trim_matches(|c: char| {
                    !c.is_ascii_digit() && c != '-' && c != '.' && c != '/'
                }),
            )
        })
        .map(format_iso_date)
}

fn parse_date_token(token: &str) -> Option<Date> {
    let separators = ['.', '/', '-'];
    let parts: Vec<&str> = token.split(|c| separators.contains(&c)).collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let (year, month, day) = if parts[0].len() == 4 {
        (parts[0], parts[1], parts[2])
    } else if parts[2].len() == 4 {
        (parts[2], parts[1], parts[0])
    } else {
        return None;
    };
    let year: i32 = year.parse().ok()?;
    let month = Month::try_from(month.parse::<u8>().ok()?).ok()?;
    Date::from_calendar_date(year, month, day.parse().ok()?).ok()
}

fn is_vat_id(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 10
        && bytes.len() <= 14
        && bytes[..2].iter().all(u8::is_ascii_uppercase)
        && bytes[2..]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && bytes[2..].iter().any(u8::is_ascii_digit)
}

fn iban_from(rest: &str) -> Option<String> {
    let joined: String = rest.chars().filter(|c| !c.is_whitespace()).collect();
    let candidate: String = joined
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    let bytes = candidate.as_bytes();
    let shaped = bytes.len() >= 15
        && bytes.len() <= 34
        && bytes[..2].iter().all(u8::is_ascii_uppercase)
        && bytes[2..4].iter().all(u8::is_ascii_digit);
    shaped.then_some(candidate)
}

fn currency_of(text: &str) -> Option<String> {
    for token in text.split(|c: char| !c.is_ascii_alphabetic()) {
        if matches!(token, "EUR" | "USD" | "GBP" | "CHF") {
            return Some(token.to_owned());
        }
    }
    if text.contains('€') {
        Some("EUR".to_owned())
    } else if text.contains('£') {
        Some("GBP".to_owned())
    } else if text.contains('$') {
        Some("USD".to_owned())
    } else {
        None
    }
}

/// The last amount on the first line that has one of `labels` and none of `skip`.
fn amount_after(lines: &[String], labels: &[&str], skip: &[&str], exponent: u32) -> Option<i64> {
    for label in labels {
        for line in lines {
            let lower = line.to_ascii_lowercase();
            if !lower.contains(label)
                || skip
                    .iter()
                    .any(|word| lower.contains(word) && !label.contains(word))
            {
                continue;
            }
            let amount = line
                .split_whitespace()
                .rev()
                .find_map(|token| parse_minor(token, exponent));
            if amount.is_some() {
                return amount;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVOICE: &str = "Halden Supplies GmbH\n\
        VAT ID: DE123456789\n\
        Invoice No: RE-2026-0042\n\
        Invoice date: 15.10.2026\n\
        Due date: 2026-11-14\n\
        IBAN DE89 3704 0044 0532 0130 00\n\
        Subtotal 1.000,00 EUR\n\
        VAT 19%: 190,00 EUR\n\
        Total 1.190,00 EUR\n";

    #[test]
    fn finds_the_header_fields_and_totals_with_their_labels() {
        let invoice = parse_text(INVOICE);
        assert_eq!(invoice.vendor_name.as_deref(), Some("Halden Supplies GmbH"));
        assert_eq!(invoice.vendor_vat_id.as_deref(), Some("DE123456789"));
        assert_eq!(invoice.invoice_number.as_deref(), Some("RE-2026-0042"));
        assert_eq!(invoice.invoice_date.as_deref(), Some("2026-10-15"));
        assert_eq!(invoice.due_date.as_deref(), Some("2026-11-14"));
        assert_eq!(
            invoice.vendor_iban.as_deref(),
            Some("DE89370400440532013000")
        );
        assert_eq!(invoice.currency.as_deref(), Some("EUR"));
        assert_eq!(invoice.net_minor, Some(100_000));
        assert_eq!(invoice.vat_minor, Some(19_000));
        assert_eq!(invoice.gross_minor, Some(119_000));
        assert_eq!(invoice.confidence.get("gross_minor"), Some(&LABELLED));
        assert!(invoice.lines.is_empty(), "the rules never read line items");
    }

    #[test]
    fn a_missing_field_stays_absent() {
        let invoice = parse_text("Just a letter with no invoice data at all");
        assert_eq!(invoice.invoice_number, None);
        assert_eq!(invoice.gross_minor, None);
        assert!(!invoice.confidence.contains_key("gross_minor"));
    }

    #[test]
    fn a_scan_without_a_text_layer_is_not_readable_by_rules() {
        let doc = DocumentInput {
            bytes: b"%PDF-1.7",
            mime: "application/pdf",
            text_layer: None,
        };
        let result = pollster::block_on(RuleExtractor.extract(&doc));
        assert!(matches!(result, Err(ExtractError::NotReadable(_))));
    }
}
