//! Tolerance rules: allowed = min(abs, reference × `pct_bp` / 10000).
//! Only overruns checked; underruns never block.

#![allow(clippy::cast_possible_truncation)]

use crate::model::Quantity;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Tolerance: absolute and percentage limits. Within when variance ≤ allowed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tolerance {
    pub quantity_abs: Quantity, // Absolute quantity variance (units)
    pub quantity_pct_bp: u32,   // Percentage tolerance in basis points
    pub price_abs_minor: i64,   // Absolute price variance (minor units)
    pub price_pct_bp: u32,      // Percentage price tolerance in basis points
}

impl Tolerance {
    /// The zero tolerance: no variance allowed.
    pub const ZERO: Self = Self {
        quantity_abs: Quantity::from_thousandths(0),
        quantity_pct_bp: 0,
        price_abs_minor: 0,
        price_pct_bp: 0,
    };

    /// Allowed quantity variance (min of abs and pct).
    #[must_use]
    pub fn allowed_quantity(&self, reference: Quantity) -> Quantity {
        let abs_allowed = self.quantity_abs.to_thousandths();
        let ref_thr = reference.to_thousandths();

        if self.quantity_pct_bp == 0 || ref_thr == 0 {
            return self.quantity_abs;
        }

        let pct_allowed = i128::from(ref_thr)
            .checked_mul(i128::from(self.quantity_pct_bp))
            .unwrap_or(i128::MAX)
            .saturating_div(10000);
        let pct_allowed_i64 = i64::try_from(pct_allowed).unwrap_or(i64::MAX);
        Quantity::from_thousandths(abs_allowed.min(pct_allowed_i64))
    }

    /// Allowed variance for a price in minor units (min of abs and pct).
    /// Reference is non-negative; uses checked/saturating i128 math.
    #[must_use]
    pub fn allowed_price(&self, reference_minor: i128) -> i128 {
        if self.price_pct_bp == 0 || reference_minor == 0 {
            return i128::from(self.price_abs_minor);
        }

        let pct_allowed = reference_minor
            .checked_mul(i128::from(self.price_pct_bp))
            .unwrap_or(i128::MAX)
            .saturating_div(10000);
        i128::from(self.price_abs_minor).min(pct_allowed)
    }

    /// Check if a quantity variance is within this tolerance (overrun only).
    #[must_use]
    pub fn is_quantity_within(&self, variance: Quantity, reference: Quantity) -> bool {
        // Only overruns are checked; underruns never block
        if variance.to_thousandths() <= 0 {
            return true;
        }
        variance.to_thousandths() <= self.allowed_quantity(reference).to_thousandths()
    }

    /// Check if a price variance is within this tolerance (overrun only, in minor units).
    #[must_use]
    pub fn is_price_within(&self, variance_minor: i64, reference_minor: i128) -> bool {
        // Only overruns are checked; underruns never block
        if variance_minor <= 0 {
            return true;
        }
        i128::from(variance_minor) <= self.allowed_price(reference_minor)
    }
}

/// A set of tolerances: a default and vendor-specific overrides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tolerances {
    /// Default tolerance applied to all vendors not in `vendors`.
    pub default: Tolerance,
    /// Vendor-specific tolerances, keyed by vendor name.
    pub vendors: BTreeMap<String, Tolerance>,
}

impl Tolerances {
    /// Retrieve the tolerance for a specific vendor, falling back to default.
    #[must_use]
    pub fn for_vendor(&self, vendor: &str) -> Tolerance {
        self.vendors
            .get(vendor)
            .cloned()
            .unwrap_or_else(|| self.default.clone())
    }

    /// Create a new set with a default tolerance and no vendor overrides.
    #[must_use]
    pub fn new(default: Tolerance) -> Self {
        Self {
            default,
            vendors: BTreeMap::new(),
        }
    }

    /// Add a vendor-specific tolerance.
    #[must_use]
    pub fn with_vendor(mut self, vendor: String, tolerance: Tolerance) -> Self {
        self.vendors.insert(vendor, tolerance);
        self
    }
}
