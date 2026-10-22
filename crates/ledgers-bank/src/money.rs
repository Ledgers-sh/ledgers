//! Decimal text to signed minor units, without floats.
//!
//! The exponent comes from the currency (JPY 0, EUR 2, BHD 3). The text may
//! carry one sign, group separators in the integer part, and at most the
//! currency's exponent in fraction digits (trailing zeros beyond it are
//! allowed, anything else is refused rather than rounded).

/// The decimal separators a file may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decimal {
    /// `1234.56`.
    Point,
    /// `1234,56`.
    Comma,
}

/// Parses decimal text into minor units of a currency with `exponent`.
///
/// # Errors
/// A message naming the text when it is not a plain decimal amount, or does
/// not fit an `i64` once scaled.
pub fn parse_minor(text: &str, decimal: Decimal, exponent: u32) -> Result<i64, String> {
    let trimmed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let (negative, body) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(&trimmed)),
    };
    let separator = match decimal {
        Decimal::Point => '.',
        Decimal::Comma => ',',
    };
    let group = match decimal {
        Decimal::Point => ',',
        Decimal::Comma => '.',
    };
    let (int_part, frac_part) = match body.split_once(separator) {
        Some((int, frac)) => (int, Some(frac)),
        None => (body, None),
    };
    let int_digits: String = int_part
        .chars()
        .filter(|c| *c != group && *c != '\'')
        .collect();
    if int_digits.is_empty() || !int_digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("not an amount: {text:?}"));
    }
    let frac_digits = frac_part.unwrap_or("");
    if !frac_digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("not an amount: {text:?}"));
    }
    let exp = exponent as usize;
    let significant = frac_digits.trim_end_matches('0');
    if significant.len() > exp {
        return Err(format!("too many decimals for the currency: {text:?}"));
    }
    let mut digits = int_digits;
    // Digits beyond the exponent are zeros (checked above): keep the first
    // `exp`, and pad a short fraction up to it, so the scaled value is exact.
    digits.push_str(&frac_digits[..exp.min(frac_digits.len())]);
    let pad = exp.saturating_sub(frac_digits.len());
    digits.extend(std::iter::repeat_n('0', pad));
    let magnitude: i64 = digits
        .trim_start_matches('0')
        .parse::<i64>()
        .or_else(|err| {
            if digits.chars().all(|c| c == '0') {
                Ok(0)
            } else {
                Err(err)
            }
        })
        .map_err(|_| format!("amount out of range: {text:?}"))?;
    Ok(if negative { -magnitude } else { magnitude })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_and_comma_decimals_scale_by_the_exponent() {
        assert_eq!(parse_minor("1234.56", Decimal::Point, 2), Ok(123_456));
        assert_eq!(parse_minor("-0,5", Decimal::Comma, 2), Ok(-50));
        assert_eq!(parse_minor("1.234,50", Decimal::Comma, 2), Ok(123_450));
        assert_eq!(parse_minor("1,234.50", Decimal::Point, 2), Ok(123_450));
        assert_eq!(parse_minor("12", Decimal::Point, 0), Ok(12));
        assert_eq!(parse_minor("3.500", Decimal::Point, 3), Ok(3_500));
    }

    #[test]
    fn extra_decimals_are_refused_not_rounded() {
        assert!(parse_minor("1.005", Decimal::Point, 2).is_err());
        assert_eq!(parse_minor("1.500", Decimal::Point, 2), Ok(150));
    }

    #[test]
    fn non_amounts_are_refused() {
        for bad in ["", "-", "1e3", "12.3.4", "abc", "1..0", "--5"] {
            assert!(parse_minor(bad, Decimal::Point, 2).is_err(), "{bad}");
        }
    }

    #[test]
    fn out_of_range_is_refused() {
        assert!(parse_minor("99999999999999999999", Decimal::Point, 2).is_err());
    }
}
