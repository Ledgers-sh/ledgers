//! The `OpenAI` connector (issue #142, LLM providers).
//!
//! Cost: the organization costs API, `GET https://api.openai.com/v1/organization/costs`
//! with `start_time` and `end_time` (`Unix` seconds), `bucket_width=1d`,
//! `group_by=project_id`, paged by `page`/`next_page` while `has_more`. Each bucket
//! has `start_time` and `results[]` with `amount: {value, currency}` (a number in
//! dollars, currency lower case) and `project_id` (null when unattributed).
//! Source: <https://platform.openai.com/docs/api-reference/usage/costs>
//!
//! **Needs an admin key.** The costs endpoint is for an organization admin key
//! (Settings, Organization, Admin API keys, created by an owner or admin). A project
//! key cannot read organization costs. The endpoint's reference page could not be
//! checked against a live key when this was written: if `OpenAI` answers 404 for a
//! valid admin key, the sync is recorded as a failed run with that status, and no
//! money is booked.
//!
//! Allocation: by project (`kind: project`). A cost with no project books as
//! `unattributed`. Months are booked once they have ended, as for the other cost
//! connectors.

use crate::connector::{Connector, CredentialField, SyncContext, is_date, next_day};
use crate::cost::{CostLine, date_of_epoch, epoch_seconds, micro_of_value, month_of, output};
use crate::http::{VendorError, percent_encode};
use crate::model::SyncOutput;
use async_trait::async_trait;
use serde_json::Value;

const HOST: &str = "api.openai.com";
const HOSTS: &[&str] = &[HOST];
/// Pages followed per sync.
const MAX_PAGES: u32 = 50;
/// Buckets per page (the endpoint allows up to 180 daily buckets).
const LIMIT: u32 = 31;

/// The `OpenAI` connector.
pub struct OpenAi;

#[async_trait]
impl Connector for OpenAi {
    fn id(&self) -> &'static str {
        "openai"
    }

    fn display_name(&self) -> &'static str {
        "OpenAI"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "Admin API key (organization owner or admin)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, _config: &Value) -> Result<(), String> {
        // The organization costs API has no connection setting; the config is the booking.
        Ok(())
    }

    fn completed_periods_only(&self) -> bool {
        true
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        if !is_date(ctx.since) || !is_date(ctx.until) {
            return Err(malformed("the window is not two dates"));
        }
        let end = next_day(ctx.until).ok_or_else(|| malformed("until is not a date"))?;
        let start_seconds = epoch_seconds(&format!("{}T00:00:00Z", ctx.since))
            .ok_or_else(|| malformed("since is not a date"))?;
        let end_seconds = epoch_seconds(&format!("{end}T00:00:00Z"))
            .ok_or_else(|| malformed("until is not a date"))?;
        let headers = vec![("Authorization", format!("Bearer {}", ctx.credential))];

        let mut lines = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut url = format!(
                "https://{HOST}/v1/organization/costs?start_time={start_seconds}&end_time={end_seconds}&bucket_width=1d&group_by=project_id&limit={LIMIT}"
            );
            if let Some(token) = &page {
                url.push_str("&page=");
                url.push_str(&percent_encode(token));
            }
            let answer = ctx.http.get_json(HOSTS, &url, &headers).await?;
            for bucket in answer
                .get("data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let start = bucket
                    .get("start_time")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| malformed("a bucket has no start_time"))?;
                let day = date_of_epoch(start);
                if day.as_str() < ctx.since || day.as_str() > ctx.until {
                    continue;
                }
                let period =
                    month_of(&day).ok_or_else(|| malformed("a bucket date is not a date"))?;
                for result in bucket
                    .get("results")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    lines.push(cost_line(result, &period)?);
                }
            }
            match answer.get("next_page").and_then(Value::as_str) {
                Some(next)
                    if answer.get("has_more").and_then(Value::as_bool) == Some(true)
                        && !next.is_empty() =>
                {
                    page = Some(next.to_owned());
                }
                _ => break,
            }
        }
        let today = ctx.now.get(0..10).unwrap_or_default();
        let mut notes = Vec::new();
        let mut out = output("openai", "OpenAI", &lines, today, &mut notes);
        notes.push("OpenAI costs are allocated by project; a cost with no project books to the default unit as unattributed".to_owned());
        out.notes = notes;
        Ok(out)
    }
}

/// One cost result as a cost line for `period`.
fn cost_line(result: &Value, period: &str) -> Result<CostLine, VendorError> {
    let amount = result
        .get("amount")
        .ok_or_else(|| malformed("a cost result has no amount"))?;
    let value = amount
        .get("value")
        .ok_or_else(|| malformed("a cost has no value"))?;
    let micro = micro_of_value(value).map_err(|detail| malformed(&detail))?;
    let currency = amount
        .get("currency")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("a cost has no currency"))?
        .to_uppercase();
    let project = result
        .get("project_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or("unattributed");
    Ok(CostLine {
        period: period.to_owned(),
        currency,
        kind: "project",
        id: project.to_owned(),
        name: project.to_owned(),
        micro,
    })
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
