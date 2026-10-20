//! Lookup rates by date within a configurable window.

use crate::error::FxError;
use cratefield_core::{Database, Statement};
use ledgers_money::Rate;
use sea_query::Value as SeaValue;
use std::str::FromStr;
use time::Date;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RateSource {
    Manual,
    Csv,
    Ecb,
}

impl RateSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Csv => "csv",
            Self::Ecb => "ecb",
        }
    }
}

impl FromStr for RateSource {
    type Err = FxError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "manual" => Ok(Self::Manual),
            "csv" => Ok(Self::Csv),
            "ecb" => Ok(Self::Ecb),
            _ => Err(FxError::InvalidRate {
                reason: "unknown source".to_owned(),
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExchangeRate {
    pub company_id: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub rate_date: Date,
    pub rate: Rate,
    pub source: RateSource,
    pub recorded_at: String,
}

#[derive(Copy, Clone, Debug)]
pub struct LookupWindow {
    pub days: i64,
}

impl LookupWindow {
    pub const DEFAULT: i64 = 7;
    #[must_use]
    pub fn new(days: i64) -> Self {
        LookupWindow { days }
    }
}

impl Default for LookupWindow {
    fn default() -> Self {
        LookupWindow {
            days: Self::DEFAULT,
        }
    }
}

/// Look up rate on date or within window.
///
/// # Errors
/// [`FxError::NoRateInWindow`], [`FxError::SameCurrency`], or [`FxError::Database`].
pub async fn lookup(
    db: &dyn Database,
    company_id: &str,
    base: &str,
    quote: &str,
    date: Date,
    window: LookupWindow,
) -> Result<ExchangeRate, FxError> {
    if base == quote {
        return Err(FxError::SameCurrency {
            currency: base.to_owned(),
        });
    }

    let date_str = date.to_string();
    let window_start = date
        .checked_sub(time::Duration::days(window.days))
        .unwrap_or(date)
        .to_string();

    let rows = db
        .query(&Statement::with_values(
            "SELECT rate_scaled, source, recorded_at FROM exchange_rate \
             WHERE company_id = ? AND base_currency = ? AND quote_currency = ? \
               AND rate_date <= ? AND rate_date >= ? \
             ORDER BY rate_date DESC LIMIT 1"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(base),
                SeaValue::from(quote),
                SeaValue::from(date_str.as_str()),
                SeaValue::from(window_start.as_str()),
            ],
        ))
        .await
        .map_err(|e| FxError::Database {
            reason: e.to_string(),
        })?;

    if let Some(row) = rows.first() {
        let rate_scaled_str =
            row.get::<String>("rate_scaled")
                .ok_or_else(|| FxError::Database {
                    reason: "rate_scaled missing".to_owned(),
                })?;
        let rate_scaled_i128 = rate_scaled_str
            .parse::<i128>()
            .map_err(|_| FxError::Database {
                reason: format!("rate_scaled is not a valid i128: {rate_scaled_str}"),
            })?;
        let source_str = row
            .get::<String>("source")
            .ok_or_else(|| FxError::Database {
                reason: "source missing".to_owned(),
            })?;
        let recorded_at = row
            .get::<String>("recorded_at")
            .ok_or_else(|| FxError::Database {
                reason: "recorded_at missing".to_owned(),
            })?;

        Ok(ExchangeRate {
            company_id: company_id.to_owned(),
            base_currency: base.to_owned(),
            quote_currency: quote.to_owned(),
            rate_date: date,
            rate: Rate::from_scaled(rate_scaled_i128),
            source: RateSource::from_str(&source_str)?,
            recorded_at,
        })
    } else {
        Err(FxError::NoRateInWindow {
            base: base.to_owned(),
            quote: quote.to_owned(),
            date: date_str,
            window: window.days,
        })
    }
}
