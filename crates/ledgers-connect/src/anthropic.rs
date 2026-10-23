//! The Anthropic connector (issue #142).
//!
//! Cost (the invoice): the Admin API cost report, `GET https://api.anthropic.com/v1/organizations/cost_report` with `starting_at`,
//! `ending_at`, `bucket_width=1d` and paging by `page`/`next_page`. Each bucket
//! has `results[].amount`, a decimal string in the lowest currency unit (cents
//! for USD) and `currency`. Source:
//! <https://platform.claude.com/docs/en/api/beta/organization/cost_report/retrieve>
//!
//! Usage (the allocation key): the Admin API messages usage report, `GET https://api.anthropic.com/v1/organizations/usage_report/messages` grouped by
//! `group_by[]=api_key_id`. Source:
//! <https://platform.claude.com/docs/en/api/beta/organization/usage_report/retrieve_messages>
//! The cost report cannot group by API key (its only groupings are
//! `description` and `workspace_id`), so the month's cost is one invoice and
//! its split follows each API key's token share.
//!
//! Auth: an Admin API key in `x-api-key`, with `anthropic-version`
//! (<https://platform.claude.com/docs/en/api/admin-api>). Admin keys exist only
//! for organizations, not personal accounts. A month is imported only after
//! it has ended, because the cost is still moving until then.

use crate::connector::{Connector, CredentialField, SyncContext, config_text, is_date, next_day};
use crate::http::{VendorError, sanitize_vendor_text};
use crate::model::{InvoiceRecord, SyncOutput, UsageRecord};
use crate::money::parse_decimal;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

const HOSTS: &[&str] = &["api.anthropic.com"];
const VERSION: &str = "2023-06-01";
const MAX_PAGES: u32 = 20;
/// Cost amounts carry five decimals of cents.
const CENT_SCALE: u32 = 5;

/// The Anthropic connector.
pub struct Anthropic;

#[async_trait]
impl Connector for Anthropic {
    fn id(&self) -> &'static str {
        "anthropic"
    }

    fn display_name(&self) -> &'static str {
        "Anthropic"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "Admin API key (organization admin)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        // The Admin API has no per-connection setting; the config is the booking.
        config_text(config, "expense_account").map(|_| ())
    }

    fn completed_periods_only(&self) -> bool {
        true
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let headers = vec![
            ("x-api-key", ctx.credential.to_owned()),
            ("anthropic-version", VERSION.to_owned()),
        ];
        let ending = next_day(ctx.until).ok_or_else(|| malformed("until is not a date"))?;
        if !is_date(ctx.since) {
            return Err(malformed("since is not a date"));
        }
        let mut out = SyncOutput::default();

        // Cost per month, in cents at five decimals, summed exactly.
        let mut cost_by_period: BTreeMap<String, (i128, String)> = BTreeMap::new();
        let mut page: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut url = format!(
                "https://api.anthropic.com/v1/organizations/cost_report?starting_at={}T00:00:00Z&ending_at={ending}T00:00:00Z&bucket_width=1d&limit=31",
                ctx.since
            );
            if let Some(token) = &page {
                url.push_str("&page=");
                url.push_str(token);
            }
            let body = ctx.http.get_json(HOSTS, &url, &headers).await?;
            for bucket in body
                .get("data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let starting = bucket
                    .get("starting_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let day = starting.get(0..10).unwrap_or_default();
                if day < ctx.since || day > ctx.until {
                    continue;
                }
                let period = starting.get(0..7).unwrap_or_default().to_owned();
                for result in bucket
                    .get("results")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let currency = result
                        .get("currency")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if currency != "USD" {
                        return Err(VendorError::NotAvailable(format!(
                            "the cost report is in {currency}; only USD is booked for Anthropic"
                        )));
                    }
                    let amount = result
                        .get("amount")
                        .and_then(Value::as_str)
                        .ok_or_else(|| malformed("a cost result has no amount string"))?;
                    let cents = parse_decimal(amount, CENT_SCALE)
                        .map_err(|err| malformed(&err.to_string()))?;
                    let entry = cost_by_period
                        .entry(period.clone())
                        .or_insert_with(|| (0, currency.to_owned()));
                    entry.0 += cents;
                }
            }
            if body.get("has_more").and_then(Value::as_bool) != Some(true) {
                break;
            }
            page = body
                .get("next_page")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if page.is_none() {
                break;
            }
        }

        for (period, (scaled_cents, currency)) in cost_by_period {
            // Round half up to whole cents, once, after the exact sum.
            let total_minor = (scaled_cents + 50_000).div_euclid(100_000);
            let total_minor =
                i64::try_from(total_minor).map_err(|_| malformed("cost out of range"))?;
            if total_minor == 0 {
                continue;
            }
            out.invoices.push(InvoiceRecord {
                external_id: format!("anthropic-cost:{period}"),
                vendor_name: "Anthropic".to_owned(),
                invoice_number: None,
                issued_on: format!("{period}-01"),
                period: period.clone(),
                currency,
                total_minor,
                description: sanitize_vendor_text(&format!("Anthropic API usage {period}")),
                hosted_url: None,
            });
        }

        usage(ctx, &headers, &ending, &mut out).await?;
        Ok(out)
    }
}

/// Tokens per API key per month, the allocation weights.
async fn usage(
    ctx: &SyncContext<'_>,
    headers: &[(&str, String)],
    ending: &str,
    out: &mut SyncOutput,
) -> Result<(), VendorError> {
    let mut tokens: BTreeMap<(String, String), i64> = BTreeMap::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "https://api.anthropic.com/v1/organizations/usage_report/messages?starting_at={}T00:00:00Z&ending_at={ending}T00:00:00Z&bucket_width=1d&group_by[]=api_key_id&limit=31",
            ctx.since
        );
        if let Some(token) = &page {
            url.push_str("&page=");
            url.push_str(token);
        }
        let body = ctx.http.get_json(HOSTS, &url, headers).await?;
        for bucket in body
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let starting = bucket
                .get("starting_at")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let day = starting.get(0..10).unwrap_or_default();
            if day < ctx.since || day > ctx.until {
                continue;
            }
            let period = starting.get(0..7).unwrap_or_default().to_owned();
            for row in bucket
                .get("results")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let key = row
                    .get("api_key_id")
                    .and_then(Value::as_str)
                    .unwrap_or("console")
                    .to_owned();
                let count = token_total(row);
                let entry = tokens.entry((period.clone(), key)).or_insert(0);
                *entry = entry.saturating_add(count);
            }
        }
        if body.get("has_more").and_then(Value::as_bool) != Some(true) {
            break;
        }
        page = body
            .get("next_page")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if page.is_none() {
            break;
        }
    }
    for ((period, key), quantity) in tokens {
        // The API returns key ids, not names, so suggestions cannot match on a name.
        out.usage.push(UsageRecord {
            period,
            resource_kind: "api-key".to_owned(),
            resource_name: key.clone(),
            resource_id: key,
            quantity,
        });
    }
    Ok(())
}

fn token_total(row: &Value) -> i64 {
    let field = |name: &str| row.get(name).and_then(Value::as_i64).unwrap_or(0);
    let cache = row.get("cache_creation");
    let cache_field = |name: &str| {
        cache
            .and_then(|value| value.get(name))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };
    [
        field("uncached_input_tokens"),
        field("cache_read_input_tokens"),
        field("output_tokens"),
        cache_field("ephemeral_5m_input_tokens"),
        cache_field("ephemeral_1h_input_tokens"),
    ]
    .iter()
    .fold(0_i64, |sum, part| sum.saturating_add(*part))
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOSTS[0].to_owned(),
        detail: detail.to_owned(),
    }
}
