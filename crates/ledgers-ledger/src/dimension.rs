//! Dimensions: internal units, cost centres, and projects.
//!
//! Dimensions allow lines to be tagged with categorization: a `unit`
//! (for inter-unit balancing), a `cost_centre`, or a `project`. Accounts
//! can enforce that lines carry certain dimensions, or forbid others.
//!
//! This module defines the types and store helpers; posting logic lives
//! in [`crate::posting`] and reports in [`crate::report`].

use crate::error::AccountsError;
use ledgers_money::Amount;
use std::collections::BTreeMap;

/// The three dimension kinds: units, cost centres, projects.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DimensionKind {
    /// An internal unit (for per-unit balancing).
    Unit,
    /// A cost centre.
    CostCentre,
    /// A project.
    Project,
}

impl DimensionKind {
    /// The wire key for this kind (matches the `BTreeMap` keys on lines).
    #[must_use]
    pub fn wire_key(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::CostCentre => "cost_centre",
            Self::Project => "project",
        }
    }

    /// All three kinds.
    pub const ALL: &[Self] = &[Self::Unit, Self::CostCentre, Self::Project];
}

impl std::fmt::Display for DimensionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire_key())
    }
}

/// A dimension rule: whether a dimension is required or forbidden on an account.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DimensionRule {
    /// A line on this account must carry this dimension.
    Required,
    /// A line on this account must not carry this dimension.
    Forbidden,
}

impl std::fmt::Display for DimensionRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Required => "required",
            Self::Forbidden => "forbidden",
        })
    }
}

impl std::str::FromStr for DimensionRule {
    type Err = AccountsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "required" => Ok(Self::Required),
            "forbidden" => Ok(Self::Forbidden),
            _ => Err(AccountsError::UnknownDimensionRule {
                value: s.to_owned(),
            }),
        }
    }
}

/// A pure function that, given an entry's lines, the set of own-books
/// unit codes, and the clearing account number, returns the clearing
/// lines needed to balance each unit independently.
///
/// For each own-books unit whose lines don't net to zero in base, adds
/// one clearing line on the clearing account carrying `unit=<code>` with
/// the opposite side/amount. If the sum of those clearing lines is
/// non-zero (because other lines have no unit or a non-own-books unit),
/// adds one compensating clearing line with no unit, so the entry still
/// balances overall. Base currency only (no fx on clearing lines).
///
/// Returns an empty vec if no clearing is needed.
///
/// # Errors
/// Returns `MoneyError` if an amount operation overflows while computing
/// clearing line totals.
pub fn clearing_lines_for_units(
    lines: &[crate::Line],
    own_books_units: &[String],
    clearing_account: &str,
    base_currency: ledgers_money::Currency,
) -> Result<Vec<crate::Line>, ledgers_money::MoneyError> {
    use crate::Side;

    // TEST: temporarily disable clearing to verify AC2 test fails without clearing
    // return Ok(Vec::new());

    let mut result = Vec::new();
    let mut clearing_total = Amount::new(0, base_currency);

    // For each own-books unit, compute the net in base and add a
    // clearing line if unbalanced.
    for unit_code in own_books_units {
        let unit_key = "unit";
        let mut unit_net_minor: i128 = 0;
        for line in lines.iter().filter(|line| {
            line.dimensions
                .get(unit_key)
                .is_some_and(|u| u == unit_code)
        }) {
            let signed = line.base.minor() * i128::from(line.side.sign());
            unit_net_minor += signed;
        }
        let unit_net = Amount::new(unit_net_minor, base_currency);

        if !unit_net.is_zero() {
            // The clearing line has the opposite side and sign to balance this unit.
            let clearing_side = if unit_net.minor() > 0 {
                Side::Credit
            } else {
                Side::Debit
            };
            let clearing_amount = Amount::new(unit_net.minor().abs(), base_currency);

            let mut dimensions = BTreeMap::new();
            dimensions.insert("unit".to_owned(), unit_code.clone());

            result.push(crate::Line {
                account_number: clearing_account.to_owned(),
                side: clearing_side,
                base: clearing_amount,
                fx: None,
                tax_code: None,
                dimensions,
                memo: None,
            });

            // Add to the clearing total.
            let clearing_signed = clearing_amount.minor() * i128::from(clearing_side.sign());
            clearing_total = Amount::new(clearing_total.minor() + clearing_signed, base_currency);
        }
    }

    // If the clearing lines don't net to zero, add a compensating line with no unit.
    if !clearing_total.is_zero() {
        let compensating_side = if clearing_total.minor() > 0 {
            Side::Credit
        } else {
            Side::Debit
        };
        let compensating_amount = Amount::new(clearing_total.minor().abs(), base_currency);

        result.push(crate::Line {
            account_number: clearing_account.to_owned(),
            side: compensating_side,
            base: compensating_amount,
            fx: None,
            tax_code: None,
            dimensions: BTreeMap::new(),
            memo: None,
        });
    }

    Ok(result)
}
