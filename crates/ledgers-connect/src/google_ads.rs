//! The Google Ads connector (monthly campaign cost, one connection per customer account).
//!
//! Cost: the Google Ads API `searchStream` for one customer, with this GAQL:
//!
//! ```text
//! SELECT campaign.id, campaign.name, customer.currency_code, segments.month, metrics.cost_micros
//! FROM campaign WHERE segments.date BETWEEN 'since' AND 'until'
//! ```
//!
//! `segments.month` is the first day of the month; `cost_micros` is an `int64` in
//! millionths of the account currency, read as an exact integer (never through a
//! float). The API version is pinned in [`API_VERSION`].
//!
//! Auth: an OAuth refresh token is exchanged at `oauth2.googleapis.com/token` for an
//! access token on each sync (the token is sent as a form body, never in a URL).
//! Every API call carries `Authorization: Bearer`, the `developer-token` header and,
//! for a manager (MCC) account, `login-customer-id`.
//!
//! Developer token: a production account needs an approved developer token (basic
//! access). A test account works with a test-level token straight away, but its
//! data is test data. The token's refusal is reported as the 401 or 403 it returns.
//!
//! Booking: one invoice per (month, account currency), with the external id
//! `google-ads-cost:{month}:{currency}`, unique per connection. Each campaign is one
//! usage line for its month. The current month is never booked. Re-importing is a
//! no-op.
//!
//! Allocation: as for Meta Ads, allocation rules by campaign id, then the optional
//! `name_prefixes` config key.

use crate::connector::{
    Connector, CredentialField, SyncContext, config_text, credential_text, is_date,
};
use crate::cost::{CostLine, month_of, output};
use crate::http::{VendorError, VendorHttp, form_body, sanitize_vendor_text};
use crate::model::SyncOutput;
use async_trait::async_trait;
use serde_json::Value;

const ADS_HOST: &str = "googleads.googleapis.com";
const TOKEN_HOST: &str = "oauth2.googleapis.com";
/// The Google Ads API version this connector reads. Pinned on purpose.
pub const API_VERSION: &str = "v26";

/// The Google Ads connector.
pub struct GoogleAds;

#[async_trait]
impl Connector for GoogleAds {
    fn id(&self) -> &'static str {
        "google_ads"
    }

    fn display_name(&self) -> &'static str {
        "Google Ads"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "developer_token",
                label: "Developer token",
            },
            CredentialField {
                name: "client_id",
                label: "OAuth client ID",
            },
            CredentialField {
                name: "client_secret",
                label: "OAuth client secret",
            },
            CredentialField {
                name: "refresh_token",
                label: "Refresh token (scope adwords)",
            },
        ]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        &[ADS_HOST, TOKEN_HOST]
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        customer_digits(config_text(config, "customer_id")?)?;
        match config.get("login_customer_id") {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) if text.trim().is_empty() => {}
            Some(Value::String(text)) => {
                customer_digits(text).map_err(|_| {
                    "login_customer_id is the manager account id, 10 digits".to_owned()
                })?;
            }
            Some(_) => return Err("login_customer_id is text".to_owned()),
        }
        crate::allocation::validate_prefixes(config)
    }

    fn completed_periods_only(&self) -> bool {
        true
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let developer_token =
            credential_text(ctx.credential, "developer_token").map_err(|d| malformed(&d))?;
        let client_id = credential_text(ctx.credential, "client_id").map_err(|d| malformed(&d))?;
        let client_secret =
            credential_text(ctx.credential, "client_secret").map_err(|d| malformed(&d))?;
        let refresh_token =
            credential_text(ctx.credential, "refresh_token").map_err(|d| malformed(&d))?;
        let customer =
            customer_digits(config_text(ctx.config, "customer_id").map_err(|d| malformed(&d))?)
                .map_err(|d| malformed(&d))?;
        let login = match ctx.config.get("login_customer_id").and_then(Value::as_str) {
            Some(text) if !text.trim().is_empty() => {
                Some(customer_digits(text).map_err(|d| malformed(&d))?)
            }
            _ => None,
        };
        if !is_date(ctx.since) || !is_date(ctx.until) {
            return Err(malformed("the sync window is not two dates"));
        }

        let access = access_token(ctx.http, &client_id, &client_secret, &refresh_token).await?;
        let mut headers = vec![
            ("Authorization", format!("Bearer {access}")),
            ("developer-token", developer_token),
        ];
        if let Some(login) = login {
            headers.push(("login-customer-id", login));
        }
        let url =
            format!("https://{ADS_HOST}/{API_VERSION}/customers/{customer}/googleAds:searchStream");
        let query = format!(
            "SELECT campaign.id, campaign.name, customer.currency_code, segments.month, metrics.cost_micros FROM campaign WHERE segments.date BETWEEN '{}' AND '{}'",
            ctx.since, ctx.until
        );
        let body = serde_json::json!({ "query": query });
        let answer = ctx
            .http
            .post_json(&[ADS_HOST], &url, &headers, &body)
            .await?;

        let mut lines: Vec<CostLine> = Vec::new();
        let mut skipped = 0_usize;
        let batches = answer
            .as_array()
            .ok_or_else(|| malformed("the search answer is not a list of batches"))?;
        for batch in batches {
            let rows = match batch.get("results") {
                None => continue,
                Some(rows) => rows
                    .as_array()
                    .ok_or_else(|| malformed("a batch has no results list"))?,
            };
            for row in rows {
                match line_of(row) {
                    Some(line) => lines.push(line),
                    None => skipped += 1,
                }
            }
        }

        let today = ctx.now.get(0..10).unwrap_or_default();
        let mut notes = Vec::new();
        let mut out = output("google-ads", "Google Ads", &lines, today, &mut notes);
        for invoice in &mut out.invoices {
            invoice.description = sanitize_vendor_text(&format!(
                "Google Ads campaign cost for customer {customer}, {} in {}",
                invoice.period, invoice.currency
            ));
        }
        if skipped > 0 {
            notes.push(format!(
                "{skipped} result row(s) had no readable cost, currency or month and were skipped"
            ));
        }
        out.notes = notes;
        Ok(out)
    }
}

/// Exchanges the refresh token for an access token. A refused grant (400, 401 or
/// 403 from the token endpoint) is a refused credential, named as such.
async fn access_token(
    http: &VendorHttp,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<String, VendorError> {
    let body = form_body(&[
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ]);
    let answer = http
        .post_form(
            &[TOKEN_HOST],
            &format!("https://{TOKEN_HOST}/token"),
            &[],
            &body,
        )
        .await
        .map_err(|err| match err {
            VendorError::Status { status, .. } if matches!(status, 400 | 401 | 403) => {
                VendorError::Refused {
                    host: TOKEN_HOST.to_owned(),
                    status,
                }
            }
            other => other,
        })?;
    answer
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| malformed("the token answer has no access_token"))
}

/// One result row as a cost line, or `None` when a field is missing or bad. The cost
/// is `costMicros`, an integer in millionths; it is not scaled again.
fn line_of(row: &Value) -> Option<CostLine> {
    let period = row
        .pointer("/segments/month")
        .and_then(Value::as_str)
        .and_then(month_of)?;
    let currency = row
        .pointer("/customer/currencyCode")
        .and_then(Value::as_str)?
        .to_uppercase();
    let micro = match row.pointer("/metrics/costMicros")? {
        Value::String(text) => text.parse::<i128>().ok()?,
        Value::Number(number) => i128::from(number.as_i64()?),
        _ => return None,
    };
    let id = match row.pointer("/campaign/id")? {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    if id.is_empty() || micro < 0 {
        return None;
    }
    let name = row
        .pointer("/campaign/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some(CostLine {
        period,
        currency,
        kind: "campaign",
        id,
        name,
        micro,
    })
}

/// Digits of a customer id, dashes removed: ten digits.
///
/// # Errors
/// A sentence when the id is not 10 digits once dashes are removed.
pub fn customer_digits(text: &str) -> Result<String, String> {
    let digits: String = text.trim().chars().filter(|c| *c != '-').collect();
    if digits.len() == 10 && digits.bytes().all(|b| b.is_ascii_digit()) {
        Ok(digits)
    } else {
        Err("customer_id is the 10-digit Google Ads customer id".to_owned())
    }
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: ADS_HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
