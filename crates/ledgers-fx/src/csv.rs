//! CSV import: strict parsing of `date,base,quote,rate` format.

use crate::error::FxError;
use crate::record::RateToRecord;
use ledgers_money::{Currency, Rate};
use std::collections::HashSet;
use time::Date;

/// Parse CSV: strict, rejects bad dates, unknown currencies, duplicates.
///
/// # Errors
/// [`FxError::CsvError`] or [`FxError::DuplicateKey`].
pub fn parse_csv(content: &str) -> Result<Vec<RateToRecord>, FxError> {
    let mut lines = content.lines().enumerate();
    let (_, header) = lines.next().ok_or_else(|| FxError::CsvError {
        line: 1,
        reason: "empty".to_owned(),
    })?;

    if header != "date,base,quote,rate" {
        return Err(FxError::CsvError {
            line: 1,
            reason: "expected header: date,base,quote,rate".to_owned(),
        });
    }

    let mut rates = Vec::new();
    let mut seen = HashSet::new();

    for (idx, line) in lines {
        let line_num = idx + 1;
        if line.trim().is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err(FxError::CsvError {
                line: line_num,
                reason: format!("expected 4 fields, got {}", parts.len()),
            });
        }

        let date = Date::parse(
            parts[0],
            &time::format_description::well_known::Iso8601::DEFAULT,
        )
        .map_err(|_| FxError::CsvError {
            line: line_num,
            reason: format!("bad date: {}", parts[0]),
        })?;

        let base = parts[1];
        let quote = parts[2];

        if Currency::from_code(base).is_err() {
            return Err(FxError::CsvError {
                line: line_num,
                reason: format!("unknown base: {base}"),
            });
        }
        if Currency::from_code(quote).is_err() {
            return Err(FxError::CsvError {
                line: line_num,
                reason: format!("unknown quote: {quote}"),
            });
        }
        if base == quote {
            return Err(FxError::CsvError {
                line: line_num,
                reason: "base == quote".to_owned(),
            });
        }

        let rate = Rate::parse(parts[3]).map_err(|_| FxError::CsvError {
            line: line_num,
            reason: format!("bad rate: {}", parts[3]),
        })?;

        let key = (parts[0], base, quote);
        if !seen.insert(key) {
            return Err(FxError::DuplicateKey {
                base: base.to_owned(),
                quote: quote.to_owned(),
                date: parts[0].to_owned(),
            });
        }

        rates.push(RateToRecord {
            base: base.to_owned(),
            quote: quote.to_owned(),
            date,
            rate,
        });
    }

    Ok(rates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csv_valid() {
        let csv = "date,base,quote,rate\n2026-10-20,EUR,USD,1.0812";
        let rates = parse_csv(csv).unwrap();
        assert_eq!(rates.len(), 1);
        assert_eq!(rates[0].base, "EUR");
    }

    #[test]
    fn test_csv_bad_header() {
        let csv = "date,base,rate\n2026-10-20,EUR,USD,1.0812";
        assert!(parse_csv(csv).is_err());
    }

    #[test]
    fn test_csv_duplicate() {
        let csv = "date,base,quote,rate\n2026-10-20,EUR,USD,1.0812\n2026-10-20,EUR,USD,1.0815";
        assert!(matches!(parse_csv(csv), Err(FxError::DuplicateKey { .. })));
    }
}
