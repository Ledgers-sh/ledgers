//! Exact rates: FX and tax ratios as scaled integers.
//!
//! A [`Rate`] is an [`i128`] scaled by [`SCALE`] (10¹², twelve fractional
//! digits), per ADR `docs/adr/0003-fixed-point-rates.md`. Rates are only
//! ever ratios applied to [`Amount`]s (see
//! [`Amount::mul_rate`](crate::Amount::mul_rate)), never user-entered
//! general-purpose decimals, so twelve digits of precision and a ±1.7 ×
//! 10²⁶ range are ample.

use crate::error::MoneyError;
use std::fmt;
use std::str::FromStr;

/// The scale of a [`Rate`]: `scaled / SCALE` is the represented value.
/// 10¹² — twelve fractional digits.
pub const SCALE: i128 = 1_000_000_000_000;

/// An exact decimal ratio, stored as an [`i128`] scaled by [`SCALE`].
///
/// All arithmetic is checked and returns [`Result`]; nothing here can
/// panic. Values with more than twelve fractional digits cannot be
/// represented and are a parse error, not a silent rounding.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Rate {
    scaled: i128,
}

impl Rate {
    /// The multiplicative identity, `1.0`.
    pub const ONE: Self = Self {
        scaled: 1_000_000_000_000,
    };

    /// Zero.
    pub const ZERO: Self = Self { scaled: 0 };

    /// Build a rate from its scaled representation
    /// (`value = scaled / SCALE`).
    #[must_use]
    pub const fn from_scaled(scaled: i128) -> Self {
        Self { scaled }
    }

    /// The scaled representation (`value = scaled / SCALE`).
    #[must_use]
    pub const fn scaled(self) -> i128 {
        self.scaled
    }

    /// Checked addition.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] if the sum does not fit an [`i128`].
    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        self.scaled
            .checked_add(other.scaled)
            .map(Self::from_scaled)
            .ok_or(MoneyError::Overflow)
    }

    /// Checked subtraction.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] if the difference does not fit an [`i128`].
    pub fn checked_sub(self, other: Self) -> Result<Self, MoneyError> {
        self.scaled
            .checked_sub(other.scaled)
            .map(Self::from_scaled)
            .ok_or(MoneyError::Overflow)
    }

    /// Checked negation.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] for [`i128::MIN`].
    pub fn checked_neg(self) -> Result<Self, MoneyError> {
        self.scaled
            .checked_neg()
            .map(Self::from_scaled)
            .ok_or(MoneyError::Overflow)
    }

    /// Absolute value. Panics never; returns [`i128::MIN`] unchanged for
    /// itself (documented quirk of two's complement, matching the minimum
    /// rate's magnitude exceeding the maximum's).
    #[must_use]
    pub const fn abs(self) -> Self {
        Self {
            scaled: if self.scaled < 0 {
                self.scaled.wrapping_neg()
            } else {
                self.scaled
            },
        }
    }

    /// Parse `[+-]?digits[.digits]` with 1 to 12 fraction digits. No
    /// exponent notation, no separators, ASCII digits only; whitespace
    /// anywhere is an error. The scaled value must fit an [`i128`].
    ///
    /// # Errors
    /// [`MoneyError::ParseError`] for any malformed input;
    /// [`MoneyError::Overflow`] if the scaled value exceeds [`i128`].
    pub fn parse(input: &str) -> Result<Self, MoneyError> {
        let err = |reason: &'static str| MoneyError::ParseError {
            input: input.to_owned(),
            reason,
        };
        if input.is_empty() {
            return Err(err("empty input"));
        }
        let mut negative = false;
        let mut rest = input;
        if let Some(stripped) = rest.strip_prefix('+') {
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix('-') {
            negative = true;
            rest = stripped;
        }
        if rest.is_empty() {
            return Err(err("missing integer part"));
        }
        let mut int_digits: u128 = 0;
        let mut int_count = 0u32;
        let mut frac_digits: u128 = 0;
        let mut frac_count = 0u32;
        let mut seen_dot = false;
        for c in rest.chars() {
            if c == '.' {
                if seen_dot {
                    return Err(err("misplaced decimal separator"));
                }
                seen_dot = true;
            } else if let Some(d32) = c.to_digit(10) {
                let d = u128::from(d32);
                if seen_dot {
                    if frac_count >= 12 {
                        return Err(err("fraction longer than 12 digits"));
                    }
                    frac_digits = frac_digits * 10 + d;
                    frac_count += 1;
                } else {
                    int_digits = int_digits
                        .checked_mul(10)
                        .and_then(|v| v.checked_add(d))
                        .ok_or(MoneyError::Overflow)?;
                    int_count += 1;
                }
            } else {
                return Err(err("invalid digit"));
            }
        }
        if int_count == 0 {
            return Err(err("missing integer part"));
        }
        if seen_dot && frac_count == 0 {
            return Err(err("misplaced decimal separator"));
        }
        // Scale and combine as an unsigned magnitude — the same shape
        // `Display` formats — and apply the sign last, so a scaled
        // magnitude of 2^127 parses (with `-`) as i128::MIN and the
        // documented Display/parse round trip holds at the i128 bounds.
        let scaled_int = int_digits
            .checked_mul(SCALE.unsigned_abs())
            .ok_or(MoneyError::Overflow)?;
        let frac_scale = 10_u128
            .checked_pow(12 - frac_count)
            .ok_or(MoneyError::Overflow)?;
        let mag = scaled_int
            .checked_add(
                frac_digits
                    .checked_mul(frac_scale)
                    .ok_or(MoneyError::Overflow)?,
            )
            .ok_or(MoneyError::Overflow)?;
        let scaled = if negative && mag > 0 {
            // A magnitude of 2^127 is representable only as i128::MIN.
            // For any magnitude that fits at all, mag - 1 fits an i128
            // and -(mag - 1) - 1 == -mag without overflow.
            let m = i128::try_from(mag - 1).map_err(|_| MoneyError::Overflow)?;
            -m - 1
        } else {
            i128::try_from(mag).map_err(|_| MoneyError::Overflow)?
        };
        Ok(Self { scaled })
    }
}

impl FromStr for Rate {
    type Err = MoneyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for Rate {
    /// Minimal canonical form: integer part, then the fraction with
    /// trailing zeros stripped (omitted entirely when zero); a sign only
    /// when negative. `parse` inverts this exactly.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.scaled == 0 {
            return f.write_str("0");
        }
        let mag = self.scaled.unsigned_abs();
        let int_part = mag / SCALE.unsigned_abs();
        let frac = mag % SCALE.unsigned_abs();
        if self.scaled < 0 {
            f.write_str("-")?;
        }
        write!(f, "{int_part}")?;
        if frac == 0 {
            return Ok(());
        }
        // Twelve zero-padded fraction digits, trailing zeros stripped.
        let text = format!("{frac:0>12}").trim_end_matches('0').to_owned();
        write!(f, ".{text}")
    }
}

/// The named rounding modes for multiplying an amount by a [`Rate`]
/// ([`Amount::mul_rate`](crate::Amount::mul_rate)).
///
/// Rounding always acts on the **magnitude**; the sign is reapplied
/// afterwards, so every mode is symmetric around zero.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Rounding {
    /// Round to the nearest minor unit; exact ties (half of a minor unit)
    /// go to the even neighbour. The default for tax calculations.
    /// `2.5 → 2`, `3.5 → 4`, `-2.5 → -2`.
    #[default]
    HalfEven,
    /// Round to the nearest minor unit; exact ties go away from zero.
    /// `2.5 → 3`, `-2.5 → -3`.
    HalfUp,
    /// Truncate toward zero. `2.9 → 2`, `-2.9 → -2`.
    Down,
}

#[cfg(test)]
mod tests {
    use super::{Rate, Rounding, SCALE};
    use crate::error::MoneyError;

    fn parse_err(s: &str) -> MoneyError {
        Rate::parse(s).unwrap_err()
    }

    #[test]
    fn one_and_zero() {
        assert_eq!(Rate::ONE.scaled(), SCALE);
        assert_eq!(Rate::ZERO.scaled(), 0);
        assert_eq!(Rate::default(), Rate::ZERO);
        assert_eq!(Rounding::default(), Rounding::HalfEven);
    }

    #[test]
    fn parses_plain_and_fractional() {
        assert_eq!(Rate::parse("1").unwrap(), Rate::ONE);
        assert_eq!(Rate::parse("0").unwrap(), Rate::ZERO);
        assert_eq!(Rate::parse("0.5").unwrap().scaled(), 5 * SCALE / 10);
        assert_eq!(Rate::parse("-0.25").unwrap().scaled(), -(SCALE / 4));
        assert_eq!(Rate::parse("+2").unwrap().scaled(), 2 * SCALE);
        assert_eq!(
            Rate::parse("0.000000000001").unwrap().scaled(),
            1,
            "twelve fraction digits are exact"
        );
        assert_eq!(Rate::parse("1.5").unwrap(), "1.5".parse::<Rate>().unwrap());
    }

    #[test]
    fn parse_rejects_malformed() {
        assert_eq!(
            parse_err(""),
            MoneyError::ParseError {
                input: String::new(),
                reason: "empty input"
            }
        );
        assert!(matches!(
            parse_err("1e5"),
            MoneyError::ParseError {
                reason: "invalid digit",
                ..
            }
        ));
        assert!(matches!(
            parse_err("1 5"),
            MoneyError::ParseError {
                reason: "invalid digit",
                ..
            }
        ));
        assert!(matches!(
            parse_err("١٢"),
            MoneyError::ParseError {
                reason: "invalid digit",
                ..
            }
        ));
        assert!(matches!(
            parse_err("+-1"),
            MoneyError::ParseError {
                reason: "invalid digit",
                ..
            }
        ));
        assert!(matches!(
            parse_err(".5"),
            MoneyError::ParseError {
                reason: "missing integer part",
                ..
            }
        ));
        assert!(matches!(
            parse_err("-"),
            MoneyError::ParseError {
                reason: "missing integer part",
                ..
            }
        ));
        assert!(matches!(
            parse_err("1."),
            MoneyError::ParseError {
                reason: "misplaced decimal separator",
                ..
            }
        ));
        assert!(matches!(
            parse_err("1.2.3"),
            MoneyError::ParseError {
                reason: "misplaced decimal separator",
                ..
            }
        ));
        assert!(matches!(
            parse_err("0.0000000000001"),
            MoneyError::ParseError {
                reason: "fraction longer than 12 digits",
                ..
            }
        ));
        // Far beyond i128 when scaled.
        assert!(matches!(
            Rate::parse("170141183460469231731687303715884105728"),
            Err(MoneyError::Overflow)
        ));
    }

    #[test]
    fn display_is_minimal_and_round_trips() {
        assert_eq!(Rate::ZERO.to_string(), "0");
        assert_eq!(Rate::ONE.to_string(), "1");
        assert_eq!(Rate::from_scaled(5 * SCALE / 10).to_string(), "0.5");
        assert_eq!(Rate::from_scaled(SCALE / 4).to_string(), "0.25");
        assert_eq!(Rate::from_scaled(-1).to_string(), "-0.000000000001");
        assert_eq!(Rate::from_scaled(SCALE + 5).to_string(), "1.000000000005");
        for scaled in [
            0_i128,
            1,
            -1,
            42,
            SCALE,
            -SCALE,
            SCALE / 4,
            SCALE + 5,
            i128::from(i64::from(i32::MAX)),
        ] {
            let r = Rate::from_scaled(scaled);
            assert_eq!(Rate::parse(&r.to_string()).unwrap(), r, "round trip {r}");
        }
    }

    #[test]
    fn display_parse_round_trip_at_i128_bounds() {
        let min = Rate::from_scaled(i128::MIN);
        let max = Rate::from_scaled(i128::MAX);
        assert_eq!(min.to_string(), "-170141183460469231731687303.715884105728");
        assert_eq!(max.to_string(), "170141183460469231731687303.715884105727");
        assert_eq!(Rate::parse(&min.to_string()).unwrap(), min);
        assert_eq!(Rate::parse(&max.to_string()).unwrap(), max);
        // A scaled magnitude of 2^127 with no sign (or with '+') is
        // above i128::MAX; only the '-' form, i128::MIN, is
        // representable.
        let two_pow_127 = "170141183460469231731687303.715884105728";
        assert!(matches!(
            Rate::parse(two_pow_127),
            Err(MoneyError::Overflow)
        ));
        assert!(matches!(
            Rate::parse(&format!("+{two_pow_127}")),
            Err(MoneyError::Overflow)
        ));
        assert_eq!(Rate::parse(&format!("-{two_pow_127}")).unwrap(), min);
    }

    #[test]
    fn checked_ops() {
        let big = Rate::from_scaled(i128::MAX);
        assert_eq!(
            big.checked_add(Rate::ONE).unwrap_err(),
            MoneyError::Overflow
        );
        let small = Rate::from_scaled(i128::MIN);
        assert_eq!(
            small.checked_sub(Rate::ONE).unwrap_err(),
            MoneyError::Overflow
        );
        assert_eq!(small.checked_neg().unwrap_err(), MoneyError::Overflow);
        assert_eq!(
            Rate::from_scaled(3)
                .checked_add(Rate::from_scaled(4))
                .unwrap(),
            Rate::from_scaled(7)
        );
        assert_eq!(
            Rate::from_scaled(3)
                .checked_sub(Rate::from_scaled(4))
                .unwrap(),
            Rate::from_scaled(-1)
        );
        assert_eq!(
            Rate::from_scaled(-3).checked_neg().unwrap(),
            Rate::from_scaled(3)
        );
    }
}
