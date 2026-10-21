//! Rolling daily accounting: company-day boundaries and consumption statements.
//!
//! A rolling daily limit is checked within the company's local calendar day.
//! This module converts UTC timestamps to the company's local day, and generates
//! guard statements for per-currency, per-day consumption.

use cratefield_core::Statement;
use ledgers_db::limit::daily_consume_statement;
use ledgers_money::Currency;
use std::collections::BTreeMap;
use thiserror::Error;
use time::{OffsetDateTime, UtcOffset};

/// Errors for daily limit consumption.
#[derive(Clone, Debug, Error)]
pub enum LimitError {
    /// A currency is missing a limit (fail-closed).
    #[error("no limit defined for currency {0}")]
    MissingCurrencyLimit(String),

    /// An amount exceeds u64 range (overflow).
    #[error("amount overflow: {0}")]
    AmountOverflow(String),
}

/// Converts `now` to the company's local calendar day, returned as a UTC
/// timestamp at midnight of that day (for the harness `Period::Day` bucket).
///
/// The company operates in `offset` (e.g., +02:00 for Europe/Berlin). This
/// function converts `now` to that offset, extracts the calendar date, and
/// returns it as a UTC timestamp at midnight.
///
/// # Example
/// - now = 2026-04-10T23:30:00Z (UTC), offset = +02:00 (Europe/Berlin local)
///   → local date is 2026-04-11
///   → returned as 2026-04-11T00:00:00Z (labeled UTC but represents company day)
///
/// # Panics
/// Panics if the offset is out of range (should never happen for valid IANA zones).
#[must_use]
pub fn company_day(now: OffsetDateTime, offset: UtcOffset) -> OffsetDateTime {
    let local = now.to_offset(offset);
    let date = local.date();

    // Re-construct midnight (00:00:00) on that date, then re-label as UTC
    // so the harness Period::Day recognizes it.
    time::PrimitiveDateTime::new(date, time::Time::MIDNIGHT).assume_utc()
}

/// The meter name for posting limit consumption.
/// Returns a string like `"post:EUR"` for the harness `Usage` subject.
#[must_use]
pub fn meter(currency: &Currency) -> String {
    format!("post:{}", currency.code())
}

/// Generates guard statements to consume the rolling daily limit for each
/// currency in the effect.
///
/// For each currency with a non-zero effect and a defined `per_day` limit,
/// this emits a [`daily_consume_statement`] guarded upsert that either admits
/// the amount or fails the whole batch (`NOT NULL` on `used`).
///
/// A currency with no limit → [`LimitError::MissingCurrencyLimit`] (fail closed).
///
/// # Errors
///
/// Returns an error if a currency has no limit or an amount overflows u64.
pub fn consume_statements(
    key_id: &str,
    effect: &BTreeMap<String, i128>,
    now: OffsetDateTime,
    offset: UtcOffset,
    limits: &BTreeMap<String, crate::CurrencyLimit>,
) -> Result<Vec<Statement>, LimitError> {
    let mut statements = Vec::new();

    for (code, amount) in effect {
        // Only consume if amount > 0.
        if *amount <= 0 {
            continue;
        }

        // Look up the currency and limit.
        let currency = Currency::from_code(code)
            .map_err(|_| LimitError::MissingCurrencyLimit(format!("invalid currency: {code}")))?;

        let limit = limits.get(code).ok_or_else(|| {
            LimitError::MissingCurrencyLimit(format!(
                "no limit for currency {code} for key {key_id}"
            ))
        })?;

        // Only consume if the limit is non-zero.
        if limit.per_day == 0 {
            return Err(LimitError::MissingCurrencyLimit(format!(
                "no limit for currency {code} for key {key_id}"
            )));
        }

        // Convert amount to u64 (checked).
        let amount_u64: u64 = (*amount).try_into().map_err(|_| {
            LimitError::AmountOverflow(format!("amount {amount} for {code} exceeds u64 range"))
        })?;

        let limit_u64: u64 = (limit.per_day).try_into().map_err(|_| {
            LimitError::AmountOverflow(format!(
                "per_day limit {amount} for {code} exceeds u64 range"
            ))
        })?;

        let day = company_day(now, offset);
        let m = meter(&currency);

        statements.push(daily_consume_statement(
            key_id, &m, day, amount_u64, limit_u64,
        ));
    }

    Ok(statements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn company_day_same_day_utc() {
        // 2026-04-10T12:00:00Z with UTC offset → same day
        let now = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, time::Month::April, 10).expect("valid date"),
            time::Time::from_hms(12, 0, 0).expect("valid time"),
        )
        .assume_utc();

        let day = company_day(now, UtcOffset::UTC);
        assert_eq!(day.date().day(), 10);
        assert_eq!(day.time(), time::Time::MIDNIGHT);
    }

    #[test]
    fn company_day_next_day_positive_offset() {
        // 2026-04-10T23:30:00Z with +02:00 → 2026-04-11 locally
        let now = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, time::Month::April, 10).expect("valid date"),
            time::Time::from_hms(23, 30, 0).expect("valid time"),
        )
        .assume_utc();

        let offset = UtcOffset::from_hms(2, 0, 0).expect("valid offset");
        let day = company_day(now, offset);
        assert_eq!(day.date().day(), 11);
        assert_eq!(day.time(), time::Time::MIDNIGHT);
    }

    #[test]
    fn company_day_previous_day_negative_offset() {
        // 2026-04-10T04:00:00Z with -05:00 → 2026-04-09 locally
        let now = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, time::Month::April, 10).expect("valid date"),
            time::Time::from_hms(4, 0, 0).expect("valid time"),
        )
        .assume_utc();

        let offset = UtcOffset::from_hms(-5, 0, 0).expect("valid offset");
        let day = company_day(now, offset);
        assert_eq!(day.date().day(), 9);
        assert_eq!(day.time(), time::Time::MIDNIGHT);
    }

    #[test]
    fn meter_name() {
        let eur = Currency::from_code("EUR").expect("valid currency");
        assert_eq!(meter(&eur), "post:EUR");

        let usd = Currency::from_code("USD").expect("valid currency");
        assert_eq!(meter(&usd), "post:USD");
    }

    #[test]
    fn consume_statements_empty_effect() {
        let now = time::OffsetDateTime::now_utc();
        let offset = UtcOffset::UTC;
        let effect = BTreeMap::new();
        let limits = BTreeMap::new();

        let result = consume_statements("key-1", &effect, now, offset, &limits);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn consume_statements_missing_currency_limit() {
        let now = time::OffsetDateTime::now_utc();
        let offset = UtcOffset::UTC;
        let mut effect = BTreeMap::new();
        effect.insert("EUR".to_string(), 10_000);

        let limits = BTreeMap::new();

        let result = consume_statements("key-1", &effect, now, offset, &limits);
        assert!(result.is_err());
        assert!(matches!(result, Err(LimitError::MissingCurrencyLimit(_))));
    }

    #[test]
    fn consume_statements_zero_limit() {
        let now = time::OffsetDateTime::now_utc();
        let offset = UtcOffset::UTC;
        let mut effect = BTreeMap::new();
        effect.insert("EUR".to_string(), 10_000);

        let mut limits = BTreeMap::new();
        limits.insert(
            "EUR".to_string(),
            crate::CurrencyLimit {
                per_entry: 10_000,
                per_day: 0,
            },
        );

        let result = consume_statements("key-1", &effect, now, offset, &limits);
        assert!(result.is_err());
    }

    #[test]
    fn consume_statements_ok() {
        let now = time::OffsetDateTime::now_utc();
        let offset = UtcOffset::UTC;
        let mut effect = BTreeMap::new();
        effect.insert("EUR".to_string(), 10_000);

        let mut limits = BTreeMap::new();
        limits.insert(
            "EUR".to_string(),
            crate::CurrencyLimit {
                per_entry: 10_000,
                per_day: 100_000,
            },
        );

        let result = consume_statements("key-1", &effect, now, offset, &limits);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);
    }
}
