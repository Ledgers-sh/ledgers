//! The connector contract (issue #142). A connector knows its vendor's API:
//! which credential fields it needs, which hosts it may call, which config it
//! reads, and how to turn one sync window into [`SyncOutput`]. It never
//! writes to the books: the server hands the output to the import command.

use crate::http::{VendorError, VendorHttp};
use crate::model::SyncOutput;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Serialize;

/// One credential field a connection needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CredentialField {
    /// The field name in the API (`token`).
    pub name: &'static str,
    /// The label the app shows.
    pub label: &'static str,
}

/// What a connector gets for one sync.
pub struct SyncContext<'a> {
    /// The vendor client with the host allow-list applied by the connector.
    pub http: &'a VendorHttp,
    /// The decrypted credential, to be sent to the vendor only. A connector
    /// with one field gets the token; one with several gets a JSON object of
    /// the fields (see [`credential_text`]).
    pub credential: &'a str,
    /// The current instant, RFC 3339 UTC (`2026-10-24T10:00:00Z`), for requests
    /// that are signed over a timestamp.
    pub now: &'a str,
    /// The connection's config, validated by [`Connector::validate_config`].
    pub config: &'a serde_json::Value,
    /// The first day of the window, `YYYY-MM-DD` (inclusive).
    pub since: &'a str,
    /// The last day of the window, `YYYY-MM-DD` (inclusive).
    pub until: &'a str,
}

/// A vendor connector.
#[async_trait]
pub trait Connector: Send + Sync {
    /// The stable id stored with a connection (`cloudflare`, `anthropic`, `planetscale`).
    fn id(&self) -> &'static str;

    /// The name the app shows.
    fn display_name(&self) -> &'static str;

    /// The credential fields a connection needs.
    fn credential_fields(&self) -> &'static [CredentialField];

    /// The only hosts this connector may call.
    fn allowed_hosts(&self) -> &'static [&'static str];

    /// Checks the connector-specific config keys.
    ///
    /// # Errors
    /// A sentence naming the bad key.
    fn validate_config(&self, config: &serde_json::Value) -> Result<(), String>;

    /// Whether the vendor's totals for the current month are still moving, so
    /// a sync must only import months that have ended. Anthropic's monthly
    /// cost is final only after the month closes.
    fn completed_periods_only(&self) -> bool {
        false
    }

    /// Reads the vendor for one window.
    ///
    /// # Errors
    /// [`VendorError`]; a data item the vendor does not expose is a note in
    /// the output, not an error, and an unavailable endpoint is
    /// [`VendorError::NotAvailable`].
    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError>;
}

/// The connectors Ledgers ships, in gallery order.
#[must_use]
pub fn all() -> Vec<Box<dyn Connector>> {
    vec![
        Box::new(crate::cloudflare::Cloudflare),
        Box::new(crate::anthropic::Anthropic),
        Box::new(crate::planetscale::PlanetScale),
        Box::new(crate::aws::AwsCostExplorer),
        Box::new(crate::vercel::Vercel),
    ]
}

/// The connector with `id`, if Ledgers ships one.
#[must_use]
pub fn by_id(id: &str) -> Option<Box<dyn Connector>> {
    all().into_iter().find(|connector| connector.id() == id)
}

/// Reads one field of a multi-field credential: the sealed credential is a
/// JSON object such as `{"access_key_id": "...", "secret_access_key": "..."}`.
/// Empty and non-string values are refused; the message names the field, never
/// the value.
///
/// # Errors
/// A sentence naming the missing field.
pub fn credential_text(credential: &str, field: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(credential)
        .map_err(|_| "the stored credential is not in the expected form".to_owned())?;
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("the credential has no {field}"))
}

/// Reads a required string config key.
///
/// # Errors
/// A sentence when the key is missing, empty or not a string.
pub fn config_text<'a>(config: &'a serde_json::Value, key: &str) -> Result<&'a str, String> {
    config
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("the config key {key} is required"))
}

/// One calendar month inside a sync window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonthWindow {
    /// `YYYY-MM`.
    pub period: String,
    /// The first day, `YYYY-MM-DD`.
    pub first: String,
    /// The last day, `YYYY-MM-DD`.
    pub last: String,
}

/// The day after `text` (`YYYY-MM-DD`), or `None` for a bad date.
#[must_use]
pub fn next_day(text: &str) -> Option<String> {
    let (year, month, day) = parse_ymd(text)?;
    let (year, month, day) = if day < days_in_month(year, month) {
        (year, month, day + 1)
    } else if month < 12 {
        (year, month + 1, 1)
    } else {
        (year + 1, 1, 1)
    };
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// The day before `text` (`YYYY-MM-DD`), or `None` for a bad date.
#[must_use]
pub fn previous_day(text: &str) -> Option<String> {
    let (year, month, day) = parse_ymd(text)?;
    let (year, month, day) = if day > 1 {
        (year, month, day - 1)
    } else if month > 1 {
        (year, month - 1, days_in_month(year, month - 1))
    } else {
        (year - 1, 12, 31)
    };
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// The default sync window for a connection, from `today`: three calendar
/// months ending today, or ending with the last complete month when the
/// vendor's totals are final only after a month closes (`completed_only`).
///
/// # Errors
/// A sentence when `today` is not a date.
pub fn default_window(today: &str, completed_only: bool) -> Result<(String, String), String> {
    let (year, month, _) = parse_ymd(today).ok_or_else(|| format!("{today} is not a date"))?;
    let until = if completed_only {
        previous_day(&format!("{year:04}-{month:02}-01"))
            .ok_or_else(|| "no previous month".to_owned())?
    } else {
        today.to_owned()
    };
    let (until_year, until_month, _) =
        parse_ymd(&until).ok_or_else(|| "bad window end".to_owned())?;
    let index = i64::from(until_year) * 12 + i64::from(until_month) - 1 - 2;
    let since_year =
        i32::try_from(index.div_euclid(12)).map_err(|_| "year out of range".to_owned())?;
    let since_month = u32::try_from(index.rem_euclid(12) + 1).unwrap_or(1);
    Ok((format!("{since_year:04}-{since_month:02}-01"), until))
}

/// Whether `text` is a `YYYY-MM-DD` calendar date.
#[must_use]
pub fn is_date(text: &str) -> bool {
    parse_ymd(text).is_some()
}

fn parse_ymd(text: &str) -> Option<(i32, u32, u32)> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i32 = text.get(0..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    Some((year, month, day))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
    }
}

/// The calendar months that `since..=until` touches, in order.
///
/// # Errors
/// A sentence when a bound is not a `YYYY-MM-DD` date or the window is empty.
pub fn month_windows(since: &str, until: &str) -> Result<Vec<MonthWindow>, String> {
    let (sy, sm, _) =
        parse_ymd(since).ok_or_else(|| format!("{since} is not a YYYY-MM-DD date"))?;
    let (uy, um, _) =
        parse_ymd(until).ok_or_else(|| format!("{until} is not a YYYY-MM-DD date"))?;
    if (sy, sm) > (uy, um) {
        return Err("the sync window ends before it starts".to_owned());
    }
    let (mut year, mut month) = (sy, sm);
    let mut out = Vec::new();
    loop {
        out.push(MonthWindow {
            period: format!("{year:04}-{month:02}"),
            first: format!("{year:04}-{month:02}-01"),
            last: format!("{year:04}-{month:02}-{:02}", days_in_month(year, month)),
        });
        if (year, month) == (uy, um) {
            break;
        }
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod window_tests {
    use super::*;

    #[test]
    fn default_windows_cover_three_months_and_respect_completion() {
        assert_eq!(
            default_window("2026-08-23", false).unwrap(),
            ("2026-06-01".to_owned(), "2026-08-23".to_owned())
        );
        assert_eq!(
            default_window("2026-08-23", true).unwrap(),
            ("2026-05-01".to_owned(), "2026-07-31".to_owned())
        );
        assert_eq!(
            default_window("2026-02-03", false).unwrap(),
            ("2025-12-01".to_owned(), "2026-02-03".to_owned())
        );
        assert!(default_window("nope", false).is_err());
    }

    #[test]
    fn previous_day_crosses_months_and_years() {
        assert_eq!(previous_day("2026-03-01").as_deref(), Some("2026-02-28"));
        assert_eq!(previous_day("2026-01-01").as_deref(), Some("2025-12-31"));
        assert_eq!(previous_day("2028-03-01").as_deref(), Some("2028-02-29"));
    }

    #[test]
    fn next_day_crosses_months_and_years() {
        assert_eq!(next_day("2026-08-31").as_deref(), Some("2026-09-01"));
        assert_eq!(next_day("2026-12-31").as_deref(), Some("2027-01-01"));
        assert_eq!(next_day("2028-02-28").as_deref(), Some("2028-02-29"));
        assert_eq!(next_day("nope"), None);
    }

    #[test]
    fn months_cover_the_window() {
        let months = month_windows("2026-07-15", "2026-09-02").unwrap();
        let periods: Vec<&str> = months.iter().map(|m| m.period.as_str()).collect();
        assert_eq!(periods, ["2026-07", "2026-08", "2026-09"]);
        assert_eq!(months[1].last, "2026-08-31");
        assert_eq!(months[2].first, "2026-09-01");
    }

    #[test]
    fn leap_years_and_bad_dates() {
        let months = month_windows("2028-02-01", "2028-02-29").unwrap();
        assert_eq!(months[0].last, "2028-02-29");
        assert!(month_windows("2026-02-30", "2026-03-01").is_err());
        assert!(month_windows("2026-05-01", "2026-04-01").is_err());
        assert!(!is_date("2026-13-01"));
    }
}
