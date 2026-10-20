//! Record rates: batched writes, optional audit.

use crate::error::FxError;
use crate::lookup::RateSource;
use cratefield_core::{Database, DbError, Statement};
use ledgers_audit::{AuditConfig, AuditContext, AuditRecord, Outcome, append_statements};
use ledgers_db::chain;
use ledgers_db::retry::{self, RetryPolicy};
use ledgers_money::Rate;
use sea_query::Value as SeaValue;
use std::time::Duration;
use time::Date;

#[derive(Clone, Debug)]
pub struct RateToRecord {
    pub base: String,
    pub quote: String,
    pub date: Date,
    pub rate: Rate,
}

pub struct ManualRateParams {
    pub company_id: String,
    pub base: String,
    pub quote: String,
    pub date: Date,
    pub rate: Rate,
    pub at: String,
}

/// Record rates: all-or-nothing batches. No audit (see [`record_manual_rate`] for audited writes).
///
/// # Errors
/// [`FxError::InvalidRate`] or [`FxError::Database`].
pub async fn record_rates(
    db: &dyn Database,
    company_id: &str,
    rates: &[RateToRecord],
    source: RateSource,
    at: &str,
) -> Result<(), FxError> {
    if rates.is_empty() {
        return Ok(());
    }

    let source_str = source.as_str();

    // Build all the insert/upsert statements
    let mut statements = Vec::with_capacity(rates.len());
    for rate in rates {
        let rate_scaled = rate.rate.scaled().to_string();

        statements.push(Statement::with_values(
            "INSERT INTO exchange_rate \
             (company_id, base_currency, quote_currency, rate_date, rate_scaled, source, recorded_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(company_id, base_currency, quote_currency, rate_date) \
             DO UPDATE SET rate_scaled = excluded.rate_scaled, source = excluded.source, recorded_at = excluded.recorded_at"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(rate.base.as_str()),
                SeaValue::from(rate.quote.as_str()),
                SeaValue::from(rate.date.to_string().as_str()),
                SeaValue::from(rate_scaled.as_str()),
                SeaValue::from(source_str),
                SeaValue::from(at),
            ],
        ));
    }

    db.batch_atomic(&statements)
        .await
        .map_err(|e| FxError::Database {
            reason: e.to_string(),
        })?;

    Ok(())
}

/// Record a manual rate with audit event, atomically.
///
/// # Errors
/// [`FxError::InvalidRate`] or [`FxError::Database`].
pub async fn record_manual_rate(
    db: &dyn Database,
    params: ManualRateParams,
    audit: &AuditContext,
    audit_config: &AuditConfig,
) -> Result<(), FxError> {
    // Perform the upsert and audit in one batch.
    let input = serde_json::json!({
        "base": params.base,
        "quote": params.quote,
        "date": params.date.to_string(),
        "rate": params.rate.to_string(),
    });
    let audited = AuditRecord::from_input(
        "fx.record_manual_rate",
        &input,
        Outcome::Allowed,
        vec![params.company_id.clone()],
        None,
    );

    retry::run_batch_with_retry(
        db,
        RetryPolicy::default(),
        || {
            let audit = audit.clone();
            let audited = audited.clone();
            let params_clone = ManualRateParams {
                company_id: params.company_id.clone(),
                base: params.base.clone(),
                quote: params.quote.clone(),
                date: params.date,
                rate: params.rate,
                at: params.at.clone(),
            };

            async move {
                let head = chain::read_head(db, &params_clone.company_id).await?;
                let appended = append_statements(
                    &params_clone.company_id,
                    &head,
                    &audit,
                    &audited,
                    &params_clone.at,
                    audit_config,
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

                let rate_scaled = params_clone.rate.scaled().to_string();

                let upsert = Statement::with_values(
                    "INSERT INTO exchange_rate \
                     (company_id, base_currency, quote_currency, rate_date, rate_scaled, source, recorded_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(company_id, base_currency, quote_currency, rate_date) \
                     DO UPDATE SET rate_scaled = excluded.rate_scaled, source = excluded.source, recorded_at = excluded.recorded_at"
                        .to_owned(),
                    vec![
                        SeaValue::from(params_clone.company_id.as_str()),
                        SeaValue::from(params_clone.base.as_str()),
                        SeaValue::from(params_clone.quote.as_str()),
                        SeaValue::from(params_clone.date.to_string().as_str()),
                        SeaValue::from(rate_scaled.as_str()),
                        SeaValue::from("manual"),
                        SeaValue::from(params_clone.at.as_str()),
                    ],
                );

                let mut statements = vec![upsert];
                statements.extend(appended.statements);
                Ok(statements)
            }
        },
        no_sleep,
    )
    .await
    .map_err(|e| FxError::Database {
        reason: e.to_string(),
    })?;

    Ok(())
}

// Dummy sleep function for retry compatibility.
fn no_sleep(_: Duration) {}
