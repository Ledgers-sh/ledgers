//! Property tests. The acceptance bar is 10 000 cases per property in CI
//! (`cargo test`), so every strategy is small-int-based and each case is
//! a handful of checked integer operations.

use crate::amount::Amount;
use crate::currency::Currency;
use crate::rate::{Rate, Rounding, SCALE};
use crate::style::DecimalStyle;
use proptest::prelude::*;

/// A fixed sample of codes covering the exponent range 0-4, active and
/// historic.
const CODES: [&str; 5] = ["EUR", "JPY", "KWD", "CLF", "DEM"];

fn currencies() -> [Currency; CODES.len()] {
    let mut out = [Currency::from_code("EUR").expect("fixture currency"); CODES.len()];
    for (slot, code) in out.iter_mut().zip(CODES) {
        *slot = Currency::from_code(code).expect("fixture currency");
    }
    out
}

proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 10_000,
        ..proptest::test_runner::Config::default()
    })]

    /// 1. Allocation: the parts sum to the input exactly, share its
    /// currency, sit within one minor unit of their exact shares, and the
    /// split is deterministic.
    #[test]
    fn allocation_sum_invariant(
        minor in any::<i64>(),
        weights in prop::collection::vec(1_i128..=1000, 1..=5),
        currency_idx in 0_usize..CODES.len(),
    ) {
        let cs = currencies();
        let currency = cs[currency_idx];
        let amount = Amount::new(i128::from(minor), currency);
        let parts = amount.allocate(&weights).expect("bounded products");
        prop_assert_eq!(parts.len(), weights.len());
        let total_weight: i128 = weights.iter().sum();
        let mag = i128::from(minor).checked_abs().expect("i64 range fits i128");
        let mut sum = Amount::new(0, currency);
        for (i, part) in parts.iter().enumerate() {
            prop_assert_eq!(part.currency(), currency);
            // |minor| <= i64::MAX and w <= 1000, so the product fits.
            let exact_base = mag.checked_mul(weights[i]).expect("bounded product") / total_weight;
            let m = part.minor().checked_abs().expect("magnitude <= i64::MAX");
            prop_assert!(
                m == exact_base || m == exact_base + 1,
                "part {i} is more than one unit off its exact share"
            );
            sum = sum.checked_add(*part).expect("parts sum to the input");
        }
        prop_assert_eq!(sum, amount);
        let again = amount.allocate(&weights).expect("bounded products");
        prop_assert_eq!(parts, again, "allocation must be deterministic");
    }

    /// 2. Equal split: the parts sum to the input and pairwise differ by
    /// at most one minor unit.
    #[test]
    fn allocate_n_parts_differ_by_at_most_one(
        minor in any::<i64>(),
        parts in 1_u32..=20,
        currency_idx in 0_usize..CODES.len(),
    ) {
        let cs = currencies();
        let currency = cs[currency_idx];
        let amount = Amount::new(i128::from(minor), currency);
        let split = amount.allocate_n(parts).expect("n <= 20");
        prop_assert_eq!(split.len(), parts as usize);
        let mut sum = Amount::new(0, currency);
        for part in &split {
            for other in &split {
                prop_assert!((part.minor() - other.minor()).abs() <= 1);
            }
            sum = sum.checked_add(*part).expect("parts sum to the input");
        }
        prop_assert_eq!(sum, amount);
    }

    /// 3. Canonical round trip.
    #[test]
    fn canonical_round_trip(
        minor in any::<i64>(),
        currency_idx in 0_usize..CODES.len(),
    ) {
        let cs = currencies();
        let currency = cs[currency_idx];
        let amount = Amount::new(i128::from(minor), currency);
        let text = amount.to_canonical_string();
        prop_assert_eq!(text.contains('-'), amount.is_negative());
        prop_assert_eq!(Amount::parse_canonical(&text, currency).unwrap(), amount);
    }

    /// 4. Grouped round trip through EN_US and DE_DE.
    #[test]
    fn grouped_round_trip(
        minor in any::<i64>(),
        style_idx in 0_usize..2,
    ) {
        let currency = Currency::from_code("EUR").expect("fixture currency");
        let style = [DecimalStyle::EN_US, DecimalStyle::DE_DE][style_idx];
        let amount = Amount::new(i128::from(minor), currency);
        let text = amount.format_grouped(style);
        prop_assert_eq!(Amount::parse_locale(&text, currency, style).unwrap(), amount);
    }

    /// 5. Addition associativity and cancellation on small magnitudes
    /// (no overflow, so no skipped cases).
    #[test]
    fn add_sub_associativity_and_cancellation(
        a in any::<i16>(),
        b in any::<i16>(),
        c in any::<i16>(),
    ) {
        let currency = Currency::from_code("EUR").expect("fixture currency");
        let (a, b, c) = (
            Amount::new(i128::from(a), currency),
            Amount::new(i128::from(b), currency),
            Amount::new(i128::from(c), currency),
        );
        let zero = Amount::new(0, currency);
        prop_assert_eq!(a.checked_add(zero).unwrap(), a);
        prop_assert_eq!(
            a.checked_add(b).unwrap().checked_add(c).unwrap(),
            a.checked_add(b.checked_add(c).unwrap()).unwrap()
        );
        prop_assert_eq!(a.checked_add(b).unwrap().checked_sub(b).unwrap(), a);
        prop_assert_eq!(a.checked_sub(a).unwrap(), zero);
    }

    /// 6. No-panic: every checked operation over the full [`i128`] range
    /// returns without panicking, and whenever it returns `Ok` the result
    /// re-derives.
    #[test]
    fn checked_ops_never_panic(
        minor in any::<i128>(),
        qty in any::<i64>(),
        currency_idx in 0_usize..CODES.len(),
    ) {
        let cs = currencies();
        let currency = cs[currency_idx];
        let a = Amount::new(minor, currency);
        let b = Amount::new(minor.wrapping_add(1), currency);
        let _ = a.checked_add(b);
        let _ = a.checked_sub(b);
        let _ = a.checked_neg();
        let _ = a.checked_abs();
        let _ = a.checked_mul(qty);
        let _ = a.to_i64();
        if let Ok(s) = a.checked_add(b) {
            prop_assert_eq!(s.checked_sub(b).unwrap(), a);
        }
        if let Ok(p) = a.checked_mul(1) {
            prop_assert_eq!(p, a);
        }
        if let Ok(z) = a.checked_mul(0) {
            prop_assert!(z.is_zero());
            prop_assert_eq!(z.currency(), currency);
        }
        if let Ok(n) = a.to_i64() {
            prop_assert_eq!(Amount::from_i64(n, currency), a);
        }
        if let Ok(neg) = a.checked_neg() {
            prop_assert_eq!(neg.signum(), -a.signum());
        }
    }

    /// 7. Rate Display/parse round trip.
    #[test]
    fn rate_round_trip(scaled in any::<i64>()) {
        let rate = Rate::from_scaled(i128::from(scaled));
        prop_assert_eq!(Rate::parse(&rate.to_string()).unwrap(), rate);
    }

    /// 8. `mul_rate` against an independently computed quotient (products
    /// bounded to 10²⁷, well inside [`i128`]), plus the mode ordering.
    #[test]
    fn mul_rate_matches_independent_quotient(
        minor in 0_i128..1_000_000_000_000,
        scaled in 0_i128..1_000_000_000_000_000,
    ) {
        let currency = Currency::from_code("EUR").expect("fixture currency");
        let amount = Amount::new(minor, currency);
        let rate = Rate::from_scaled(scaled);
        let product = minor.checked_mul(scaled).expect("bounded by the strategies");
        let floor_q = product / SCALE;
        let rem = product % SCALE;
        let half_up = floor_q + i128::from(2 * rem >= SCALE);
        let half_even = if 2 * rem > SCALE || (2 * rem == SCALE && floor_q % 2 == 1) {
            floor_q + 1
        } else {
            floor_q
        };
        let down = amount.mul_rate(rate, Rounding::Down).unwrap().minor();
        let he = amount.mul_rate(rate, Rounding::HalfEven).unwrap().minor();
        let hu = amount.mul_rate(rate, Rounding::HalfUp).unwrap().minor();
        prop_assert_eq!(down, floor_q);
        prop_assert_eq!(he, half_even);
        prop_assert_eq!(hu, half_up);
        prop_assert!(down <= he && he <= hu);
    }

    /// 9. The [`i64`] storage boundary.
    #[test]
    fn storage_boundary(
        minor in any::<i64>(),
        overshoot in 1_i128..=1_000_000_000_000,
    ) {
        let currency = Currency::from_code("EUR").expect("fixture currency");
        let amount = Amount::from_i64(minor, currency);
        prop_assert_eq!(amount.to_i64().unwrap(), minor);
        prop_assert!(Amount::new(i128::from(i64::MAX) + overshoot, currency)
            .to_i64()
            .is_err());
        prop_assert!(Amount::new(i128::from(i64::MIN) - overshoot, currency)
            .to_i64()
            .is_err());
    }
}
