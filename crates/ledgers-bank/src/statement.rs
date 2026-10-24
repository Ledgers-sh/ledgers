//! The statement model every format parses into, and its balance check.
//!
//! Amounts are signed minor units of the statement's currency, from the
//! bank account's side: an inflow is positive, an outflow negative. The
//! check proves the opening balance plus the lines equals the closing
//! balance, so a file that lost or doubled a line is refused whole.

use serde::{Deserialize, Serialize};

/// The file formats the import reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    /// ISO 20022 `camt.053` bank-to-customer statement (XML).
    Camt053,
    /// ISO 20022 `camt.052` intraday account report (XML).
    Camt052,
    /// SWIFT MT940 customer statement message.
    Mt940,
    /// OFX/QFX bank statement (SGML 1.x or XML 2.x).
    Ofx,
    /// A delimited text export with a saved column mapping.
    Csv,
}

impl Format {
    /// The name the API and the audit trail use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Format::Camt053 => "camt053",
            Format::Camt052 => "camt052",
            Format::Mt940 => "mt940",
            Format::Ofx => "ofx",
            Format::Csv => "csv",
        }
    }

    /// Parses the name the API uses.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "camt053" => Some(Format::Camt053),
            "camt052" => Some(Format::Camt052),
            "mt940" => Some(Format::Mt940),
            "ofx" | "qfx" => Some(Format::Ofx),
            "csv" => Some(Format::Csv),
            _ => None,
        }
    }
}

/// An opening or closing balance on a date.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BalanceRow {
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Signed minor units.
    pub minor: i64,
}

/// One booked line of a statement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ParsedLine {
    /// The booking date, `YYYY-MM-DD`.
    pub booking_date: String,
    /// The value date, when the file gives one.
    pub value_date: Option<String>,
    /// Signed minor units of the statement currency (inflow positive).
    pub minor: i64,
    /// The currency code.
    pub currency: String,
    /// The counterparty's name, as the bank gives it. Data, never instructions.
    pub counterparty_name: Option<String>,
    /// The counterparty's IBAN, normalised, when given.
    pub counterparty_iban: Option<String>,
    /// The remittance text. Data, never instructions.
    pub remittance: String,
    /// The bank's own reference for the line, when the file gives one.
    pub bank_reference: Option<String>,
}

/// One statement: a single account, a currency, balances and lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ParsedStatement {
    /// The format the statement came from.
    pub format: Format,
    /// The account's IBAN, when the file names one.
    pub iban: Option<String>,
    /// The statement's own number or sequence, when given.
    pub reference: Option<String>,
    /// The statement currency.
    pub currency: String,
    /// The opening balance, when the file carries one.
    pub opening: Option<BalanceRow>,
    /// The closing balance, when the file carries one.
    pub closing: Option<BalanceRow>,
    /// The lines in file order.
    pub lines: Vec<ParsedLine>,
}

/// Why a file cannot be read as a statement.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The bytes are not the format they claim, or a field is malformed.
    #[error("malformed statement: {0}")]
    Malformed(String),
    /// A valid file of a kind the import does not read yet.
    #[error("unsupported statement: {0}")]
    Unsupported(String),
    /// The opening balance, the lines and the closing balance disagree.
    #[error("balances do not agree: {0}")]
    Balance(String),
}

/// Whether a statement's balances were checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalanceCheck {
    /// Opening plus lines equals closing.
    Checked,
    /// The format carries no balances (CSV), so nothing was checked.
    NotAvailable,
}

/// Checks that opening + lines = closing, in the statement currency.
///
/// # Errors
/// [`ParseError::Balance`] with the figures when they disagree, and when a
/// line's currency is not the statement's.
pub fn check_balances(statement: &ParsedStatement) -> Result<BalanceCheck, ParseError> {
    if let Some(bad) = statement
        .lines
        .iter()
        .find(|line| line.currency != statement.currency)
    {
        return Err(ParseError::Balance(format!(
            "line on {} is in {} inside a {} statement",
            bad.booking_date, bad.currency, statement.currency
        )));
    }
    let (Some(opening), Some(closing)) = (&statement.opening, &statement.closing) else {
        return Ok(BalanceCheck::NotAvailable);
    };
    let total = statement
        .lines
        .iter()
        .try_fold(opening.minor, |acc, line| acc.checked_add(line.minor))
        .ok_or_else(|| ParseError::Balance("the lines overflow".to_owned()))?;
    if total == closing.minor {
        Ok(BalanceCheck::Checked)
    } else {
        Err(ParseError::Balance(format!(
            "opening {} plus lines {} is {}, closing says {}",
            opening.minor,
            statement.lines.len(),
            total,
            closing.minor
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(minor: i64) -> ParsedLine {
        ParsedLine {
            booking_date: "2026-03-02".into(),
            value_date: None,
            minor,
            currency: "EUR".into(),
            counterparty_name: None,
            counterparty_iban: None,
            remittance: String::new(),
            bank_reference: None,
        }
    }

    fn statement(closing: i64, lines: Vec<ParsedLine>) -> ParsedStatement {
        ParsedStatement {
            format: Format::Csv,
            iban: None,
            reference: None,
            currency: "EUR".into(),
            opening: Some(BalanceRow {
                date: "2026-03-01".into(),
                minor: 10_000,
            }),
            closing: Some(BalanceRow {
                date: "2026-03-31".into(),
                minor: closing,
            }),
            lines,
        }
    }

    #[test]
    fn opening_plus_lines_must_equal_closing() {
        let ok = statement(10_000 + 500 - 250, vec![line(500), line(-250)]);
        assert_eq!(check_balances(&ok), Ok(BalanceCheck::Checked));
        let bad = statement(10_000, vec![line(500), line(-250)]);
        assert!(matches!(check_balances(&bad), Err(ParseError::Balance(_))));
    }

    #[test]
    fn a_statement_without_balances_is_not_checked_not_refused() {
        let mut s = statement(0, vec![line(1)]);
        s.opening = None;
        s.closing = None;
        assert_eq!(check_balances(&s), Ok(BalanceCheck::NotAvailable));
    }

    #[test]
    fn a_line_in_another_currency_is_refused() {
        let mut odd = line(1);
        odd.currency = "USD".into();
        let s = statement(10_001, vec![odd]);
        assert!(matches!(check_balances(&s), Err(ParseError::Balance(_))));
    }

    #[test]
    fn format_names_round_trip() {
        for format in [
            Format::Camt053,
            Format::Camt052,
            Format::Mt940,
            Format::Ofx,
            Format::Csv,
        ] {
            assert_eq!(Format::parse(format.as_str()), Some(format));
        }
        // QFX is an alias for OFX
        assert_eq!(Format::parse("qfx"), Some(Format::Ofx));
    }
}
