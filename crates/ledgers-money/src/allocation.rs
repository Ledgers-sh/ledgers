//! Largest-remainder allocation: splitting an [`Amount`] into parts that
//! sum to the input exactly, with no lost or invented minor unit.

use crate::amount::Amount;
use crate::error::MoneyError;

/// The maximum number of parts [`Amount::allocate_n`] will split into.
///
/// A million parts is ~32 MiB of output — generous for any accounting
/// split — while an untrusted `parts` (up to [`u32::MAX`]) must yield
/// [`MoneyError::InvalidAllocation`], never an allocation so large the
/// allocator aborts instead of failing.
pub const MAX_ALLOCATION_PARTS: u32 = 1_000_000;

impl Amount {
    /// Split `self` across `weights`, largest-remainder first.
    ///
    /// Every weight must be a positive integer (empty input, zero or
    /// negative weights, and a weight sum that overflows [`i128`] are
    /// [`MoneyError::InvalidAllocation`]). The split is computed on the
    /// magnitude with floor division, then the remaining units are handed
    /// out one each to the parts with the largest remainders, ties broken
    /// by lower index — deterministic for the same inputs. The sign is
    /// reapplied at the end.
    ///
    /// Guarantees, property-tested: the parts sum to `self` exactly, every
    /// part is within one minor unit of its exact real share, and all
    /// parts share `self`'s currency. A zero amount yields all-zero parts
    /// (still one per weight).
    ///
    /// # Errors
    /// [`MoneyError::InvalidAllocation`] for empty or non-positive
    /// weights or an overflowing weight sum; [`MoneyError::Overflow`] if
    /// an intermediate product does not fit an [`i128`].
    pub fn allocate(self, weights: &[i128]) -> Result<Vec<Amount>, MoneyError> {
        if weights.is_empty() {
            return Err(MoneyError::InvalidAllocation { reason: "no parts" });
        }
        if weights.iter().any(|&w| w <= 0) {
            return Err(MoneyError::InvalidAllocation {
                reason: "weights must be positive",
            });
        }
        let total = weights
            .iter()
            .try_fold(0_i128, |acc, &w| acc.checked_add(w))
            .ok_or(MoneyError::InvalidAllocation {
                reason: "weights sum overflows",
            })?;
        let mag = self.minor().checked_abs().ok_or(MoneyError::Overflow)?;
        let parts = allocate_magnitude(mag, weights, total)?;
        Ok(self.apply_sign(parts))
    }

    /// Split `self` into `parts` equal parts, giving the remainder (with
    /// the sign of the amount) to the earliest parts: `100/3` →
    /// `[34, 33, 33]`, `-100/3` → `[-34, -33, -33]`, `1/2` → `[1, 0]`.
    ///
    /// `parts` is capped at [`MAX_ALLOCATION_PARTS`]: `parts` is often
    /// untrusted, and a count beyond the cap must come back as an
    /// error, not as a request so large the allocator aborts.
    ///
    /// # Errors
    /// [`MoneyError::InvalidAllocation`] when `parts` is zero or exceeds
    /// [`MAX_ALLOCATION_PARTS`].
    pub fn allocate_n(self, parts: u32) -> Result<Vec<Amount>, MoneyError> {
        if parts == 0 {
            return Err(MoneyError::InvalidAllocation { reason: "no parts" });
        }
        if parts > MAX_ALLOCATION_PARTS {
            return Err(MoneyError::InvalidAllocation {
                reason: "too many parts",
            });
        }
        let n = i128::from(parts);
        let mag = self.minor().checked_abs().ok_or(MoneyError::Overflow)?;
        let base = mag / n;
        let mut leftover = mag % n;
        let mut minors = vec![base; parts as usize];
        for minor in &mut minors {
            if leftover == 0 {
                break;
            }
            *minor += 1;
            leftover -= 1;
        }
        Ok(self.apply_sign(minors))
    }

    /// Reapply `self`'s sign to magnitude parts and wrap them as amounts
    /// of `self`'s currency. Each part has magnitude at most `|self|`, so
    /// the negation cannot overflow.
    fn apply_sign(self, minors: Vec<i128>) -> Vec<Amount> {
        if self.minor() < 0 {
            // Each part has magnitude <= mag <= i128::MAX, so negation
            // cannot overflow.
            minors
                .into_iter()
                .map(|m| Amount::new(-m, self.currency()))
                .collect()
        } else {
            minors
                .into_iter()
                .map(|m| Amount::new(m, self.currency()))
                .collect()
        }
    }
}

/// Largest-remainder split of `mag` across positive `weights` summing to
/// `total`. Products `mag * w` are checked: a caller passing an
/// [`i128`]-sized magnitude with large weights gets
/// [`MoneyError::Overflow`], never a wrapped share.
fn allocate_magnitude(mag: i128, weights: &[i128], total: i128) -> Result<Vec<i128>, MoneyError> {
    let mut shares: Vec<i128> = Vec::with_capacity(weights.len());
    let mut remainders: Vec<i128> = Vec::with_capacity(weights.len());
    let mut distributed: i128 = 0;
    for &w in weights {
        let product = mag.checked_mul(w).ok_or(MoneyError::Overflow)?;
        let base = product / total;
        shares.push(base);
        remainders.push(product % total);
        distributed += base;
    }
    let mut leftover = mag - distributed;
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| remainders[b].cmp(&remainders[a]).then(a.cmp(&b)));
    for &idx in &order {
        if leftover == 0 {
            break;
        }
        shares[idx] += 1;
        leftover -= 1;
    }
    Ok(shares)
}

#[cfg(test)]
mod tests {
    use super::MAX_ALLOCATION_PARTS;
    use crate::amount::Amount;
    use crate::currency::Currency;
    use crate::error::MoneyError;

    fn eur() -> Currency {
        Currency::from_code("EUR").unwrap()
    }
    fn e(minor: i128) -> Amount {
        Amount::new(minor, eur())
    }
    fn minors(parts: &[Amount]) -> Vec<i128> {
        parts.iter().map(|p| p.minor()).collect()
    }

    fn sum(parts: &[Amount]) -> Amount {
        parts
            .iter()
            .copied()
            .fold(Amount::new(0, eur()), |acc, p| acc.checked_add(p).unwrap())
    }

    #[test]
    fn equal_split_with_remainder_to_earliest() {
        assert_eq!(minors(&e(100).allocate_n(3).unwrap()), vec![34, 33, 33]);
        assert_eq!(minors(&e(-100).allocate_n(3).unwrap()), vec![-34, -33, -33]);
        assert_eq!(minors(&e(1).allocate_n(2).unwrap()), vec![1, 0]);
        assert_eq!(minors(&e(-1).allocate_n(2).unwrap()), vec![-1, 0]);
        assert_eq!(minors(&e(99).allocate_n(3).unwrap()), vec![33, 33, 33]);
        assert_eq!(minors(&e(0).allocate_n(3).unwrap()), vec![0, 0, 0]);
        assert_eq!(minors(&e(5).allocate_n(1).unwrap()), vec![5]);
        assert!(matches!(
            e(5).allocate_n(0),
            Err(MoneyError::InvalidAllocation { reason: "no parts" })
        ));
    }

    #[test]
    fn weighted_split_and_tie_break() {
        // Ties break to the lower index.
        assert_eq!(minors(&e(1).allocate(&[1, 1, 1]).unwrap()), vec![1, 0, 0]);
        assert_eq!(minors(&e(2).allocate(&[1, 1, 1]).unwrap()), vec![1, 1, 0]);
        // Largest remainder wins the leftover unit: 10*1 % 4 = 2 for the
        // first two weights, 10*2 % 4 = 0 for the third, so the leftover
        // unit goes to index 0 (tie with index 1 broken by lower index).
        assert_eq!(minors(&e(10).allocate(&[1, 1, 2]).unwrap()), vec![3, 2, 5]);
        assert_eq!(minors(&e(100).allocate(&[70, 30]).unwrap()), vec![70, 30]);
        assert_eq!(
            minors(&e(-10).allocate(&[1, 1, 2]).unwrap()),
            vec![-3, -2, -5],
            "sign applied to the magnitude split"
        );
        assert_eq!(minors(&e(0).allocate(&[3, 7]).unwrap()), vec![0, 0]);
        // Every part within one minor unit of its exact share.
        let parts = e(1000).allocate(&[1, 2, 3, 4]).unwrap();
        for (i, p) in parts.iter().enumerate() {
            let exact = 1000 * (i as i128 + 1) / 10;
            assert!((p.minor() - exact).abs() <= 1);
        }
        assert_eq!(sum(&parts), e(1000));
    }

    #[test]
    fn sum_invariant_on_hand_cases() {
        for amount in [-101_i128, -100, -7, -1, 0, 1, 7, 100, 101, 123_456] {
            for n in 1_u32..=7 {
                let parts = e(amount).allocate_n(n).unwrap();
                assert_eq!(parts.len(), n as usize);
                assert_eq!(sum(&parts), e(amount), "{amount}/{n}");
                assert!(parts.iter().all(|p| p.currency() == eur()));
                let lo = parts.iter().map(|p| p.minor()).min().unwrap();
                let hi = parts.iter().map(|p| p.minor()).max().unwrap();
                assert!(hi - lo <= 1, "parts differ by more than one unit");
            }
            let parts = e(amount).allocate(&[2, 3, 5, 7, 11]).unwrap();
            assert_eq!(sum(&parts), e(amount));
        }
        // Deterministic across two runs.
        let a1 = e(123_457).allocate(&[7, 11, 13]).unwrap();
        let a2 = e(123_457).allocate(&[7, 11, 13]).unwrap();
        assert_eq!(minors(&a1), minors(&a2));
    }

    #[test]
    fn allocate_n_rejects_part_counts_beyond_the_cap() {
        assert!(matches!(
            e(100).allocate_n(u32::MAX),
            Err(MoneyError::InvalidAllocation {
                reason: "too many parts"
            })
        ));
        assert!(matches!(
            e(100).allocate_n(MAX_ALLOCATION_PARTS + 1),
            Err(MoneyError::InvalidAllocation {
                reason: "too many parts"
            })
        ));
        // At the cap itself the split succeeds and still sums exactly.
        let parts = e(1_000_001)
            .allocate_n(MAX_ALLOCATION_PARTS)
            .expect("the cap itself is fine");
        assert_eq!(parts.len(), MAX_ALLOCATION_PARTS as usize);
        assert_eq!(sum(&parts), e(1_000_001));
        assert_eq!(parts[0].minor(), 2, "the remainder goes to the earliest");
        assert_eq!(parts[1].minor(), 1);
    }

    #[test]
    fn invalid_weights() {
        assert!(matches!(
            e(10).allocate(&[]),
            Err(MoneyError::InvalidAllocation { reason: "no parts" })
        ));
        assert!(matches!(
            e(10).allocate(&[0]),
            Err(MoneyError::InvalidAllocation {
                reason: "weights must be positive"
            })
        ));
        assert!(matches!(
            e(10).allocate(&[1, -2, 3]),
            Err(MoneyError::InvalidAllocation {
                reason: "weights must be positive"
            })
        ));
        assert!(matches!(
            e(10).allocate(&[i128::MAX, 1]),
            Err(MoneyError::InvalidAllocation {
                reason: "weights sum overflows"
            })
        ));
        // i128::MIN magnitude cannot be abs'd.
        assert!(matches!(
            e(i128::MIN).allocate(&[1, 1]),
            Err(MoneyError::Overflow)
        ));
    }
}
