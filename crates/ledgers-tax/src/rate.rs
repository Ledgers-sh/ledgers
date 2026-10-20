//! Tax rates as dated data per jurisdiction.
//!
//! A [`DatedRate`] carries the jurisdiction, category, rate, valid date
//! range, and source. A [`RateTable`] validates that rates have no gaps
//! or overlaps and provides [`rate_on`] to look up the rate for a given
//! jurisdiction, category and date.
//!
//! The table is immutable and built once; dates are `time::Date` in the
//! domain and ISO 8601 strings only at boundaries.

use crate::error::TaxError;
use ledgers_money::Rate;
use std::collections::BTreeMap;
use time::Date;

/// A jurisdiction code: `"DE"` for Germany, `"generic"` for caller-supplied.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Jurisdiction {
    /// Germany (Deutschland).
    Germany,
    /// Caller-supplied rates for any jurisdiction.
    Generic,
}

impl Jurisdiction {
    /// Parse from a string: `"DE"` for Germany, `"generic"` otherwise.
    #[must_use]
    pub fn from_code(code: &str) -> Self {
        match code {
            "DE" => Jurisdiction::Germany,
            _ => Jurisdiction::Generic,
        }
    }

    /// The jurisdiction code as a string.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Jurisdiction::Germany => "DE",
            Jurisdiction::Generic => "generic",
        }
    }

    /// The rounding scope this jurisdiction requires (per-line or
    /// per-document).
    #[must_use]
    pub fn rounding_scope(self) -> crate::calc::RoundingScope {
        match self {
            Jurisdiction::Germany => crate::calc::RoundingScope::PerDocument,
            Jurisdiction::Generic => crate::calc::RoundingScope::PerLine,
        }
    }
}

impl std::fmt::Display for Jurisdiction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// A tax rate category.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RateCategory {
    /// Standard rate (e.g., 19% in Germany).
    Standard,
    /// Reduced rate (e.g., 7% in Germany).
    Reduced,
    /// Zero rate.
    Zero,
    /// Exempt (no VAT charged, not deductible).
    Exempt,
}

impl RateCategory {
    /// Parse from a string: `"standard"`, `"reduced"`, `"zero"`, `"exempt"`.
    ///
    /// # Errors
    ///
    /// Returns [`TaxError::Invalid`] if the code is not recognized.
    pub fn from_code(code: &str) -> Result<Self, TaxError> {
        match code.to_lowercase().as_str() {
            "standard" => Ok(RateCategory::Standard),
            "reduced" => Ok(RateCategory::Reduced),
            "zero" => Ok(RateCategory::Zero),
            "exempt" => Ok(RateCategory::Exempt),
            _ => Err(TaxError::Invalid {
                reason: format!(
                    "unknown rate category {code:?}, expected standard/reduced/zero/exempt"
                ),
            }),
        }
    }

    /// The category as a string.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            RateCategory::Standard => "standard",
            RateCategory::Reduced => "reduced",
            RateCategory::Zero => "zero",
            RateCategory::Exempt => "exempt",
        }
    }
}

impl std::fmt::Display for RateCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// One tax rate valid for a date range.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DatedRate {
    /// The jurisdiction this rate applies to.
    pub jurisdiction: Jurisdiction,
    /// The rate category (standard, reduced, etc.).
    pub category: RateCategory,
    /// The rate itself (e.g., 0.19 for 19%).
    pub rate: Rate,
    /// The first date this rate is valid (inclusive).
    pub valid_from: Date,
    /// The last date this rate is valid (inclusive), or `None` for open-ended.
    pub valid_to: Option<Date>,
    /// The source of this rate, for audit: e.g.,
    /// "§12 `UStG`; Zweites Corona-Steuerhilfegesetz, `BGBl`. I 2020 S. 1512".
    pub source: String,
}

impl DatedRate {
    /// Check if this rate is valid on the given date.
    #[must_use]
    pub fn is_valid_on(&self, date: Date) -> bool {
        date >= self.valid_from && self.valid_to.is_none_or(|to| date <= to)
    }
}

/// A table of tax rates, validated for gaps and overlaps.
#[derive(Debug)]
pub struct RateTable {
    /// Keyed by `(jurisdiction, category)`, values are sorted by `valid_from`.
    rates: BTreeMap<(Jurisdiction, RateCategory), Vec<DatedRate>>,
}

impl RateTable {
    /// Build a rate table from a list of dated rates. Validates that
    /// each `(jurisdiction, category)` pair has no gaps or overlaps.
    ///
    /// # Errors
    /// [`TaxError::RateTableInvalid`] if any jurisdiction/category pair
    /// has overlapping or gapped date ranges.
    pub fn new(rates: Vec<DatedRate>) -> Result<Self, TaxError> {
        let mut by_key: BTreeMap<(Jurisdiction, RateCategory), Vec<DatedRate>> = BTreeMap::new();

        for rate in rates {
            by_key
                .entry((rate.jurisdiction, rate.category))
                .or_default()
                .push(rate);
        }

        // Validate each sequence for gaps and overlaps.
        for sequences in by_key.values_mut() {
            sequences.sort_by_key(|r| r.valid_from);

            for i in 0..sequences.len() {
                let current = &sequences[i];
                if let Some(next) = sequences.get(i + 1) {
                    if let Some(current_to) = current.valid_to {
                        if next.valid_from <= current_to {
                            // Overlap
                            return Err(TaxError::RateTableInvalid {
                                reason: format!(
                                    "{} {}: overlap between {:?}-{:?} and {:?}-{:?}",
                                    current.jurisdiction,
                                    current.category,
                                    current.valid_from,
                                    current.valid_to,
                                    next.valid_from,
                                    next.valid_to
                                ),
                            });
                        }
                        // Check for gaps: next should start the day after current ends
                        let next_day_after_current =
                            current_to
                                .next_day()
                                .ok_or_else(|| TaxError::RateTableInvalid {
                                    reason: format!(
                                        "{} {}: date overflow checking gap",
                                        current.jurisdiction, current.category
                                    ),
                                })?;
                        if next.valid_from != next_day_after_current {
                            return Err(TaxError::RateTableInvalid {
                                reason: format!(
                                    "{} {}: gap between {:?} and {:?}",
                                    current.jurisdiction,
                                    current.category,
                                    current_to,
                                    next.valid_from
                                ),
                            });
                        }
                    } else {
                        // Open-ended range cannot have a successor.
                        return Err(TaxError::RateTableInvalid {
                            reason: format!(
                                "{} {}: open-ended range must be last",
                                current.jurisdiction, current.category
                            ),
                        });
                    }
                }
            }
        }

        Ok(RateTable { rates: by_key })
    }

    /// Look up the rate for the given jurisdiction, category, and date.
    ///
    /// # Errors
    /// [`TaxError::RateNotFound`] if no rate exists for this combination
    /// or the date is outside the valid range.
    pub fn rate_on(
        &self,
        jurisdiction: Jurisdiction,
        category: RateCategory,
        date: Date,
    ) -> Result<&DatedRate, TaxError> {
        self.rates
            .get(&(jurisdiction, category))
            .and_then(|seqs| seqs.iter().find(|r| r.is_valid_on(date)))
            .ok_or_else(|| TaxError::RateNotFound {
                jurisdiction: jurisdiction.code().to_owned(),
                category: category.code().to_owned(),
                date: date.to_string(),
            })
    }
}

/// Helper to create a German [`DatedRate`].
fn german_rate(
    rate_str: &str,
    category: RateCategory,
    from_y: i32,
    from_m: u8,
    from_d: u8,
    to: Option<(i32, u8, u8)>,
    source: &str,
) -> Result<DatedRate, TaxError> {
    let rate = rate_str
        .parse()
        .map_err(|e: ledgers_money::MoneyError| TaxError::Invalid {
            reason: format!("failed to parse rate {rate_str}: {e}"),
        })?;

    let month_from = time::Month::try_from(from_m).map_err(|_| TaxError::Invalid {
        reason: format!("invalid month {from_m}"),
    })?;
    let valid_from =
        Date::from_calendar_date(from_y, month_from, from_d).map_err(|_| TaxError::Invalid {
            reason: format!("invalid date {from_y:04}-{from_m:02}-{from_d:02}"),
        })?;

    let valid_to = if let Some((to_y, to_m, to_d)) = to {
        let month_to = time::Month::try_from(to_m).map_err(|_| TaxError::Invalid {
            reason: format!("invalid month {to_m}"),
        })?;
        Some(
            Date::from_calendar_date(to_y, month_to, to_d).map_err(|_| TaxError::Invalid {
                reason: format!("invalid date {to_y:04}-{to_m:02}-{to_d:02}"),
            })?,
        )
    } else {
        None
    };

    Ok(DatedRate {
        jurisdiction: Jurisdiction::Germany,
        category,
        rate,
        valid_from,
        valid_to,
        source: source.to_string(),
    })
}

/// Build the German VAT rate table with real historical rates.
///
/// Rates from 2020-07-01 to 2020-12-31 (temporary reduction):
/// - Standard: 16%
/// - Reduced: 5%
///
/// Rates from 2007-01-01 to 2020-06-30 and from 2021-01-01 onwards:
/// - Standard: 19%
/// - Reduced: 7%
///
/// # Errors
///
/// Returns [`TaxError::Invalid`] if rate parsing fails or a date is invalid.
///
/// # Sources
/// - "§12 `UStG`; Zweites Corona-Steuerhilfegesetz, `BGBl`. I 2020 S. 1512" (temporary)
/// - "§12 `UStG`" (standard)
pub fn germany_rates() -> Result<RateTable, TaxError> {
    let rates = vec![
        german_rate(
            "0.19",
            RateCategory::Standard,
            2007,
            1,
            1,
            Some((2020, 6, 30)),
            "§12 UStG",
        )?,
        german_rate(
            "0.07",
            RateCategory::Reduced,
            2007,
            1,
            1,
            Some((2020, 6, 30)),
            "§12 UStG",
        )?,
        german_rate(
            "0.16",
            RateCategory::Standard,
            2020,
            7,
            1,
            Some((2020, 12, 31)),
            "§12 UStG; Zweites Corona-Steuerhilfegesetz, BGBl. I 2020 S. 1512",
        )?,
        german_rate(
            "0.05",
            RateCategory::Reduced,
            2020,
            7,
            1,
            Some((2020, 12, 31)),
            "§12 UStG; Zweites Corona-Steuerhilfegesetz, BGBl. I 2020 S. 1512",
        )?,
        german_rate("0.19", RateCategory::Standard, 2021, 1, 1, None, "§12 UStG")?,
        german_rate("0.07", RateCategory::Reduced, 2021, 1, 1, None, "§12 UStG")?,
        german_rate("0", RateCategory::Zero, 2007, 1, 1, None, "§12 UStG")?,
    ];

    RateTable::new(rates)
}
