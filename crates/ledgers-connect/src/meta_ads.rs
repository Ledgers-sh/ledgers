//! The Meta Ads connector (monthly ad spend per campaign, one connection per ad account).
//!
//! Cost: the Marketing API insights edge of one ad account,
//! `GET https://graph.facebook.com/v26.0/act_{ad_account_id}/insights` with
//! `level=campaign`, `time_increment=monthly`, `time_range={"since":...,"until":...}`
//! and the fields `spend`, `account_currency`, `campaign_id`, `campaign_name`,
//! `date_start` and `date_stop`. `spend` is a decimal string in the account's
//! currency. Paging follows `paging.next`, which must stay on `graph.facebook.com`.
//!
//! The Graph API version is pinned in [`GRAPH_VERSION`]; a new version is a code
//! change, reviewed like any other.
//!
//! Auth: a system-user access token with the `ads_read` permission only, sent as
//! `Authorization: Bearer`. It is never put in a URL. A token that lacks the
//! permission is refused (401 or 403) and the sync fails with that status named.
//!
//! Booking: one invoice per (month, account currency), with the external id
//! `meta-ads-cost:{month}:{currency}`, which is unique per connection. Each
//! campaign is one usage line for its month (the allocation key is the campaign
//! id, the name is the campaign name). The current month is never booked, because
//! its spend is still moving. Re-importing a month is a no-op.
//!
//! Allocation: campaigns map to business units through the connection's
//! allocation rules. A campaign with no rule can take its unit from the
//! connection's `name_prefixes` (for example `SQZ:SQZ`): the campaign name starts
//! with the prefix followed by a separator.

use crate::connector::{Connector, CredentialField, SyncContext, config_text, credential_text};
use crate::cost::{CostLine, micro_of_value, month_of, output};
use crate::http::{VendorError, percent_encode, sanitize_vendor_text};
use crate::model::SyncOutput;
use async_trait::async_trait;
use serde_json::Value;

const HOST: &str = "graph.facebook.com";
const HOSTS: &[&str] = &[HOST];
/// The Graph API version this connector reads. Pinned on purpose.
pub const GRAPH_VERSION: &str = "v26.0";
/// Rows per page the API is asked for.
const LIMIT: usize = 500;
/// Pages read per sync; a larger answer is noted, not read.
const MAX_PAGES: usize = 20;
/// The fields requested.
const FIELDS: &str = "spend,account_currency,campaign_id,campaign_name,date_start,date_stop";

/// The Meta Ads connector.
pub struct MetaAds;

#[async_trait]
impl Connector for MetaAds {
    fn id(&self) -> &'static str {
        "meta_ads"
    }

    fn display_name(&self) -> &'static str {
        "Meta Ads"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "access_token",
            label: "System-user access token (ads_read only)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        account_digits(config_text(config, "ad_account_id")?)?;
        crate::allocation::validate_prefixes(config)
    }

    fn completed_periods_only(&self) -> bool {
        true
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let token = credential_text(ctx.credential, "access_token").map_err(|d| malformed(&d))?;
        let account =
            account_digits(config_text(ctx.config, "ad_account_id").map_err(|d| malformed(&d))?)
                .map_err(|d| malformed(&d))?;
        let headers = vec![("Authorization", format!("Bearer {token}"))];
        let time_range = format!(
            "{{\"since\":\"{}\",\"until\":\"{}\"}}",
            ctx.since, ctx.until
        );
        let mut url = format!(
            "https://{HOST}/{GRAPH_VERSION}/act_{account}/insights?level=campaign&time_increment=monthly&fields={FIELDS}&time_range={}&limit={LIMIT}",
            percent_encode(&time_range)
        );

        let mut lines: Vec<CostLine> = Vec::new();
        let mut skipped = 0_usize;
        let mut truncated = false;
        for page_number in 1..=MAX_PAGES {
            let page = ctx.http.get_json(HOSTS, &url, &headers).await?;
            let rows = page
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| malformed("the insights answer has no data array"))?;
            for row in rows {
                match line_of(row) {
                    Some(line) => lines.push(line),
                    None => skipped += 1,
                }
            }
            let Some(next) = page
                .pointer("/paging/next")
                .and_then(Value::as_str)
                .filter(|next| !next.is_empty())
            else {
                break;
            };
            if !next.starts_with(&format!("https://{HOST}/{GRAPH_VERSION}/")) {
                return Err(malformed("paging.next leaves the pinned Graph API path"));
            }
            if page_number == MAX_PAGES {
                truncated = true;
                break;
            }
            next.clone_into(&mut url);
        }

        let today = ctx.now.get(0..10).unwrap_or_default();
        let mut notes = Vec::new();
        let mut out = output("meta-ads", "Meta Ads", &lines, today, &mut notes);
        for invoice in &mut out.invoices {
            invoice.description = sanitize_vendor_text(&format!(
                "Meta Ads spend for ad account {account}, {} in {}",
                invoice.period, invoice.currency
            ));
        }
        if truncated {
            notes.push(format!(
                "more than {MAX_PAGES} insight pages; the rest were not read"
            ));
        }
        if skipped > 0 {
            notes.push(format!(
                "{skipped} insight row(s) had no readable spend, currency or month and were skipped"
            ));
        }
        out.notes = notes;
        Ok(out)
    }
}

/// One insight row as a cost line, or `None` when a field is missing or bad.
fn line_of(row: &Value) -> Option<CostLine> {
    let period = row
        .get("date_start")
        .and_then(Value::as_str)
        .and_then(month_of)?;
    let currency = row
        .get("account_currency")
        .and_then(Value::as_str)?
        .to_uppercase();
    let micro = micro_of_value(row.get("spend")?).ok()?;
    let id = row.get("campaign_id").and_then(Value::as_str)?.to_owned();
    let name = row
        .get("campaign_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if id.is_empty() || micro < 0 {
        return None;
    }
    Some(CostLine {
        period,
        currency,
        kind: "campaign",
        id,
        name,
        micro,
    })
}

/// The digits of an ad account id, with or without the `act_` prefix.
///
/// # Errors
/// A sentence when the id is not `act_` plus 5 to 20 digits.
pub fn account_digits(text: &str) -> Result<String, String> {
    let digits = text.trim().strip_prefix("act_").unwrap_or(text.trim());
    if (5..=20).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
        Ok(digits.to_owned())
    } else {
        Err("ad_account_id is the ad account id: act_ and its digits".to_owned())
    }
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
