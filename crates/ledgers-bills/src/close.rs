//! Close check for flagged extraction fields (issue #21).
//!
//! A bill blocks close if it falls within the period being closed and is still
//! awaiting review (not posted, not discarded) and has a flagged extraction:
//! - any failed check in `checks_json`, or
//! - any confidence below the low-confidence threshold in `confidence_json`, or
//! - non-empty `instruction_flags`.

use cratefield_core::{DbError, Row, Statement};
use ledgers_extract::LOW_CONFIDENCE_THRESHOLD;
use ledgers_ledger::close::{Blocker, CloseCheck, CloseCheckContext};
use ledgers_ledger::error::AccountsError;
use ledgers_ledger::journal_port::BoxFuture;
use sea_query::Value;
use std::collections::BTreeMap;

/// Check if a bill row is flagged for extraction issues. JSON parse errors are propagated
/// as errors; NULL or empty columns remain unflagged.
fn is_bill_flagged(row: &Row) -> Result<bool, AccountsError> {
    // Check if any check failed
    if let Some(checks_str) = row.get::<String>("checks_json")
        && !checks_str.is_empty()
    {
        let checks: Vec<serde_json::Value> = serde_json::from_str(&checks_str)
            .map_err(|e| AccountsError::Database(DbError::Query(format!("checks_json: {e}"))))?;
        if checks
            .iter()
            .any(|check| check.get("ok").and_then(serde_json::Value::as_bool) == Some(false))
        {
            return Ok(true);
        }
    }

    // Check if any field has low confidence
    if let Some(conf_str) = row.get::<String>("confidence_json")
        && !conf_str.is_empty()
        && conf_str != "{}"
    {
        let confidence: BTreeMap<String, u8> = serde_json::from_str(&conf_str).map_err(|e| {
            AccountsError::Database(DbError::Query(format!("confidence_json: {e}")))
        })?;
        if confidence
            .values()
            .any(|&conf| conf < LOW_CONFIDENCE_THRESHOLD)
        {
            return Ok(true);
        }
    }

    // Check if instruction_flags is non-empty
    if let Some(flags_str) = row.get::<String>("instruction_flags")
        && !flags_str.is_empty()
        && flags_str != "[]"
    {
        let flags: Vec<String> = serde_json::from_str(&flags_str).map_err(|e| {
            AccountsError::Database(DbError::Query(format!("instruction_flags: {e}")))
        })?;
        if !flags.is_empty() {
            return Ok(true);
        }
    }

    Ok(false)
}

/// The close check for flagged extraction fields.
pub struct FlaggedExtractionCloseCheck;

impl CloseCheck for FlaggedExtractionCloseCheck {
    fn id(&self) -> &'static str {
        "bills.flagged-extraction"
    }

    fn blockers<'a>(
        &'a self,
        ctx: CloseCheckContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<Blocker>, AccountsError>> {
        Box::pin(async move {
            // Query bills in the period awaiting review (not posted, not discarded).
            // Fall back to creation date (first 10 chars of ISO-8601 created_at) if invoice_date is NULL.
            let stmt = Statement::with_values(
                "SELECT b.checks_json, b.confidence_json, b.instruction_flags \
                 FROM bill b \
                 WHERE b.company_id = ? AND COALESCE(b.invoice_date, substr(b.created_at, 1, 10)) >= ? \
                   AND COALESCE(b.invoice_date, substr(b.created_at, 1, 10)) <= ? \
                   AND b.posted_entry_id IS NULL AND b.discarded_at IS NULL"
                    .to_string(),
                vec![
                    Value::from(ctx.company_id),
                    Value::from(ctx.starts_on),
                    Value::from(ctx.ends_on),
                ],
            );

            let rows = ctx.db.query(&stmt).await?;

            let mut flagged_count = 0i64;
            for row in rows.rows {
                if is_bill_flagged(&row)? {
                    flagged_count += 1;
                }
            }

            if flagged_count == 0 {
                return Ok(Vec::new());
            }

            Ok(vec![Blocker {
                check_id: self.id().to_owned(),
                kind: "flagged-extraction".to_owned(),
                count: flagged_count,
                link: format!("/companies/{}/bills?status=needs-review", ctx.company_id),
            }])
        })
    }
}
