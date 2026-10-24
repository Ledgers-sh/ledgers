//! The `OpenRouter` connector (issue #142, LLM providers).
//!
//! Cost: the activity endpoint, `GET https://openrouter.ai/api/v1/activity?date=YYYY-MM-DD`,
//! one request per UTC day. Each row has `usage` (the spend in dollars, a number),
//! `requests`, `model`, `provider_name` and `endpoint_id`. The endpoint covers only
//! the last 30 completed UTC days, and it needs a **management key**. Source:
//! <https://openrouter.ai/docs/api/api-reference/analytics/get-user-activity>
//!
//! What is booked. A completed day is final once it has ended, and the activity
//! endpoint keeps only 30 days, so a whole month is never in reach. Each completed
//! day in the window is therefore one invoice, `openrouter-usage:{date}:USD`, that
//! is booked once and then skipped on every later sync.
//!
//! What is not booked yet. Allocation by model needs usage weights per day, and the
//! framework keeps usage per month. `OpenRouter` days therefore write no usage rows,
//! and their spend books to the connection's default unit with a note that says so.
//! Per-model totals for each day are shown in the run notes.
//!
//! Test connection. The test window is today, which is not completed. The connector
//! then reads yesterday's activity to prove the key, and books nothing.

use crate::connector::{Connector, CredentialField, SyncContext, is_date, next_day, previous_day};
use crate::cost::{micro_of_value, to_minor};
use crate::http::{VendorError, percent_encode};
use crate::model::{InvoiceRecord, SyncOutput};
use async_trait::async_trait;
use serde_json::Value;

const HOST: &str = "openrouter.ai";
const HOSTS: &[&str] = &[HOST];
/// The activity endpoint keeps this many completed days.
const RETAINED_DAYS: u32 = 30;

/// The `OpenRouter` connector.
pub struct OpenRouter;

#[async_trait]
impl Connector for OpenRouter {
    fn id(&self) -> &'static str {
        "openrouter"
    }

    fn display_name(&self) -> &'static str {
        "OpenRouter"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "Management API key",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, _config: &Value) -> Result<(), String> {
        Ok(())
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        if !is_date(ctx.since) || !is_date(ctx.until) {
            return Err(malformed("the window is not two dates"));
        }
        let today = ctx
            .now
            .get(0..10)
            .ok_or_else(|| malformed("the clock is not RFC 3339"))?
            .to_owned();
        let yesterday = previous_day(&today).ok_or_else(|| malformed("the clock is not a date"))?;
        let oldest = (0..RETAINED_DAYS).try_fold(yesterday.clone(), |day, _| {
            previous_day(&day).ok_or_else(|| malformed("the clock is not a date"))
        })?;

        // The completed days the window covers, within the retained 30. A window with
        // none (a test run for today) reads yesterday, to prove the key, and books nothing.
        let mut days = Vec::new();
        let mut day = ctx.since.to_owned();
        while day.as_str() <= ctx.until && day.as_str() <= yesterday.as_str() {
            if day.as_str() >= oldest.as_str() {
                days.push(day.clone());
            }
            day = next_day(&day).ok_or_else(|| malformed("a day is not a date"))?;
        }
        let probe = days.is_empty();
        if probe {
            days.push(yesterday.clone());
        }

        let headers = vec![("Authorization", format!("Bearer {}", ctx.credential))];
        let mut out = SyncOutput::default();
        for day in &days {
            let url = format!(
                "https://{HOST}/api/v1/activity?date={}",
                percent_encode(day)
            );
            let answer = ctx.http.get_json(HOSTS, &url, &headers).await?;
            let rows = answer
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| malformed("the activity answer has no data"))?;
            let mut micro: i128 = 0;
            let mut requests: i64 = 0;
            let mut models: Vec<String> = Vec::new();
            for row in rows {
                let usage = row
                    .get("usage")
                    .ok_or_else(|| malformed("an activity row has no usage"))?;
                micro += micro_of_value(usage).map_err(|detail| malformed(&detail))?;
                requests += row.get("requests").and_then(Value::as_i64).unwrap_or(0);
                if let Some(model) = row.get("model").and_then(Value::as_str)
                    && !models.iter().any(|known| known == model)
                {
                    models.push(model.to_owned());
                }
            }
            let booked = probe || (day.as_str() >= ctx.since && day.as_str() <= ctx.until);
            if !booked {
                continue;
            }
            let total = to_minor(micro, 2);
            if total < 0 {
                out.notes
                    .push(format!("{day}: the activity is negative and is not booked"));
                continue;
            }
            out.notes.push(format!(
                "{day}: {requests} request(s) across {} model(s)",
                models.len()
            ));
            if probe || total == 0 {
                continue;
            }
            let total_minor =
                i64::try_from(total).map_err(|_| malformed("a day's spend is out of range"))?;
            let period = day.get(0..7).unwrap_or_default().to_owned();
            out.invoices.push(InvoiceRecord {
                external_id: format!("openrouter-usage:{day}:USD"),
                vendor_name: "OpenRouter".to_owned(),
                invoice_number: None,
                issued_on: day.clone(),
                period,
                currency: "USD".to_owned(),
                total_minor,
                description: format!("OpenRouter usage {day}"),
                hosted_url: None,
            });
        }
        out.notes.push(
            "OpenRouter days book to the default unit: the activity endpoint gives per-model cost by day, and no per-day usage weights exist yet"
                .to_owned(),
        );
        Ok(out)
    }
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
