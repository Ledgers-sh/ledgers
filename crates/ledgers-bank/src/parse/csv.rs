//! Delimited text exports, read through a saved column mapping.
//!
//! A mapping names the column of each field (by header name, or `#n` for the
//! 1-based column) and says how dates and decimals are written. Quoted fields
//! may hold delimiters, line breaks and doubled quotes. A CSV has no balances,
//! so an import of one is recorded as unchecked, never as verified.

use crate::money::{Decimal, parse_minor};
use crate::statement::{Format, ParseError, ParsedLine, ParsedStatement};
use serde::{Deserialize, Serialize};

/// A column, by header name or as `#n` (1-based).
pub type ColumnRef = String;

/// The mapping a bank account saves for its CSV exports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CsvMapping {
    /// The field delimiter: `;`, `,` or a tab.
    pub delimiter: String,
    /// Lines before the header (or the first record when there is none).
    pub skip_rows: u32,
    /// Whether the first record after `skip_rows` names the columns.
    pub has_header: bool,
    /// The decimal separator: `.` or `,`.
    pub decimal: String,
    /// The date format: `YYYY-MM-DD`, `DD.MM.YYYY`, `DD/MM/YYYY` or `MM/DD/YYYY`.
    pub date_format: String,
    /// The booking date column.
    pub booking_date: ColumnRef,
    /// The value date column, when the export has one.
    pub value_date: Option<ColumnRef>,
    /// A single signed amount column (negative is an outflow).
    pub amount: Option<ColumnRef>,
    /// Outflows as positive numbers, when debits and credits are separate.
    pub debit: Option<ColumnRef>,
    /// Inflows as positive numbers, when debits and credits are separate.
    pub credit: Option<ColumnRef>,
    /// A currency column; without one the account's currency is used.
    pub currency: Option<ColumnRef>,
    /// The counterparty's name column.
    pub counterparty_name: Option<ColumnRef>,
    /// The counterparty's IBAN column.
    pub counterparty_iban: Option<ColumnRef>,
    /// Remittance columns, joined with a space in the order given.
    pub remittance: Vec<ColumnRef>,
    /// The bank's reference column.
    pub bank_reference: Option<ColumnRef>,
}

impl CsvMapping {
    /// Checks the mapping's own shape before a file is read with it.
    ///
    /// # Errors
    /// A message naming the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.delimiter.as_str(), ";" | "," | "\t") {
            return Err("the delimiter must be ';', ',' or a tab".to_owned());
        }
        if !matches!(self.decimal.as_str(), "." | ",") {
            return Err("the decimal separator must be '.' or ','".to_owned());
        }
        if !matches!(
            self.date_format.as_str(),
            "YYYY-MM-DD" | "DD.MM.YYYY" | "DD/MM/YYYY" | "MM/DD/YYYY"
        ) {
            return Err("the date format is not one of the supported formats".to_owned());
        }
        if self.booking_date.trim().is_empty() {
            return Err("the booking date column is required".to_owned());
        }
        match (&self.amount, &self.debit, &self.credit) {
            (Some(_), None, None) | (None, Some(_), Some(_)) => Ok(()),
            _ => Err("give either an amount column, or both debit and credit columns".to_owned()),
        }
    }
}

/// Splits CSV text into records of fields. Quoted fields may contain the
/// delimiter, line breaks and `""` for a quote. Blank lines are skipped.
#[must_use]
pub fn records(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.is_empty() {
            quoted = true;
        } else if c == delimiter {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            if row.iter().any(|f| !f.trim().is_empty()) {
                rows.push(std::mem::take(&mut row));
            } else {
                row.clear();
            }
        } else {
            field.push(c);
        }
    }
    row.push(field);
    if row.iter().any(|f| !f.trim().is_empty()) {
        rows.push(row);
    }
    rows
}

/// `DD.MM.YYYY` and the other supported forms to `YYYY-MM-DD`.
fn normalise_date(text: &str, format: &str) -> Result<String, ParseError> {
    let text = text.trim();
    let (year, month, day) = match format {
        "YYYY-MM-DD" => split3(text, '-', (0, 1, 2)),
        "DD.MM.YYYY" => split3(text, '.', (2, 1, 0)),
        "DD/MM/YYYY" => split3(text, '/', (2, 1, 0)),
        "MM/DD/YYYY" => split3(text, '/', (2, 0, 1)),
        other => {
            return Err(ParseError::Malformed(format!(
                "unknown date format {other}"
            )));
        }
    }
    .ok_or_else(|| ParseError::Malformed(format!("date {text:?} does not match {format}")))?;
    let month_n: u32 = month.parse().unwrap_or(0);
    let day_n: u32 = day.parse().unwrap_or(0);
    let year_ok = year.len() == 4 && year.chars().all(|c| c.is_ascii_digit());
    if !year_ok || !(1..=12).contains(&month_n) || !(1..=31).contains(&day_n) {
        return Err(ParseError::Malformed(format!(
            "date {text:?} is out of range"
        )));
    }
    Ok(format!("{year}-{month:0>2}-{day:0>2}"))
}

fn split3(text: &str, sep: char, order: (usize, usize, usize)) -> Option<(&str, &str, &str)> {
    let parts: Vec<&str> = text.split(sep).collect();
    if parts.len() != 3 {
        return None;
    }
    Some((parts[order.0], parts[order.1], parts[order.2]))
}

/// Resolves a column reference to an index.
fn column(header: Option<&[String]>, reference: &str) -> Result<usize, ParseError> {
    if let Some(number) = reference.strip_prefix('#') {
        let n: usize = number
            .parse()
            .map_err(|_| ParseError::Malformed(format!("column {reference} is not #n")))?;
        return n
            .checked_sub(1)
            .ok_or_else(|| ParseError::Malformed("columns start at #1".to_owned()));
    }
    let header = header.ok_or_else(|| {
        ParseError::Malformed(format!(
            "column {reference:?} needs a header row, or use #n"
        ))
    })?;
    header
        .iter()
        .position(|name| name.trim().eq_ignore_ascii_case(reference.trim()))
        .ok_or_else(|| ParseError::Malformed(format!("no column named {reference:?}")))
}

/// Reads a CSV file with a mapping.
///
/// # Errors
/// [`ParseError::Malformed`] for a mapping that does not fit the file, a
/// missing column, a bad date or amount, or a currency that is not one.
#[allow(clippy::too_many_lines)]
pub fn parse(
    text: &str,
    mapping: &CsvMapping,
    currency: &str,
) -> Result<ParsedStatement, ParseError> {
    mapping.validate().map_err(ParseError::Malformed)?;
    let delimiter = mapping.delimiter.chars().next().unwrap_or(';');
    let decimal = if mapping.decimal == "," {
        Decimal::Comma
    } else {
        Decimal::Point
    };
    let all = records(text, delimiter);
    let rows: Vec<Vec<String>> = all.into_iter().skip(mapping.skip_rows as usize).collect();
    let (header, body): (Option<Vec<String>>, &[Vec<String>]) = if mapping.has_header {
        let (first, rest) = rows
            .split_first()
            .ok_or_else(|| ParseError::Malformed("the file has no header row".to_owned()))?;
        (Some(first.clone()), rest)
    } else {
        (None, rows.as_slice())
    };
    let header_ref = header.as_deref();
    let booking = column(header_ref, &mapping.booking_date)?;
    let value_date = mapping
        .value_date
        .as_deref()
        .map(|r| column(header_ref, r))
        .transpose()?;
    let amount = mapping
        .amount
        .as_deref()
        .map(|r| column(header_ref, r))
        .transpose()?;
    let debit = mapping
        .debit
        .as_deref()
        .map(|r| column(header_ref, r))
        .transpose()?;
    let credit = mapping
        .credit
        .as_deref()
        .map(|r| column(header_ref, r))
        .transpose()?;
    let currency_col = mapping
        .currency
        .as_deref()
        .map(|r| column(header_ref, r))
        .transpose()?;
    let name_col = mapping
        .counterparty_name
        .as_deref()
        .map(|r| column(header_ref, r))
        .transpose()?;
    let iban_col = mapping
        .counterparty_iban
        .as_deref()
        .map(|r| column(header_ref, r))
        .transpose()?;
    let reference_col = mapping
        .bank_reference
        .as_deref()
        .map(|r| column(header_ref, r))
        .transpose()?;
    let remittance_cols = mapping
        .remittance
        .iter()
        .map(|r| column(header_ref, r))
        .collect::<Result<Vec<_>, _>>()?;

    let exponent = crate::parse::finish_currency(currency)?;
    let mut lines = Vec::with_capacity(body.len());
    let mut statement_currency = currency.to_owned();
    for (index, row) in body.iter().enumerate() {
        let at = |i: usize| row.get(i).map_or("", String::as_str);
        let line_no = index + 1;
        let booking_date = normalise_date(at(booking), &mapping.date_format)
            .map_err(|err| row_error(line_no, &err))?;
        let value = value_date
            .map(|i| normalise_date(at(i), &mapping.date_format))
            .transpose()
            .map_err(|err| row_error(line_no, &err))?;
        let minor = match (amount, debit, credit) {
            (Some(i), _, _) => parse_minor(at(i), decimal, exponent)
                .map_err(|e| row_error(line_no, &ParseError::Malformed(e)))?,
            (None, Some(d), Some(c)) => {
                // An empty debit or credit cell is zero: each row fills one side.
                let side = |cell: &str| -> Result<i64, ParseError> {
                    if cell.trim().is_empty() {
                        Ok(0)
                    } else {
                        parse_minor(cell, decimal, exponent)
                            .map_err(|e| row_error(line_no, &ParseError::Malformed(e)))
                    }
                };
                side(at(c))? - side(at(d))?
            }
            _ => {
                return Err(ParseError::Malformed(
                    "the mapping has no amount".to_owned(),
                ));
            }
        };
        let line_currency = match currency_col {
            Some(i) if !at(i).trim().is_empty() => at(i).trim().to_ascii_uppercase(),
            _ => currency.to_owned(),
        };
        if line_currency != statement_currency {
            return Err(row_error(
                line_no,
                &ParseError::Malformed(format!(
                    "currency {line_currency} differs from {statement_currency}"
                )),
            ));
        }
        statement_currency.clone_from(&line_currency);
        // Line breaks inside a quoted remittance become spaces.
        let remittance = remittance_cols
            .iter()
            .map(|i| at(*i))
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(ParsedLine {
            booking_date,
            value_date: value,
            minor,
            currency: line_currency,
            counterparty_name: name_col
                .map(|i| at(i).trim().to_owned())
                .filter(|s| !s.is_empty()),
            counterparty_iban: iban_col
                .map(at)
                .map(crate::iban::normalise)
                .filter(|s| !s.is_empty()),
            remittance,
            bank_reference: reference_col
                .map(|i| at(i).trim().to_owned())
                .filter(|s| !s.is_empty()),
        });
    }
    Ok(ParsedStatement {
        format: Format::Csv,
        iban: None,
        reference: None,
        currency: statement_currency,
        opening: None,
        closing: None,
        lines,
    })
}

fn row_error(line_no: usize, err: &ParseError) -> ParseError {
    ParseError::Malformed(format!("record {line_no}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> CsvMapping {
        CsvMapping {
            delimiter: ";".into(),
            skip_rows: 0,
            has_header: true,
            decimal: ",".into(),
            date_format: "DD.MM.YYYY".into(),
            booking_date: "Buchungstag".into(),
            value_date: Some("Valuta".into()),
            amount: Some("Betrag".into()),
            debit: None,
            credit: None,
            currency: None,
            counterparty_name: Some("Empfaenger".into()),
            counterparty_iban: None,
            remittance: vec!["Verwendungszweck".into()],
            bank_reference: None,
        }
    }

    #[test]
    fn quoted_fields_keep_delimiters_and_quotes() {
        let rows = records("a;\"b;c\";\"d \"\"e\"\"\"\n1;2;3\n", ';');
        assert_eq!(rows[0], vec!["a", "b;c", "d \"e\""]);
        assert_eq!(rows[1], vec!["1", "2", "3"]);
    }

    #[test]
    fn a_german_export_reads_with_its_mapping() {
        let text = "Buchungstag;Valuta;Empfaenger;Verwendungszweck;Betrag\n\
02.03.2026;02.03.2026;Halden Supplies GmbH;\"Rechnung 4711; Lieferung\";-1.234,50\n\
05.03.2026;05.03.2026;Acme GmbH;Erstattung;99,99\n";
        let s = parse(text, &mapping(), "EUR").expect("parses");
        assert_eq!(s.lines.len(), 2);
        assert_eq!(s.lines[0].minor, -123_450);
        assert_eq!(s.lines[0].booking_date, "2026-03-02");
        assert_eq!(s.lines[0].remittance, "Rechnung 4711; Lieferung");
        assert_eq!(s.lines[1].minor, 9_999);
        assert_eq!(s.lines[1].counterparty_name.as_deref(), Some("Acme GmbH"));
        assert!(s.opening.is_none() && s.closing.is_none());
    }

    #[test]
    fn debit_and_credit_columns_combine_into_a_signed_amount() {
        let mut m = mapping();
        m.amount = None;
        m.debit = Some("Soll".into());
        m.credit = Some("Haben".into());
        let text = "Buchungstag;Valuta;Empfaenger;Verwendungszweck;Soll;Haben\n\
01.03.2026;01.03.2026;X;Y;10,00;\n01.03.2026;01.03.2026;X;Y;;4,00\n";
        let s = parse(text, &m, "EUR").expect("parses");
        assert_eq!(s.lines[0].minor, -1_000);
        assert_eq!(s.lines[1].minor, 400);
    }

    #[test]
    fn a_bad_row_names_its_record() {
        let text = "Buchungstag;Valuta;Empfaenger;Verwendungszweck;Betrag\n\
31.02.2026;x;y;z;1,00\n";
        let err = parse(text, &mapping(), "EUR").unwrap_err();
        assert!(err.to_string().contains("record 1"), "{err}");
    }

    #[test]
    fn a_mapping_must_name_an_amount_form_and_a_date_format() {
        let mut m = mapping();
        m.amount = None;
        assert!(m.validate().is_err());
        let mut m = mapping();
        m.date_format = "YYYYMMDD".into();
        assert!(m.validate().is_err());
        assert!(mapping().validate().is_ok());
    }

    #[test]
    fn column_numbers_index_a_headerless_file() {
        let mut m = mapping();
        m.has_header = false;
        m.booking_date = "#1".into();
        m.value_date = None;
        m.amount = Some("#2".into());
        m.counterparty_name = None;
        m.remittance = vec!["#3".into()];
        let s = parse("02.03.2026;-5,00;Miete März\n", &m, "EUR").expect("parses");
        assert_eq!(s.lines[0].minor, -500);
        assert_eq!(s.lines[0].remittance, "Miete März");
    }
}
