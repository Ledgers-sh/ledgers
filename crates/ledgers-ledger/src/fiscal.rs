//! The fiscal-year model: pure calendar arithmetic, no I/O — docs/PLAN.md
//! §6, the `ledgers-ledger` row ("Chart of accounts, journals, entries,
//! periods, reversals, numbering, dimensions"): this module is the
//! periods part's calendar. Storage and commands live in
//! [`crate::period`]; nothing here touches the database.
//!
//! # Period keys are calendar months
//!
//! A period id is the calendar `"YYYY-MM"` of the month it contains (or
//! `"{year}-13"` for a fiscal year's adjustment period), **not** a
//! fiscal ordinal. That is what makes the pieces compose:
//! `ledgers_db::period::period_of_date` reduces any `"YYYY-MM-DD"`
//! posting date to the same spelling, so the posting guard, the close
//! and reopen surface and the audit payloads all address one
//! `accounting_period` row with no translation layer. A fiscal year may
//! therefore span calendar years — start month 7, label 2025 runs its
//! periods `"2025-07"` .. `"2026-06"` — and [`period_ids`] returns
//! those calendar ids in fiscal order.
//!
//! The **label** is the calendar year the fiscal year starts in:
//! [`starts_on`] `(label, start_month)` is `label-start_month-01` and
//! [`ends_on`] is the day before the next year starts.
//!
//! # The adjustment period
//!
//! With [`FiscalYearSpec::has_adjustment_period`] the year carries a
//! 13th period, `"YYYY-13"` in the calendar year of [`ends_on`]. It has
//! no calendar days of its own — month 12 already ends exactly on
//! `ends_on` — so a date-derived period can never name it; entries land
//! there only through an explicit period override on the entry, and
//! [`period_range`] gives it the single-date range
//! `(ends_on, ends_on)`.
//!
//! All functions are total: they compute in `i64` internally, so no
//! ordering of the (validated) inputs can underflow, and none of them
//! panic.

use crate::error::AccountsError;
use cratefield_core::Row;

/// The smallest fiscal-year label [`FiscalYearSpec::validate`] accepts.
pub const MIN_LABEL: i64 = 1900;

/// The largest fiscal-year label [`FiscalYearSpec::validate`] accepts.
/// The next year must still be addressable (`label + 1`), so 9999 is the
/// last year that can be closed into.
pub const MAX_LABEL: i64 = 9999;

/// The system tag naming the retained-earnings account the year-end
/// close posts its balancing line to (see [`crate::lint`] for the tag
/// grammar; this is the one tag this crate reads itself).
pub const RETAINED_EARNINGS_TAG: &str = "retained-earnings";

/// What the caller chooses when opening a fiscal year: everything on the
/// `fiscal_year` row that is not derived or bookkeeping.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FiscalYearSpec {
    /// The calendar year the fiscal year starts in
    /// ([`MIN_LABEL`]..=[`MAX_LABEL`]).
    pub label: i64,
    /// The calendar month the year starts in (1..=12): 1 is a calendar
    /// year, 7 a July-to-June year, and so on.
    pub start_month: u32,
    /// Whether the year carries a 13th adjustment period
    /// (`"{year}-13"`, in the calendar year of [`ends_on`]).
    pub has_adjustment_period: bool,
}

impl FiscalYearSpec {
    /// Validates the spec: `start_month` in 1..=12 and `label` in
    /// [`MIN_LABEL`]..=[`MAX_LABEL`]. Everything else in this module is
    /// arithmetic over an already-valid spec.
    ///
    /// # Errors
    ///
    /// [`AccountsError::InvalidFiscalYear`] naming the offending field
    /// and its bounds when either value is out of range.
    pub fn validate(&self) -> Result<(), AccountsError> {
        if !(1..=12).contains(&self.start_month) {
            return Err(AccountsError::InvalidFiscalYear {
                reason: format!("start_month must be 1..=12, got {}", self.start_month),
            });
        }
        if !(MIN_LABEL..=MAX_LABEL).contains(&self.label) {
            return Err(AccountsError::InvalidFiscalYear {
                reason: format!(
                    "label must be {MIN_LABEL}..={MAX_LABEL}, got {}",
                    self.label
                ),
            });
        }
        Ok(())
    }
}

/// One stored `fiscal_year` row: the spec plus the derived span and the
/// year-end close's bookkeeping columns.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FiscalYear {
    /// The company this year belongs to (no cross-module foreign key —
    /// see the migration comments).
    pub company_id: String,
    /// The calendar year the fiscal year starts in.
    pub label: i64,
    /// The calendar month the year starts in (1..=12).
    pub start_month: u32,
    /// Whether the year carries a 13th adjustment period.
    pub has_adjustment_period: bool,
    /// Inclusive `"YYYY-MM-DD"` first day ([`starts_on`]).
    pub starts_on: String,
    /// Inclusive `"YYYY-MM-DD"` last day ([`ends_on`]).
    pub ends_on: String,
    /// ISO 8601 UTC timestamp of the year-end close; `None` until the
    /// close commits. A set value is the idempotency marker: closing
    /// again returns the stored report and writes nothing.
    pub year_end_closed_at: Option<String>,
    /// The closing entry's id once the year-end close posted it.
    pub closing_entry_id: Option<String>,
    /// The serialized [`crate::period::YearEndReport`] of the close
    /// (single-line `key=value` text).
    pub year_end_report: Option<String>,
    /// ISO 8601 UTC creation timestamp.
    pub created_at: String,
    /// ISO 8601 UTC last-update timestamp.
    pub updated_at: String,
}

impl FiscalYear {
    /// The spec this row was opened under — the inverse of the command
    /// layer's spec-to-row construction, so period arithmetic on a read
    /// row goes through the same [`period_ids`] / [`period_range`]
    /// functions as a spec the caller just validated.
    #[must_use]
    pub fn spec(&self) -> FiscalYearSpec {
        FiscalYearSpec {
            label: self.label,
            start_month: self.start_month,
            has_adjustment_period: self.has_adjustment_period,
        }
    }

    /// Maps one result row of the `fiscal_year` table to a [`FiscalYear`].
    /// Stored values that cannot be re-typed surface as errors rather
    /// than silently defaulted ones — the same discipline as
    /// [`crate::store`]'s row mapping.
    ///
    /// # Errors
    ///
    /// [`AccountsError::Database`] when a required column is missing or
    /// holds the wrong type (the columns are written by this crate, so
    /// this is schema drift, not a runtime condition).
    pub fn from_row(row: &Row) -> Result<Self, AccountsError> {
        let required = |column: &str| -> Result<String, AccountsError> {
            row.get::<String>(column).ok_or_else(|| {
                AccountsError::Database(cratefield_core::DbError::Query(format!(
                    "column {column} missing or not text"
                )))
            })
        };
        let required_i64 = |column: &str| -> Result<i64, AccountsError> {
            row.get::<i64>(column).ok_or_else(|| {
                AccountsError::Database(cratefield_core::DbError::Query(format!(
                    "column {column} missing or not an integer"
                )))
            })
        };
        let start_month = required_i64("start_month")?;
        if !(1..=12).contains(&start_month) {
            return Err(AccountsError::Database(cratefield_core::DbError::Query(
                format!("column start_month out of 1..=12: {start_month}"),
            )));
        }
        Ok(Self {
            company_id: required("company_id")?,
            label: required_i64("label")?,
            start_month: u32::try_from(start_month).map_err(|_| {
                AccountsError::Database(cratefield_core::DbError::Query(
                    "column start_month negative".to_owned(),
                ))
            })?,
            has_adjustment_period: required_i64("has_adjustment_period")? != 0,
            starts_on: required("starts_on")?,
            ends_on: required("ends_on")?,
            year_end_closed_at: row.get::<Option<String>>("year_end_closed_at").flatten(),
            closing_entry_id: row.get::<Option<String>>("closing_entry_id").flatten(),
            year_end_report: row.get::<Option<String>>("year_end_report").flatten(),
            created_at: required("created_at")?,
            updated_at: required("updated_at")?,
        })
    }
}

/// The period id of fiscal month `month` (1..=13) under `spec`: the
/// calendar `"YYYY-MM"` the month falls in, or `"{year}-13"` for the
/// adjustment period (calendar year of [`ends_on`]). For months 1..=12
/// the calendar month shifts by `month - 1` from `start_month`, carrying
/// into the next calendar year as needed.
#[must_use]
pub fn period_id(spec: &FiscalYearSpec, month: u32) -> String {
    if month == 13 {
        // The adjustment period sits after month 12, whose calendar year
        // is the year of ends_on: label itself for a calendar year,
        // label + 1 for any other start month.
        let year = spec.label + i64::from(spec.start_month != 1);
        return format!("{year:04}-13");
    }
    let (year, calendar) = calendar_of(spec, month);
    format!("{year:04}-{calendar:02}")
}

/// The period ids of `spec` in fiscal order: 12 entries, 13 when the
/// year carries an adjustment period. These are the periods
/// [`crate::period::open_fiscal_year`] materializes and the order the
/// year-end close walks them in.
#[must_use]
pub fn period_ids(spec: &FiscalYearSpec) -> Vec<String> {
    let mut ids = Vec::with_capacity(if spec.has_adjustment_period { 13 } else { 12 });
    for month in 1..=12 {
        ids.push(period_id(spec, month));
    }
    if spec.has_adjustment_period {
        ids.push(period_id(spec, 13));
    }
    ids
}

/// Parses a period id back to its `(year, month)` pair: calendar months
/// `"YYYY-MM"` with month 1..=12, or the adjustment spelling
/// `"YYYY-13"`. `None` for anything else (wrong shape, month 0, 14+).
/// The inverse of [`period_id`].
#[must_use]
pub fn parse_period_id(s: &str) -> Option<(i64, u32)> {
    let bytes = s.as_bytes();
    if bytes.len() != 7 || bytes[4] != b'-' {
        return None;
    }
    let digits = (0..4).chain(5..7);
    if digits
        .into_iter()
        .any(|index| !bytes[index].is_ascii_digit())
    {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    if !(1..=13).contains(&month) {
        return None;
    }
    Some((year, month))
}

/// The inclusive first day of the fiscal year: `label-start_month-01`.
#[must_use]
pub fn starts_on(label: i64, start_month: u32) -> String {
    format!("{label:04}-{}-01", month_2(start_month))
}

/// The inclusive last day of the fiscal year: the day before the next
/// year's start, i.e. the last day of the calendar month before
/// `start_month` in the year the next year starts — leap-year correct
/// through [`days_in_month`].
#[must_use]
pub fn ends_on(label: i64, start_month: u32) -> String {
    // The month before next year's start: December of `label` for a
    // calendar year, else `start_month - 1` of `label + 1`.
    let (year, month) = if start_month == 1 {
        (label, 12)
    } else {
        (label + 1, start_month - 1)
    };
    format!(
        "{year:04}-{}-{}",
        month_2(month),
        days_in_month(year, month)
    )
}

/// The inclusive `"YYYY-MM-DD"` date range of fiscal month `month`
/// (1..=13) under `spec`: the whole calendar month for 1..=12, and
/// `(ends_on, ends_on)` for the adjustment period, which has no calendar
/// days of its own (see the module docs).
#[must_use]
pub fn period_range(spec: &FiscalYearSpec, month: u32) -> (String, String) {
    if month == 13 {
        let last = ends_on(spec.label, spec.start_month);
        return (last.clone(), last);
    }
    let (year, calendar) = calendar_of(spec, month);
    calendar_month_range(year, calendar)
}

/// The inclusive `"YYYY-MM-DD"` range of one bare calendar month — the
/// range [`crate::period::close_period`] scans for a `"YYYY-MM"` period
/// without needing the fiscal year that owns it.
#[must_use]
pub fn calendar_month_range(year: i64, month: u32) -> (String, String) {
    (
        format!("{year:04}-{}-01", month_2(month)),
        format!(
            "{year:04}-{}-{}",
            month_2(month),
            days_in_month(year, month)
        ),
    )
}

/// The calendar `(year, month)` fiscal month `month` (1..=12) of `spec`
/// falls in: `start_month` shifted by `month - 1`, in `i64` so no input
/// ordering can underflow.
fn calendar_of(spec: &FiscalYearSpec, month: u32) -> (i64, u32) {
    let offset = i64::from(spec.start_month) - 1 + (i64::from(month) - 1);
    let year = spec.label + offset.div_euclid(12);
    let calendar = u32::try_from(offset.rem_euclid(12) + 1).unwrap_or(1);
    (year, calendar)
}

/// Days in `month` (1..=12) of `year`, Gregorian leap rules included;
/// `0` for a month outside 1..=12 (callers validate; the shape keeps the
/// formatting functions total).
#[must_use]
pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// The Gregorian leap rule: every 4th year except centuries not divisible
/// by 400.
#[must_use]
pub fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Two-digit month formatting (plain `1` → `"01"`), total for any `u32`.
fn month_2(month: u32) -> String {
    format!("{month:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(label: i64, start_month: u32, adjustment: bool) -> FiscalYearSpec {
        FiscalYearSpec {
            label,
            start_month,
            has_adjustment_period: adjustment,
        }
    }

    #[test]
    fn validation_bounds_month_and_label() {
        assert!(spec(2026, 1, false).validate().is_ok());
        assert!(spec(2026, 12, true).validate().is_ok());
        assert!(spec(2026, 0, false).validate().is_err(), "month 0");
        assert!(spec(2026, 13, false).validate().is_err(), "month 13 start");
        assert!(spec(1899, 1, false).validate().is_err(), "label 1899");
        assert!(spec(10_000, 1, false).validate().is_err(), "label 10000");
        let err = spec(1899, 13, false).validate().unwrap_err();
        assert!(
            matches!(err, AccountsError::InvalidFiscalYear { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_calendar_year_spans_the_whole_label_year() {
        assert_eq!(starts_on(2026, 1), "2026-01-01");
        assert_eq!(ends_on(2026, 1), "2026-12-31");
        let ids = period_ids(&spec(2026, 1, false));
        assert_eq!(ids.len(), 12);
        assert_eq!(ids[0], "2026-01");
        assert_eq!(ids[11], "2026-12");
        assert_eq!(ids[3], "2026-04");
    }

    #[test]
    fn a_july_year_spans_two_calendar_years() {
        assert_eq!(starts_on(2025, 7), "2025-07-01");
        assert_eq!(ends_on(2025, 7), "2026-06-30");
        let ids = period_ids(&spec(2025, 7, false));
        assert_eq!(ids.len(), 12);
        assert_eq!(ids[0], "2025-07", "July first");
        assert_eq!(ids[5], "2025-12", "December is fiscal month 6");
        assert_eq!(ids[6], "2026-01", "January is fiscal month 7");
        assert_eq!(ids[11], "2026-06", "June last");
        // period_of_date composes: any date in the span reduces to a
        // period id in the set.
        for date in ["2025-07-01", "2025-12-31", "2026-01-01", "2026-06-30"] {
            let period = ledgers_db::period::period_of_date(date).expect("shaped date");
            assert!(ids.contains(&period), "{date} → {period} not in {ids:?}");
        }
    }

    #[test]
    fn a_december_start_ends_in_november_next_year() {
        assert_eq!(starts_on(2026, 12), "2026-12-01");
        assert_eq!(ends_on(2026, 12), "2027-11-30");
        let ids = period_ids(&spec(2026, 12, false));
        assert_eq!(ids[0], "2026-12");
        assert_eq!(ids[11], "2027-11");
    }

    #[test]
    fn ends_on_is_leap_year_correct() {
        // Leap February: a March-starting year ends on Feb 29 in 2027-28
        // (2028 is leap), on Feb 28 when it is not.
        assert_eq!(ends_on(2027, 3), "2028-02-29");
        assert_eq!(ends_on(2026, 3), "2027-02-28");
        // Calendar years ending in February hit the leap year directly.
        assert_eq!(ends_on(2023, 1), "2023-12-31");
        assert_eq!(calendar_month_range(2024, 2).1, "2024-02-29");
        assert_eq!(calendar_month_range(2023, 2).1, "2023-02-28");
        assert_eq!(calendar_month_range(2000, 2).1, "2000-02-29");
        assert_eq!(calendar_month_range(1900, 2).1, "1900-02-28");
        assert_eq!(calendar_month_range(2100, 2).1, "2100-02-28");
        assert_eq!(calendar_month_range(2026, 4).0, "2026-04-01");
        assert_eq!(calendar_month_range(2026, 4).1, "2026-04-30");
    }

    #[test]
    fn the_adjustment_period_is_month_thirteen() {
        // Calendar year: "YYYY-13" in the label year.
        let ids = period_ids(&spec(2026, 1, true));
        assert_eq!(ids.len(), 13);
        assert_eq!(ids[12], "2026-13");
        // July year: the label's end falls in the next calendar year.
        let ids = period_ids(&spec(2026, 7, true));
        assert_eq!(ids.len(), 13);
        assert_eq!(ids[11], "2027-06");
        assert_eq!(ids[12], "2027-13");
        // December-start year ends in November; adjustment still takes
        // the year of ends_on.
        assert_eq!(period_ids(&spec(2026, 12, true))[12], "2027-13");
        // Its range is the single year-end date.
        let s = spec(2026, 7, true);
        assert_eq!(
            period_range(&s, 13),
            ("2027-06-30".to_owned(), "2027-06-30".to_owned())
        );
    }

    #[test]
    fn period_ranges_cover_their_calendar_months() {
        let s = spec(2025, 7, false);
        assert_eq!(
            period_range(&s, 1),
            ("2025-07-01".to_owned(), "2025-07-31".to_owned())
        );
        assert_eq!(
            period_range(&s, 6),
            ("2025-12-01".to_owned(), "2025-12-31".to_owned())
        );
        assert_eq!(
            period_range(&s, 7),
            ("2026-01-01".to_owned(), "2026-01-31".to_owned())
        );
        assert_eq!(
            period_range(&spec(2026, 1, false), 2),
            ("2026-02-01".to_owned(), "2026-02-28".to_owned())
        );
        // The twelve ranges tile the span exactly.
        let s = spec(2025, 7, false);
        assert_eq!(period_range(&s, 1).0, starts_on(2025, 7));
        assert_eq!(period_range(&s, 12).1, ends_on(2025, 7));
    }

    #[test]
    fn period_ids_round_trip_through_parse() {
        for s in [
            spec(2026, 1, false),
            spec(2025, 7, false),
            spec(2026, 12, true),
            spec(2026, 7, true),
        ] {
            for (index, id) in period_ids(&s).into_iter().enumerate() {
                let fiscal_month = u32::try_from(index + 1).expect("12 or 13 months");
                let parsed = parse_period_id(&id).unwrap_or_else(|| panic!("{id} must parse"));
                // Fiscal month 1 falls in the start month; each later
                // fiscal month is one calendar month on (mod 12), and
                // the adjustment period is month 13 outright.
                let expected_month = if fiscal_month == 13 {
                    13
                } else {
                    (s.start_month + fiscal_month - 2) % 12 + 1
                };
                assert_eq!(
                    parsed.1, expected_month,
                    "{id} is fiscal month {fiscal_month}"
                );
                // The calendar year is the label or the one after it —
                // a fiscal year never reaches further.
                assert!(
                    parsed.0 == s.label || parsed.0 == s.label + 1,
                    "{id} outside {}..={}",
                    s.label,
                    s.label + 1
                );
                assert_eq!(id, period_id(&s, fiscal_month));
            }
        }
    }

    #[test]
    fn malformed_period_ids_do_not_parse() {
        for bad in [
            "", "2026", "2026-1", "2026-111", "20261", "2026/01", "abcd-ef", "2026-00", "2026-14",
            "2026-99", "2026-13x",
        ] {
            assert_eq!(parse_period_id(bad), None, "{bad:?} must not parse");
        }
        assert_eq!(parse_period_id("2026-13"), Some((2026, 13)));
        assert_eq!(parse_period_id("1900-01"), Some((1900, 1)));
    }

    #[test]
    fn leap_years_follow_the_gregorian_rule() {
        assert!(is_leap_year(2024));
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(2023));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2100));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);
        assert_eq!(days_in_month(2026, 12), 31);
        assert_eq!(days_in_month(2026, 13), 0, "out-of-range months shape as 0");
    }

    #[test]
    fn the_spec_round_trips_through_the_row() {
        let column = |name: &str, value: sea_query::Value| (name.to_owned(), value);
        let row = Row::new(vec![
            column(
                "company_id",
                sea_query::Value::from("test-company-000000000000"),
            ),
            column("label", sea_query::Value::BigInt(Some(2026))),
            column("start_month", sea_query::Value::BigInt(Some(7))),
            column("has_adjustment_period", sea_query::Value::BigInt(Some(1))),
            column("starts_on", sea_query::Value::from("2025-07-01")),
            column("ends_on", sea_query::Value::from("2026-06-30")),
            column("year_end_closed_at", sea_query::Value::String(None)),
            column("closing_entry_id", sea_query::Value::String(None)),
            column("year_end_report", sea_query::Value::String(None)),
            column("created_at", sea_query::Value::from("2025-06-30T10:00:00Z")),
            column("updated_at", sea_query::Value::from("2025-06-30T10:00:00Z")),
        ]);
        let year = FiscalYear::from_row(&row).expect("a full row maps");
        assert_eq!(year.company_id, "test-company-000000000000");
        assert_eq!(year.label, 2026);
        assert_eq!(year.start_month, 7);
        assert!(year.has_adjustment_period);
        assert_eq!(year.year_end_closed_at, None);
        assert_eq!(year.spec(), spec(2026, 7, true));
    }
}
