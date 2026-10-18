//! [`Amount`]: an exact quantity of one currency, in integer minor units.

use crate::currency::Currency;
use crate::error::MoneyError;
use crate::rate::{Rate, Rounding, SCALE};
use crate::style::DecimalStyle;
use std::cmp::Ordering;
use std::fmt;

/// Decimal separators and space variants across the styles this crate
/// knows; anything from this set in the wrong place parses as a
/// `"misplaced group separator"` (or `"misplaced decimal separator"`)
/// rather than an invalid digit, which keeps the diagnostics honest.
const KNOWN_SEPARATORS: [char; 6] = ['.', ',', '\'', ' ', '\u{a0}', '\u{202f}'];

/// U+2066 LEFT-TO-RIGHT ISOLATE, the opening wrapper of
/// [`Amount::format_grouped_isolated`].
const LRI: char = '\u{2066}';
/// U+2069 POP DIRECTIONAL ISOLATE, the closing wrapper.
const PDI: char = '\u{2069}';

/// An exact amount of one [`Currency`], held as integer minor units in an
/// [`i128`] (JPY 0, EUR 2, BHD 3, CLF 4 — see [`Currency::exponent`]).
///
/// There are no `std::ops` implementations: every arithmetic call goes
/// through a `checked_*` method returning `Result`, and combining two
/// currencies without converting first is [`MoneyError::CurrencyMismatch`],
/// never an implicit conversion. Money is never a float anywhere in this
/// crate.
///
/// [`Ord`] orders by currency first (by code), then minor units — a total
/// order across currencies, but amounts of different currencies only
/// compare meaningfully after conversion.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Amount {
    minor: i128,
    currency: Currency,
}

impl Amount {
    /// Wrap an existing minor-unit value. Infallible: no arithmetic
    /// happens. Use [`Amount::from_major`] for major-unit input.
    #[must_use]
    pub const fn new(minor: i128, currency: Currency) -> Self {
        Self { minor, currency }
    }

    /// Major units to minor units, checked.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] if `major * minor_per_major` does not fit
    /// an [`i128`].
    pub fn from_major(major: i128, currency: Currency) -> Result<Self, MoneyError> {
        let minor = major
            .checked_mul(currency.minor_per_major())
            .ok_or(MoneyError::Overflow)?;
        Ok(Self { minor, currency })
    }

    /// The minor-unit value, in the currency's own exponent.
    #[must_use]
    pub const fn minor(self) -> i128 {
        self.minor
    }

    /// The currency.
    #[must_use]
    pub const fn currency(self) -> Currency {
        self.currency
    }

    /// Checked addition; both operands must share one currency.
    ///
    /// # Errors
    /// [`MoneyError::CurrencyMismatch`] if the currencies differ;
    /// [`MoneyError::Overflow`] if the sum does not fit an [`i128`].
    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        self.assert_same_currency(other)?;
        let minor = self
            .minor
            .checked_add(other.minor)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Checked subtraction; both operands must share one currency.
    ///
    /// # Errors
    /// [`MoneyError::CurrencyMismatch`] if the currencies differ;
    /// [`MoneyError::Overflow`] if the difference does not fit an [`i128`].
    pub fn checked_sub(self, other: Self) -> Result<Self, MoneyError> {
        self.assert_same_currency(other)?;
        let minor = self
            .minor
            .checked_sub(other.minor)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Checked negation.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] for [`i128::MIN`] minor units.
    pub fn checked_neg(self) -> Result<Self, MoneyError> {
        let minor = self.minor.checked_neg().ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Checked absolute value.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] for [`i128::MIN`] minor units.
    pub fn checked_abs(self) -> Result<Self, MoneyError> {
        let minor = self.minor.checked_abs().ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Multiply by an integer quantity, checked. For a fractional factor
    /// (an FX or tax rate) use [`Amount::mul_rate`].
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] if the product does not fit an [`i128`].
    pub fn checked_mul(self, qty: i64) -> Result<Self, MoneyError> {
        let minor = self
            .minor
            .checked_mul(i128::from(qty))
            .ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Multiply by an exact [`Rate`] with a named [`Rounding`] mode.
    ///
    /// The intermediate product `minor × rate.scaled()` is `checked_mul`'d:
    /// with minor units up to [`i64::MAX`] in magnitude and rates up to
    /// 10⁶ it fits an [`i128`] with orders of magnitude to spare; beyond
    /// that bound the result is [`MoneyError::Overflow`], never a wrapped
    /// value. Rounding acts on the magnitude, then the sign is reapplied,
    /// so `-2.5` rounds exactly like `2.5` with a minus.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] if the scaled product does not fit an
    /// [`i128`].
    pub fn mul_rate(self, rate: Rate, mode: Rounding) -> Result<Self, MoneyError> {
        let product = self
            .minor
            .checked_mul(rate.scaled())
            .ok_or(MoneyError::Overflow)?;
        let negative = product < 0;
        let mag = product.unsigned_abs();
        let scale = SCALE.unsigned_abs();
        let quotient = mag / scale;
        let remainder = mag % scale;
        let double = remainder.checked_mul(2).ok_or(MoneyError::Overflow)?;
        let bump = match mode {
            Rounding::Down => false,
            Rounding::HalfUp => double >= scale,
            Rounding::HalfEven => double > scale || (double == scale && quotient % 2 == 1),
        };
        let rounded = quotient + u128::from(bump);
        let minor = i128::try_from(rounded).map_err(|_| MoneyError::Overflow)?;
        Ok(Self {
            minor: if negative { -minor } else { minor },
            currency: self.currency,
        })
    }

    /// True when the amount is exactly zero (of any currency).
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.minor == 0
    }

    /// True for amounts greater than zero.
    #[must_use]
    pub const fn is_positive(self) -> bool {
        self.minor > 0
    }

    /// True for amounts less than zero.
    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.minor < 0
    }

    /// The sign as `-1`, `0` or `1` — a plain [`i8`] so it composes with
    /// integer arithmetic without re-matching an [`Ordering`].
    #[must_use]
    pub const fn signum(self) -> i8 {
        if self.minor < 0 {
            -1
        } else if self.minor > 0 {
            1
        } else {
            0
        }
    }

    /// Narrow to the storage boundary: the database column is `INTEGER`
    /// (`i64`). Round-trips through [`Amount::from_i64`].
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] if the minor units do not fit an [`i64`].
    pub fn to_i64(self) -> Result<i64, MoneyError> {
        i64::try_from(self.minor).map_err(|_| MoneyError::Overflow)
    }

    /// Widen from the storage boundary. Infallible: every [`i64`] is a
    /// valid [`i128`] minor-unit value.
    #[must_use]
    pub fn from_i64(minor: i64, currency: Currency) -> Self {
        Self {
            minor: i128::from(minor),
            currency,
        }
    }

    /// The interchange form: ASCII digits, `.` decimal point, exactly
    /// [`Currency::exponent`] fraction digits (none when the exponent is
    /// 0), a leading `-` when negative. EUR `123456` → `"1234.56"`, JPY
    /// `1234` → `"1234"`, KWD `1234` → `"1.234"`. Minor units are a
    /// signed integer, so a `-0` form cannot arise. Inverted by
    /// [`Amount::parse_canonical`].
    #[must_use]
    pub fn to_canonical_string(self) -> String {
        self.format_core(None)
    }

    /// Parse the interchange form produced by
    /// [`Amount::to_canonical_string`]: optional `+`/`-`, ASCII digits,
    /// `.` decimal point, at most [`Currency::exponent`] fraction digits
    /// (fewer pads right: `"1.5"` EUR is 150 minor). Any group separator
    /// is rejected — use [`Amount::parse_locale`] for human input.
    ///
    /// # Errors
    /// [`MoneyError::ParseError`] for malformed input (whitespace
    /// anywhere, unicode digits, misplaced separators, a fraction longer
    /// than the currency's exponent); [`MoneyError::Overflow`] if the
    /// scaled value does not fit an [`i128`].
    pub fn parse_canonical(s: &str, currency: Currency) -> Result<Self, MoneyError> {
        let parsed = split_number(s, DecimalStyle::CANONICAL)?;
        build_amount(&parsed, s, currency)
    }

    /// Parse human input in the given [`DecimalStyle`]: the style's
    /// decimal separator, optional group separators matching its grouping
    /// pattern (runs read from the decimal point outward must match the
    /// pattern exactly, its last element repeating; the leading run may
    /// be shorter; ungrouped input is always accepted), at most
    /// [`Currency::exponent`] fraction digits. Whitespace is rejected
    /// except where the style lists it as a group separator (`FR_FR` lists
    /// three space variants). See [`Amount::parse_canonical`] for the
    /// shared digit rules.
    ///
    /// # Errors
    /// [`MoneyError::ParseError`] for malformed input;
    /// [`MoneyError::Overflow`] if the scaled value does not fit an
    /// [`i128`].
    pub fn parse_locale(
        s: &str,
        currency: Currency,
        style: DecimalStyle,
    ) -> Result<Self, MoneyError> {
        let parsed = split_number(s, style)?;
        build_amount(&parsed, s, currency)
    }

    /// Format for humans: the style's decimal separator and group
    /// separator (element 0 of its `group_seps`) with its grouping
    /// pattern, fraction always the full exponent width.
    #[must_use]
    pub fn format_grouped(self, style: DecimalStyle) -> String {
        self.format_core(Some(style))
    }

    /// Like [`Amount::format_grouped`], wrapped in U+2066 (LEFT-TO-RIGHT
    /// ISOLATE) … U+2069 (POP DIRECTIONAL ISOLATE).
    ///
    /// RTL safety: the digits and separators this crate emits contain no
    /// bidi characters of their own, but a bare Western number embedded
    /// in RTL text can be visually reordered by the bidi algorithm. The
    /// isolation wrapper pins the whole amount to one left-to-right run
    /// so it renders as a unit. Parse functions do **not** accept the
    /// wrapper characters.
    #[must_use]
    pub fn format_grouped_isolated(self, style: DecimalStyle) -> String {
        let mut s = String::new();
        s.push(LRI);
        s.push_str(&self.format_grouped(style));
        s.push(PDI);
        s
    }

    fn assert_same_currency(self, other: Self) -> Result<(), MoneyError> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(MoneyError::CurrencyMismatch {
                expected: self.currency,
                actual: other.currency,
            })
        }
    }

    /// Shared formatting core: `None` style means canonical (no
    /// grouping, `.` decimal point).
    fn format_core(self, style: Option<DecimalStyle>) -> String {
        let negative = self.minor < 0;
        let mag = self.minor.unsigned_abs();
        // Digits least-significant first.
        let mut digits: Vec<u8> = Vec::new();
        let mut rest = mag;
        loop {
            digits.push(u8::try_from(rest % 10).unwrap_or(0));
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        let exp = self.currency.exponent() as usize;
        let frac_src = &digits[..digits.len().min(exp)];
        let int_src: &[u8] = if digits.len() > exp {
            &digits[exp..]
        } else {
            &[]
        };
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        if int_src.is_empty() {
            out.push('0');
        } else {
            let (groups, sep) = match style {
                Some(st) if !st.groups().is_empty() => (st.groups(), Some(st.group_seps()[0])),
                _ => (&[] as &[u8], None),
            };
            // Chunk the integer digits (LSD-first) into runs read from
            // the decimal point outward; the pattern's last element
            // repeats. An empty pattern (canonical) emits the digits
            // ungrouped.
            if groups.is_empty() {
                for &d in int_src.iter().rev() {
                    out.push(char::from(b'0' + d));
                }
            } else {
                let mut runs: Vec<&[u8]> = Vec::new();
                let mut idx = 0usize;
                let mut gi = 0usize;
                while idx < int_src.len() {
                    let take = usize::from(groups[gi.min(groups.len() - 1)]);
                    let end = (idx + take).min(int_src.len());
                    runs.push(&int_src[idx..end]);
                    idx = end;
                    gi += 1;
                }
                // Emit most-significant run first, digits MSD-first
                // within each run, separators between runs.
                for (ri, run) in runs.iter().enumerate().rev() {
                    if ri != runs.len() - 1
                        && let Some(sep) = sep
                    {
                        out.push(sep);
                    }
                    for &d in run.iter().rev() {
                        out.push(char::from(b'0' + d));
                    }
                }
            }
        }
        if exp > 0 {
            out.push(match style {
                Some(st) => st.decimal_sep(),
                None => '.',
            });
            for i in (0..exp).rev() {
                let d = frac_src.get(i).copied().unwrap_or(0);
                out.push(char::from(b'0' + d));
            }
        }
        out
    }
}

impl fmt::Display for Amount {
    /// The canonical string plus the currency code: `"1234.56 EUR"`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.to_canonical_string(), self.currency.code())
    }
}

impl PartialOrd for Amount {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Amount {
    /// Currency first (by code), then minor units — see the type docs.
    fn cmp(&self, other: &Self) -> Ordering {
        self.currency
            .cmp(&other.currency)
            .then(self.minor.cmp(&other.minor))
    }
}

/// A decimal number split into validated digit runs: separators checked,
/// grouping checked, sign stripped.
struct ParsedNumber {
    negative: bool,
    /// ASCII digit values, most significant first.
    int_digits: Vec<u8>,
    /// ASCII digit values.
    frac_digits: Vec<u8>,
}

/// The one digit-parsing routine shared by the canonical and locale
/// paths, so they cannot diverge. Rules: optional single leading `+`/`-`;
/// ASCII digits only (unicode digits and whitespace are `"invalid
/// digit"`); at most one decimal separator, with at least one digit on
/// each side; group separators only between digits, never adjacent to the
/// decimal separator or either end, no empty groups, and grouped runs
/// must match the style's pattern (runs read from the decimal point
/// outward match exactly, the last element repeating; the leading run may
/// be shorter).
fn split_number(input: &str, style: DecimalStyle) -> Result<ParsedNumber, MoneyError> {
    let err = |reason: &'static str| MoneyError::ParseError {
        input: input.to_owned(),
        reason,
    };
    let mut negative = false;
    let mut rest = input;
    if let Some(stripped) = rest.strip_prefix('+') {
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix('-') {
        negative = true;
        rest = stripped;
    }
    if rest.is_empty() {
        return Err(err("empty input"));
    }
    let mut int_digits = Vec::new();
    let mut int_tokens: Vec<char> = Vec::new(); // digits and group separators, in order
    let mut frac_digits = Vec::new();
    let mut seen_decimal = false;
    for c in rest.chars() {
        if c == style.decimal_sep() {
            if seen_decimal {
                return Err(err("misplaced decimal separator"));
            }
            seen_decimal = true;
        } else if style.group_seps().contains(&c) {
            if seen_decimal || style.groups().is_empty() {
                return Err(err("misplaced group separator"));
            }
            int_tokens.push(c);
        } else if let Some(d) = c.to_digit(10) {
            let d = u8::try_from(d).unwrap_or(0);
            if seen_decimal {
                frac_digits.push(d);
            } else {
                int_digits.push(d);
                int_tokens.push(c);
            }
        } else if KNOWN_SEPARATORS.contains(&c) {
            return Err(err("misplaced group separator"));
        } else {
            return Err(err("invalid digit"));
        }
    }
    if int_digits.is_empty() {
        return Err(err("missing integer part"));
    }
    if seen_decimal && frac_digits.is_empty() {
        return Err(err("misplaced decimal separator"));
    }
    // Grouping: validate runs against the pattern, read from the decimal
    // point outward. With no group separator present, any length is fine.
    if int_tokens.iter().any(|&t| style.group_seps().contains(&t)) {
        let mut runs: Vec<String> = Vec::new();
        let mut current = String::new();
        for t in int_tokens {
            if style.group_seps().contains(&t) {
                runs.push(current.clone());
                current.clear();
            } else {
                current.push(t);
            }
        }
        runs.push(current);
        let groups = style.groups();
        let k = runs.len() - 1; // index of the run nearest the decimal point
        for (pos, run) in runs.iter().enumerate() {
            let j = k - pos; // distance of this run from the decimal point
            let required = usize::from(groups[j.min(groups.len() - 1)]);
            if run.is_empty()
                || (pos == 0 && run.len() > required)
                || (pos > 0 && run.len() != required)
            {
                return Err(err("misplaced group separator"));
            }
        }
    }
    Ok(ParsedNumber {
        negative,
        int_digits,
        frac_digits,
    })
}

/// Scale validated digit runs into minor units, checked. The magnitude
/// is accumulated in [`u128`] — mirroring `format_core`'s
/// `unsigned_abs` — so that a magnitude of 2^127 parses as
/// [`i128::MIN`] when a sign is present and the documented inversion
/// of [`Amount::to_canonical_string`] holds across the whole [`i128`]
/// range.
fn build_amount(
    parsed: &ParsedNumber,
    input: &str,
    currency: Currency,
) -> Result<Amount, MoneyError> {
    let err = |reason: &'static str| MoneyError::ParseError {
        input: input.to_owned(),
        reason,
    };
    let exp = currency.exponent() as usize;
    if parsed.frac_digits.len() > exp {
        return Err(err("fraction longer than currency exponent"));
    }
    let mut value: u128 = 0;
    for &d in &parsed.int_digits {
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(u128::from(d)))
            .ok_or(MoneyError::Overflow)?;
    }
    value = value
        .checked_mul(currency.minor_per_major().unsigned_abs())
        .ok_or(MoneyError::Overflow)?;
    let frac_scale = 10_u128
        .checked_pow(
            u32::try_from(exp - parsed.frac_digits.len()).map_err(|_| MoneyError::Overflow)?,
        )
        .ok_or(MoneyError::Overflow)?;
    let mut frac: u128 = 0;
    for &d in &parsed.frac_digits {
        frac = frac
            .checked_mul(10)
            .and_then(|v| v.checked_add(u128::from(d)))
            .ok_or(MoneyError::Overflow)?;
    }
    value = value
        .checked_add(frac.checked_mul(frac_scale).ok_or(MoneyError::Overflow)?)
        .ok_or(MoneyError::Overflow)?;
    // Apply the sign to the magnitude last. A magnitude of 2^127 is
    // representable only as i128::MIN: with a negative sign and a
    // nonzero magnitude, step down by one so the narrowing fits, then
    // rebuild -mag as -(mag - 1) - 1, which cannot overflow. Without a
    // sign, 2^127 exceeds i128::MAX and is rejected.
    let minor = if parsed.negative && value > 0 {
        let m = i128::try_from(value - 1).map_err(|_| MoneyError::Overflow)?;
        -m - 1
    } else {
        i128::try_from(value).map_err(|_| MoneyError::Overflow)?
    };
    Ok(Amount { minor, currency })
}

#[cfg(test)]
mod tests {
    use super::{Amount, DecimalStyle};
    use crate::currency::Currency;
    use crate::error::MoneyError;
    use crate::rate::{Rate, Rounding, SCALE};

    fn eur() -> Currency {
        Currency::from_code("EUR").unwrap()
    }
    fn jpy() -> Currency {
        Currency::from_code("JPY").unwrap()
    }
    fn kwd() -> Currency {
        Currency::from_code("KWD").unwrap()
    }
    fn e(minor: i128) -> Amount {
        Amount::new(minor, eur())
    }

    #[test]
    fn from_major_and_accessors() {
        assert_eq!(Amount::from_major(1234, eur()).unwrap().minor(), 123_400);
        assert_eq!(Amount::from_major(1, kwd()).unwrap().minor(), 1000);
        assert_eq!(Amount::from_major(10, jpy()).unwrap().minor(), 10);
        assert!(matches!(
            Amount::from_major(i128::MAX, kwd()),
            Err(MoneyError::Overflow)
        ));
        let a = Amount::from_major(1, eur()).unwrap();
        assert_eq!(a.currency(), eur());
        assert_eq!(a.minor(), 100);
    }

    #[test]
    fn arithmetic_checks_currency_and_overflow() {
        let usd = Currency::from_code("USD").unwrap();
        assert_eq!(
            e(100).checked_add(Amount::new(1, usd)).unwrap_err(),
            MoneyError::CurrencyMismatch {
                expected: eur(),
                actual: usd
            },
            "mismatch is Err, not a panic"
        );
        assert!(e(100).checked_sub(Amount::new(1, usd)).is_err());
        assert_eq!(e(100).checked_add(e(23)).unwrap().minor(), 123);
        assert_eq!(e(100).checked_sub(e(123)).unwrap().minor(), -23);
        assert_eq!(e(-5).checked_neg().unwrap().minor(), 5);
        assert_eq!(e(-5).checked_abs().unwrap().minor(), 5);
        assert_eq!(e(21).checked_mul(2).unwrap().minor(), 42);
        assert_eq!(e(21).checked_mul(-2).unwrap().minor(), -42);
        assert!(matches!(
            e(i128::MAX).checked_add(e(1)),
            Err(MoneyError::Overflow)
        ));
        assert!(matches!(
            e(i128::MIN).checked_neg(),
            Err(MoneyError::Overflow)
        ));
        assert!(matches!(
            e(i128::MIN).checked_abs(),
            Err(MoneyError::Overflow)
        ));
        assert!(matches!(
            e(i128::MAX).checked_mul(2),
            Err(MoneyError::Overflow)
        ));
    }

    #[test]
    fn predicates_and_signum() {
        assert!(e(0).is_zero());
        assert!(!e(1).is_zero());
        assert!(e(1).is_positive());
        assert!(e(-1).is_negative());
        assert_eq!(e(-7).signum(), -1);
        assert_eq!(e(0).signum(), 0);
        assert_eq!(e(7).signum(), 1);
    }

    #[test]
    fn storage_boundary_round_trips() {
        assert_eq!(
            Amount::new(i128::from(i64::MAX), eur()).to_i64().unwrap(),
            i64::MAX
        );
        assert_eq!(
            Amount::new(i128::from(i64::MIN), eur()).to_i64().unwrap(),
            i64::MIN
        );
        assert!(matches!(
            Amount::new(i128::from(i64::MAX) + 1, eur()).to_i64(),
            Err(MoneyError::Overflow)
        ));
        assert!(matches!(
            Amount::new(i128::from(i64::MIN) - 1, eur()).to_i64(),
            Err(MoneyError::Overflow)
        ));
        for minor in [i64::MIN, -1, 0, 1, 123_456, i64::MAX] {
            let a = Amount::from_i64(minor, eur());
            assert_eq!(a.to_i64().unwrap(), minor);
            assert_eq!(Amount::new(i128::from(minor), eur()), a);
        }
    }

    #[test]
    fn canonical_strings() {
        assert_eq!(e(123_456).to_canonical_string(), "1234.56");
        assert_eq!(Amount::new(1234, jpy()).to_canonical_string(), "1234");
        assert_eq!(Amount::new(1234, kwd()).to_canonical_string(), "1.234");
        assert_eq!(Amount::new(1234, eur()).to_canonical_string(), "12.34");
        assert_eq!(e(0).to_canonical_string(), "0.00");
        assert_eq!(Amount::new(0, jpy()).to_canonical_string(), "0");
        assert_eq!(e(-123_456).to_canonical_string(), "-1234.56");
        assert_eq!(e(-5).to_canonical_string(), "-0.05");
        assert_eq!(e(5).to_canonical_string(), "0.05");
        assert_eq!(
            e(i128::MIN).to_canonical_string(),
            "-1701411834604692317316873037158841057.28"
        );
        assert_eq!(
            e(i128::MAX).to_canonical_string(),
            "1701411834604692317316873037158841057.27"
        );
        assert_eq!(e(123_456).to_string(), "1234.56 EUR");
    }

    #[test]
    fn canonical_parse_round_trip() {
        for minor in [
            i128::from(i64::MIN),
            -123_456,
            -1,
            0,
            1,
            99,
            123_456,
            i128::from(i64::MAX),
        ] {
            let a = Amount::new(minor, eur());
            assert_eq!(
                Amount::parse_canonical(&a.to_canonical_string(), eur()).unwrap(),
                a
            );
        }
        assert_eq!(
            Amount::parse_canonical("1234.56", eur()).unwrap().minor(),
            123_456
        );
        assert_eq!(
            Amount::parse_canonical("1.5", eur()).unwrap().minor(),
            150,
            "pad right"
        );
        assert_eq!(
            Amount::parse_canonical("+1234.56", eur()).unwrap().minor(),
            123_456
        );
        assert_eq!(Amount::parse_canonical("-0.01", eur()).unwrap().minor(), -1);
        assert_eq!(Amount::parse_canonical("0", jpy()).unwrap().minor(), 0);
        assert_eq!(
            Amount::parse_canonical("1234", jpy()).unwrap().minor(),
            1234
        );
    }

    #[test]
    fn canonical_round_trip_at_i128_bounds() {
        for (minor, cur) in [
            (i128::MIN, eur()),
            (i128::MAX, eur()),
            (i128::MIN, jpy()),
            (i128::MAX, jpy()),
        ] {
            let a = Amount::new(minor, cur);
            let s = a.to_canonical_string();
            assert_eq!(Amount::parse_canonical(&s, cur).unwrap(), a, "{s}");
        }
        // A magnitude of 2^127 with no sign (or with '+') is above
        // i128::MAX; only the '-' form, i128::MIN, is representable.
        let two_pow_127 = "170141183460469231731687303715884105728";
        assert!(matches!(
            Amount::parse_canonical(two_pow_127, eur()),
            Err(MoneyError::Overflow)
        ));
        assert!(matches!(
            Amount::parse_canonical(&format!("+{two_pow_127}"), eur()),
            Err(MoneyError::Overflow)
        ));
        assert_eq!(
            Amount::parse_canonical(&format!("-{two_pow_127}"), jpy())
                .unwrap()
                .minor(),
            i128::MIN
        );
    }

    #[test]
    fn parse_rejects_malformed() {
        let bad = [
            "", ".", "-.", "1..2", "1,2.3", "12.", "+-1", " 123", "123 ", "1 2", "١٢٣", "1.2.3",
            "1,234.56", "(123)", "1.2e3",
        ];
        for s in bad {
            assert!(
                Amount::parse_canonical(s, eur()).is_err(),
                "canonical parse of {s:?} should fail"
            );
        }
        assert!(matches!(
            Amount::parse_canonical("0.001", eur()).unwrap_err(),
            MoneyError::ParseError {
                reason: "fraction longer than currency exponent",
                ..
            }
        ));
        assert!(Amount::parse_canonical("0.01", jpy()).is_err());
        assert!(matches!(
            Amount::parse_canonical("9999999999999999999999999999999999999999999", eur())
                .unwrap_err(),
            MoneyError::Overflow
        ));
    }

    #[test]
    fn locale_parse_and_grouped_format() {
        let de = DecimalStyle::DE_DE;
        let en = DecimalStyle::EN_US;
        assert_eq!(
            Amount::parse_locale("1.234,56", eur(), de).unwrap().minor(),
            123_456
        );
        assert_eq!(
            Amount::parse_locale("1,234.56", eur(), en).unwrap().minor(),
            123_456
        );
        assert_eq!(
            Amount::parse_locale("1234.56", eur(), en).unwrap().minor(),
            123_456,
            "ungrouped is always fine"
        );
        // FR accepts all three space variants.
        let fr = DecimalStyle::FR_FR;
        for s in ["1\u{202f}234,56", "1\u{a0}234,56", "1 234,56"] {
            assert_eq!(
                Amount::parse_locale(s, eur(), fr).unwrap().minor(),
                123_456,
                "{s:?}"
            );
        }
        // Indian grouping: three then twos.
        let ind = DecimalStyle::EN_IN;
        assert_eq!(
            Amount::parse_locale("1,23,456.78", eur(), ind)
                .unwrap()
                .minor(),
            12_345_678
        );
        assert_eq!(
            Amount::parse_locale("12,34,567", eur(), ind)
                .unwrap()
                .minor(),
            123_456_700,
            "a leading run of two is valid Indian grouping"
        );
        // Misgrouped.
        assert!(Amount::parse_locale("123,45,678", eur(), ind).is_err());
        assert!(Amount::parse_locale("12,34,567", eur(), en).is_err());
        assert!(Amount::parse_locale("1,2.3", eur(), en).is_err());
        assert!(Amount::parse_locale("1234,567", eur(), en).is_err());
        assert!(Amount::parse_locale("1,.23", eur(), en).is_err());
        assert!(Amount::parse_locale(",123", eur(), en).is_err());
        assert!(Amount::parse_locale("123,", eur(), en).is_err());
        // A decimal separator from another style is rejected, not ignored.
        assert!(Amount::parse_locale("1.234,56", eur(), en).is_err());

        // Formatting.
        assert_eq!(e(123_456_789).format_grouped(en), "1,234,567.89");
        assert_eq!(e(123_456_789).format_grouped(de), "1.234.567,89");
        assert_eq!(
            e(123_456_789).format_grouped(fr),
            "1\u{202f}234\u{202f}567,89"
        );
        assert_eq!(
            e(123_456_789).format_grouped(DecimalStyle::DE_CH),
            "1'234'567.89"
        );
        assert_eq!(
            Amount::new(1_234_567_890, eur()).format_grouped(ind),
            "1,23,45,678.90"
        );
        assert_eq!(e(5).format_grouped(en), "0.05");
        assert_eq!(e(-123_456_789).format_grouped(en), "-1,234,567.89");
        assert_eq!(Amount::new(-1234, jpy()).format_grouped(en), "-1,234");
        // 1234 KWD minor = 1.234; DE_DE swaps the separators.
        assert_eq!(Amount::new(1234, kwd()).format_grouped(de), "1,234");

        // Round trips.
        for minor in [
            0,
            5,
            999,
            123_456,
            1_000_000,
            i128::from(i64::MAX),
            i128::from(i64::MIN),
        ] {
            let a = Amount::new(minor, eur());
            for st in [en, de] {
                let s = a.format_grouped(st);
                assert_eq!(Amount::parse_locale(&s, eur(), st).unwrap(), a, "{s}");
            }
        }
    }

    #[test]
    fn isolated_wrapper() {
        let s = e(1234).format_grouped_isolated(DecimalStyle::EN_US);
        assert!(s.starts_with('\u{2066}'));
        assert!(s.ends_with('\u{2069}'));
        assert_eq!(s, "\u{2066}12.34\u{2069}");
        // The wrapper characters are not accepted on parse.
        assert!(Amount::parse_locale(&s, eur(), DecimalStyle::EN_US).is_err());
    }

    #[test]
    fn mul_rate_rounding_modes() {
        let rate = Rate::from_scaled(5 * SCALE / 10); // 0.5
        for (minor, he, hu, down) in [
            (5_i128, 2_i128, 3_i128, 2_i128), // 2.5 -> 2 (even) / 3 / 2
            (7, 4, 4, 3),                     // 3.5 -> 4 / 4 / 3
            (9, 4, 5, 4),                     // 4.5 -> 4 (even) / 5 / 4
            (11, 6, 6, 5),                    // 5.5 -> 6 / 6 / 5
            (15, 8, 8, 7),                    // 7.5 -> 8 / 8 / 7
            (-5, -2, -3, -2),                 // symmetric on magnitude
            (-15, -8, -8, -7),
            (10, 5, 5, 5), // exact division unaffected
            (0, 0, 0, 0),
        ] {
            let a = e(minor);
            assert_eq!(
                a.mul_rate(rate, Rounding::HalfEven).unwrap().minor(),
                he,
                "HalfEven {minor}"
            );
            assert_eq!(
                a.mul_rate(rate, Rounding::HalfUp).unwrap().minor(),
                hu,
                "HalfUp {minor}"
            );
            assert_eq!(
                a.mul_rate(rate, Rounding::Down).unwrap().minor(),
                down,
                "Down {minor}"
            );
        }
        // ONE is exact in every mode; ZERO gives zero.
        for mode in [Rounding::HalfEven, Rounding::HalfUp, Rounding::Down] {
            assert_eq!(
                e(123_456).mul_rate(Rate::ONE, mode).unwrap().minor(),
                123_456
            );
            assert_eq!(e(123_456).mul_rate(Rate::ZERO, mode).unwrap().minor(), 0);
        }
        // Non-tie rounding: 1234 minor * 0.025 = 30.85 -> 31 / 31 / 30.
        let r = Rate::parse("0.025").unwrap();
        assert_eq!(e(1234).mul_rate(r, Rounding::HalfEven).unwrap().minor(), 31);
        assert_eq!(e(1234).mul_rate(r, Rounding::HalfUp).unwrap().minor(), 31);
        assert_eq!(e(1234).mul_rate(r, Rounding::Down).unwrap().minor(), 30);
        // Overflow instead of wrap.
        assert!(matches!(
            e(i128::MAX).mul_rate(Rate::ONE, Rounding::Down),
            Err(MoneyError::Overflow)
        ));
    }

    #[test]
    fn ordering_is_currency_then_minor() {
        let usd = Currency::from_code("USD").unwrap();
        let eur_amount = e(9_999_999);
        let usd_amount = Amount::new(1, usd);
        assert!(eur_amount < usd_amount, "EUR < USD by code");
        assert!(e(1) < e(2));
        assert!(e(-2) < e(-1));
        assert_eq!(e(1), e(1));
    }
}
