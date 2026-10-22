//! Money text: parsing printed amounts into integer minor units, and the
//! currency exponent. No floats anywhere in this module.

/// The number of minor-unit digits of an ISO 4217 currency (EUR 2, JPY 0).
#[must_use]
pub fn exponent_of(code: &str) -> Option<u32> {
    let currency = ledgers_money::Currency::from_code(code).ok()?;
    let mut scale = currency.minor_per_major();
    let mut exponent = 0u32;
    while scale > 1 {
        scale /= 10;
        exponent += 1;
    }
    Some(exponent)
}

/// Parses a printed amount such as `1.234,56`, `1,234.56`, `1234,5` or `€ 99`
/// into minor units with `exponent` digits. The last separator followed by
/// exactly `exponent` digits is the decimal mark; the others are grouping.
#[must_use]
pub fn parse_minor(text: &str, exponent: u32) -> Option<i64> {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}')
        .filter(|c| !matches!(c, '€' | '$' | '£' | '¥'))
        .collect();
    let cleaned = cleaned.trim_start_matches(['-', '+']).to_owned();
    let negative = text.trim_start().starts_with('-');
    if cleaned.is_empty() || !cleaned.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let decimal_at = cleaned
        .char_indices()
        .rev()
        .find(|(index, c)| {
            (*c == '.' || *c == ',')
                && cleaned[index + 1..].len() == exponent as usize
                && cleaned[index + 1..].chars().all(|d| d.is_ascii_digit())
        })
        .map(|(index, _)| index);
    let (whole, fraction) = match decimal_at {
        Some(index) => (&cleaned[..index], &cleaned[index + 1..]),
        None => (cleaned.as_str(), ""),
    };
    // Without a decimal mark, every separator must be grouping: the first group
    // one to three digits, each later group exactly three. `1234,5` is ambiguous.
    if decimal_at.is_none() && whole.contains(['.', ',']) {
        let groups: Vec<&str> = whole.split(['.', ',']).collect();
        let grouped = groups.iter().enumerate().all(|(i, g)| {
            if i == 0 {
                (1..=3).contains(&g.len())
            } else {
                g.len() == 3
            }
        });
        if !grouped {
            return None;
        }
    }
    if !whole
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
        || !fraction.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let digits: String = whole.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() && fraction.is_empty() {
        return None;
    }
    let mut padded = fraction.to_owned();
    while padded.len() < exponent as usize {
        padded.push('0');
    }
    let text_value = format!(
        "{}{}",
        if digits.is_empty() { "0" } else { &digits },
        padded
    );
    let magnitude: i64 = text_value.parse().ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

/// Formats minor units as `1234.56` (dot decimal mark, no grouping).
#[must_use]
pub fn format_plain(minor: i64, exponent: u32) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let magnitude = minor.unsigned_abs().to_string();
    let exponent = exponent as usize;
    if exponent == 0 {
        return format!("{sign}{magnitude}");
    }
    let padded = format!("{magnitude:0>width$}", width = exponent + 1);
    let split = padded.len() - exponent;
    format!("{sign}{}.{}", &padded[..split], &padded[split..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_european_and_english_amounts() {
        assert_eq!(parse_minor("1.234,56", 2), Some(123_456));
        assert_eq!(parse_minor("1,234.56", 2), Some(123_456));
        assert_eq!(parse_minor("€ 99", 2), Some(9_900));
        assert_eq!(parse_minor("1234,5", 2), None);
        assert_eq!(parse_minor("19,00 €", 2), Some(1_900));
        assert_eq!(parse_minor("-0,50", 2), Some(-50));
        assert_eq!(parse_minor("¥ 1.500", 0), Some(1_500));
        assert_eq!(parse_minor("no amount", 2), None);
    }

    #[test]
    fn formats_minor_units_plainly() {
        assert_eq!(format_plain(123_456, 2), "1234.56");
        assert_eq!(format_plain(5, 2), "0.05");
        assert_eq!(format_plain(-1_900, 2), "-19.00");
        assert_eq!(format_plain(1_500, 0), "1500");
    }

    #[test]
    fn exponents_follow_the_currency() {
        assert_eq!(exponent_of("EUR"), Some(2));
        assert_eq!(exponent_of("JPY"), Some(0));
        assert_eq!(exponent_of("XXX-NOT-A-CODE"), None);
    }
}
