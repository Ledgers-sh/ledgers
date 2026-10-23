//! The Cloudflare connector (issue #142).
//!
//! Invoices: the account billing history, `GET https://api.cloudflare.com/client/v4/accounts/{account_id}/billing/history`
//! (paginated with `page` and `per_page`; each item carries `id`, `amount`
//! (a number, major units), `currency`, `occurred_at`, `description`,
//! `invoice_id` and `hosted_invoice_url`). Source:
//! <https://developers.cloudflare.com/api/resources/billing/subresources/history/methods/list>
//! The token needs the `Billing Read` permission.
//!
//! Usage (the allocation key): the GraphQL Analytics API, `POST https://api.cloudflare.com/client/v4/graphql`, under
//! `viewer.accounts(filter: {accountTag})`:
//! - `workersInvocationsAdaptive`, `sum { requests }` by `dimensions { scriptName }`
//!   with a `datetime` filter (query shape from
//!   <https://developers.cloudflare.com/analytics/graphql-api/tutorials/querying-workers-metrics/>);
//! - `d1AnalyticsAdaptiveGroups`, `sum { readQueries writeQueries }` by
//!   `dimensions { databaseId }` with a `date` filter (dataset list:
//!   <https://developers.cloudflare.com/analytics/graphql-api/features/data-sets/>).
//!
//! Not read: R2 operations and storage. The dataset name for R2 in the
//! official reference was not confirmed for this account, so R2 spend is
//! booked to the default unit with a note in the run log instead of guessed
//! usage. The token needs `Account Analytics: Read` for the GraphQL nodes.
//! Only the documented names above are sent.

use crate::connector::{
    Connector, CredentialField, MonthWindow, SyncContext, config_text, month_windows,
};
use crate::http::{VendorError, sanitize_vendor_text};
use crate::model::{InvoiceRecord, SyncOutput, UsageRecord};
use crate::money::amount_from_json;
use async_trait::async_trait;
use ledgers_money::Currency;
use serde_json::{Value, json};

const REST: &str = "https://api.cloudflare.com/client/v4";
const HOSTS: &[&str] = &["api.cloudflare.com"];
const MAX_PAGES: u32 = 40;
const PER_PAGE: u32 = 50;

/// The GraphQL query for one month of Workers requests and D1 queries.
const USAGE_QUERY: &str = "query Usage($accountTag: string!, $start: Time!, $end: Time!, $first: Date!, $last: Date!) {
  viewer {
    accounts(filter: { accountTag: $accountTag }) {
      workersInvocationsAdaptive(limit: 10000, filter: { datetime_geq: $start, datetime_leq: $end }) {
        sum { requests }
        dimensions { scriptName }
      }
      d1AnalyticsAdaptiveGroups(limit: 10000, filter: { date_geq: $first, date_leq: $last }) {
        sum { readQueries writeQueries }
        dimensions { databaseId }
      }
    }
  }
}";

/// The Cloudflare connector.
pub struct Cloudflare;

#[async_trait]
impl Connector for Cloudflare {
    fn id(&self) -> &'static str {
        "cloudflare"
    }

    fn display_name(&self) -> &'static str {
        "Cloudflare"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "API token (Billing Read, Account Analytics Read)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        let account = config_text(config, "account_id")?;
        if account.len() > 32 || !account.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("account_id is the 32-character Cloudflare account id".to_owned());
        }
        Ok(())
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let account = config_text(ctx.config, "account_id")
            .map_err(|detail| VendorError::Malformed {
                host: HOSTS[0].to_owned(),
                detail,
            })?
            .to_owned();
        let auth = vec![("Authorization", format!("Bearer {}", ctx.credential))];
        let mut out = SyncOutput::default();
        let months =
            month_windows(ctx.since, ctx.until).map_err(|detail| VendorError::Malformed {
                host: HOSTS[0].to_owned(),
                detail,
            })?;

        let items = billing_history(ctx, &account, &auth).await?;
        for item in items {
            match invoice_from_item(&item, ctx.since, ctx.until) {
                Ok(Some(invoice)) => out.invoices.push(invoice),
                Ok(None) => {}
                Err(note) => out.notes.push(note),
            }
        }

        for window in &months {
            usage_for_month(ctx, &account, &auth, window, &mut out).await?;
        }
        out.notes.push(
            "R2 operations and storage are not read: the dataset is not confirmed for this account; R2 spend books to the default unit."
                .to_owned(),
        );
        Ok(out)
    }
}

async fn billing_history(
    ctx: &SyncContext<'_>,
    account: &str,
    auth: &[(&str, String)],
) -> Result<Vec<Value>, VendorError> {
    let mut items = Vec::new();
    for page in 1..=MAX_PAGES {
        let url =
            format!("{REST}/accounts/{account}/billing/history?page={page}&per_page={PER_PAGE}");
        let body = ctx.http.get_json(HOSTS, &url, auth).await?;
        let batch = body
            .get("result")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("result is not a list"))?;
        let done = batch.len() < PER_PAGE as usize;
        items.extend(batch.iter().cloned());
        if done {
            break;
        }
    }
    Ok(items)
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOSTS[0].to_owned(),
        detail: detail.to_owned(),
    }
}

/// One billing item as an invoice, when it falls in the window. `Ok(None)` for
/// items outside the window or zero amounts; `Err` with a note for items that
/// are not invoices Ledgers can book (credits, unknown currency).
fn invoice_from_item(
    item: &Value,
    since: &str,
    until: &str,
) -> Result<Option<InvoiceRecord>, String> {
    let occurred = item
        .get("occurred_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let issued_on = occurred.get(0..10).unwrap_or_default();
    if !crate::connector::is_date(issued_on) || issued_on < since || issued_on > until {
        return Ok(None);
    }
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| item.get("invoice_id").and_then(Value::as_str))
        .ok_or_else(|| format!("a billing item on {issued_on} has no id and was skipped"))?;
    let currency_code = item
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_uppercase();
    let currency = Currency::from_code(&currency_code)
        .map_err(|_| format!("billing item {id} has no usable currency and was skipped"))?;
    let amount = item.get("amount").unwrap_or(&Value::Null);
    let minor = amount_from_json(amount, currency.exponent()).map_err(|_| {
        format!("billing item {id} has an amount that is not decimal and was skipped")
    })?;
    if minor == 0 {
        return Ok(None);
    }
    if minor < 0 {
        return Err(format!(
            "billing item {id} is a credit ({issued_on}) and is not booked as a bill"
        ));
    }
    let total_minor = i64::try_from(minor)
        .map_err(|_| format!("billing item {id} is out of range and was skipped"))?;
    let description = item
        .get("description")
        .and_then(Value::as_str)
        .or_else(|| item.get("action").and_then(Value::as_str))
        .unwrap_or("Cloudflare billing item");
    Ok(Some(InvoiceRecord {
        external_id: format!("cloudflare-billing:{id}"),
        vendor_name: "Cloudflare, Inc.".to_owned(),
        invoice_number: item
            .get("external_invoice_id")
            .and_then(Value::as_str)
            .map(sanitize_vendor_text)
            .filter(|value| !value.is_empty()),
        issued_on: issued_on.to_owned(),
        period: issued_on.get(0..7).unwrap_or_default().to_owned(),
        currency: currency.code().to_owned(),
        total_minor,
        description: sanitize_vendor_text(description),
        hosted_url: item
            .get("hosted_invoice_url")
            .and_then(Value::as_str)
            .filter(|url| url.starts_with("https://"))
            .map(str::to_owned),
    }))
}

async fn usage_for_month(
    ctx: &SyncContext<'_>,
    account: &str,
    auth: &[(&str, String)],
    window: &MonthWindow,
    out: &mut SyncOutput,
) -> Result<(), VendorError> {
    let body = json!({
        "query": USAGE_QUERY,
        "variables": {
            "accountTag": account,
            "start": format!("{}T00:00:00Z", window.first),
            "end": format!("{}T23:59:59Z", window.last),
            "first": window.first,
            "last": window.last,
        }
    });
    let answer = ctx
        .http
        .post_json(HOSTS, &format!("{REST}/graphql"), auth, &body)
        .await?;
    if let Some(errors) = answer.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        let message = errors[0]
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the analytics query failed");
        out.notes.push(format!(
            "usage for {} is not available: {}",
            window.period,
            sanitize_vendor_text(message)
        ));
        return Ok(());
    }
    let accounts = answer
        .pointer("/data/viewer/accounts")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("viewer.accounts is missing"))?;
    let Some(account_data) = accounts.first() else {
        out.notes
            .push(format!("no analytics account for {}", window.period));
        return Ok(());
    };
    for row in account_data
        .get("workersInvocationsAdaptive")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let script = row
            .pointer("/dimensions/scriptName")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("a Workers row has no scriptName"))?;
        let requests = row
            .pointer("/sum/requests")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        out.usage.push(UsageRecord {
            period: window.period.clone(),
            resource_kind: "worker".to_owned(),
            resource_id: script.to_owned(),
            resource_name: sanitize_vendor_text(script),
            quantity: requests,
        });
    }
    for row in account_data
        .get("d1AnalyticsAdaptiveGroups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let database = row
            .pointer("/dimensions/databaseId")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("a D1 row has no databaseId"))?;
        let reads = row
            .pointer("/sum/readQueries")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let writes = row
            .pointer("/sum/writeQueries")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        out.usage.push(UsageRecord {
            period: window.period.clone(),
            resource_kind: "d1-database".to_owned(),
            resource_id: database.to_owned(),
            resource_name: database.to_owned(),
            quantity: reads.saturating_add(writes),
        });
    }
    Ok(())
}
