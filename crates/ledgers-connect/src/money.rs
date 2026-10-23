//! Exact amounts from vendor data (issue #142). Vendors send decimal
//! strings or JSON numbers; they are parsed digit by digit into integers
//! and never through `f64` (docs/PLAN.md section 3, rule 1). Splits across
//! business units use the largest-remainder method, so the parts always sum
//! to the whole.

/// Why a vendor amount could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AmountError {
    /// The text is not a plain decimal number (no exponent, no separators).
    #[error("{0:?} is not a decimal amount")]
    NotDecimal(String),
    /// The amount does not fit in the 64-bit minor units the ledger stores.
    #[error("the amount does not fit in minor units")]
    Overflow,
}

/// Parses a plain decimal (`-12.345`, `0`, `7.`) into an integer count of
/// `10^-scale` units. Digits beyond `scale` are rounded half up, once, here.
///
/// # Errors
/// [`AmountError::NotDecimal`] for anything that is not `-?digits(.digits)?`,
/// [`AmountError::Overflow`] when the result exceeds `i64`.
pub fn parse_decimal(text: &str, scale: u32) -> Result<i128, AmountError> {
    let trimmed = text.trim();
    let not = || AmountError::NotDecimal(text.to_owned());
    let (negative, body) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed),
    };
    let (whole, fraction) = match body.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (body, ""),
    };
    if whole.is_empty() && fraction.is_empty() {
        return Err(not());
    }
    if !whole.bytes().all(|b| b.is_ascii_digit()) || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return Err(not());
    }
    let scale_usize = usize::try_from(scale).map_err(|_| AmountError::Overflow)?;
    let mut digits = String::with_capacity(whole.len() + scale_usize);
    digits.push_str(if whole.is_empty() { "0" } else { whole });
    let kept: String = fraction.chars().take(scale_usize).collect();
    digits.push_str(&kept);
    for _ in kept.len()..scale_usize {
        digits.push('0');
    }
    let round_up = fraction
        .as_bytes()
        .get(scale_usize)
        .is_some_and(|digit| *digit >= b'5');
    let mut value: i128 = digits.parse().map_err(|_| AmountError::Overflow)?;
    if round_up {
        value += 1;
    }
    if value > i128::from(i64::MAX) {
        return Err(AmountError::Overflow);
    }
    Ok(if negative { -value } else { value })
}

/// Reads a JSON value that holds an amount, a decimal string or a number.
///
/// # Errors
/// As [`parse_decimal`]; any other JSON type is [`AmountError::NotDecimal`].
pub fn amount_from_json(value: &serde_json::Value, scale: u32) -> Result<i128, AmountError> {
    match value {
        serde_json::Value::String(text) => parse_decimal(text, scale),
        serde_json::Value::Number(number) => parse_decimal(&number.to_string(), scale),
        other => Err(AmountError::NotDecimal(other.to_string())),
    }
}

/// Splits `total` across `weights` by largest remainder: each part is the
/// floor of its exact share, and the leftover units go to the largest
/// fractional remainders (ties by unit code). The parts sum to `total`
/// exactly. Zero weights get nothing. Returns `None` when every weight is
/// zero: the caller then books the whole amount to its default unit.
#[must_use]
pub fn split_by_weight(total: i64, weights: &[(String, u128)]) -> Option<Vec<(String, i64)>> {
    let sum: u128 = weights.iter().map(|(_, weight)| *weight).sum();
    if sum == 0 {
        return None;
    }
    let total_wide = i128::from(total);
    let mut parts: Vec<(String, i128, u128)> = weights
        .iter()
        .filter(|(_, weight)| *weight > 0)
        .map(|(unit, weight)| {
            let numerator = total_wide * i128::try_from(*weight).unwrap_or(i128::MAX);
            let share = numerator / i128::try_from(sum).unwrap_or(i128::MAX);
            let remainder = (numerator % i128::try_from(sum).unwrap_or(i128::MAX)).unsigned_abs();
            (unit.clone(), share, remainder)
        })
        .collect();
    let assigned: i128 = parts.iter().map(|(_, share, _)| *share).sum();
    let mut leftover = total_wide - assigned;
    parts.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    for part in &mut parts {
        if leftover == 0 {
            break;
        }
        part.1 += if leftover > 0 { 1 } else { -1 };
        leftover -= if leftover > 0 { 1 } else { -1 };
    }
    let mut out: Vec<(String, i64)> = parts
        .into_iter()
        .filter(|(_, share, _)| *share != 0)
        .map(|(unit, share, _)| (unit, i64::try_from(share).unwrap_or(0)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decimals_to_minor_units_without_floats() {
        assert_eq!(parse_decimal("12.34", 2).unwrap(), 1234);
        assert_eq!(parse_decimal("12", 2).unwrap(), 1200);
        assert_eq!(parse_decimal("0.5", 2).unwrap(), 50);
        assert_eq!(parse_decimal("-3.10", 2).unwrap(), -310);
        assert_eq!(parse_decimal(".7", 2).unwrap(), 70);
        assert_eq!(parse_decimal("12.345", 2).unwrap(), 1235, "half up, once");
        assert_eq!(parse_decimal("0.1", 2).unwrap(), 10);
        assert_eq!(parse_decimal("0.29", 2).unwrap(), 29);
        assert_eq!(
            parse_decimal("123.78912", 2).unwrap(),
            12379,
            "sub-cent digits round half up"
        );
    }

    #[test]
    fn refuses_what_is_not_plain_decimal() {
        for bad in ["", ".", "1e3", "1,000.00", "12.3.4", "abc", "--1", "NaN"] {
            assert!(
                matches!(parse_decimal(bad, 2), Err(AmountError::NotDecimal(_))),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn overflow_is_refused() {
        assert_eq!(
            parse_decimal("99999999999999999999", 2),
            Err(AmountError::Overflow)
        );
    }

    #[test]
    fn json_numbers_and_strings_both_read() {
        assert_eq!(amount_from_json(&serde_json::json!(12.5), 2).unwrap(), 1250);
        assert_eq!(
            amount_from_json(&serde_json::json!("12.50"), 2).unwrap(),
            1250
        );
        assert!(amount_from_json(&serde_json::json!(true), 2).is_err());
    }

    fn w(unit: &str, weight: u128) -> (String, u128) {
        (unit.to_owned(), weight)
    }

    #[test]
    fn split_sums_exactly_to_the_total() {
        let parts = split_by_weight(1000, &[w("FZ", 1), w("LED", 1), w("SQZ", 1)]).unwrap();
        let sum: i64 = parts.iter().map(|(_, amount)| amount).sum();
        assert_eq!(sum, 1000);
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn split_follows_weights_and_rounds_by_largest_remainder() {
        let parts = split_by_weight(10_00, &[w("A", 1), w("B", 2)]).unwrap();
        assert_eq!(parts, vec![("A".to_owned(), 333), ("B".to_owned(), 667)]);
    }

    #[test]
    fn split_handles_negative_totals() {
        let parts = split_by_weight(-100, &[w("A", 1), w("B", 1), w("C", 1)]).unwrap();
        let sum: i64 = parts.iter().map(|(_, amount)| amount).sum();
        assert_eq!(sum, -100);
    }

    #[test]
    fn zero_weights_get_nothing_and_all_zero_is_none() {
        let parts = split_by_weight(500, &[w("A", 0), w("B", 5)]).unwrap();
        assert_eq!(parts, vec![("B".to_owned(), 500)]);
        assert_eq!(split_by_weight(500, &[w("A", 0)]), None);
    }
}
