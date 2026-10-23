//! The `PlanetScale` connector (issue #142).
//!
//! Invoices: `GET https://api.planetscale.com/v1/organizations/{organization}/invoices`
//! (`page`, `per_page`; the answer is `{ data: [...], next_page }`). Each invoice
//! has `id`, `total` (a decimal string, "as a stringified decimal to preserve
//! precision"), `billing_period_start`, `billing_period_end`, `paid` and
//! `overdue`. Source: <https://planetscale.com/docs/api/reference/list_invoices.md>
//! The access is `read_invoices`; the service token is sent as
//! `Authorization: <SERVICE_TOKEN_ID>:<SERVICE_TOKEN>`
//! (<https://planetscale.com/docs/api/reference/service-tokens.md>).
//!
//! What the API does not give, and what this connector therefore does not
//! invent:
//! - **Currency.** Invoices carry no currency field; the connection's
//!   `currency` config names it, and the app asks for it at connect time.
//! - **Per-database cost.** Line items (`/invoices/{id}/line-items`) exist, but
//!   their schema is not confirmed, so they are not read. Invoices allocate by
//!   the organization resource (the allocation key is the organization slug),
//!   and per-database allocation is reported as not available for this account.

use crate::connector::{
    Connector, CredentialField, SyncContext, config_text, is_date, month_windows,
};
use crate::http::{VendorError, sanitize_vendor_text};
use crate::model::{InvoiceRecord, SyncOutput, UsageRecord};
use crate::money::amount_from_json;
use async_trait::async_trait;
use ledgers_money::Currency;
use serde_json::Value;

const HOSTS: &[&str] = &["api.planetscale.com"];
const MAX_PAGES: u32 = 20;
const PER_PAGE: u32 = 100;

/// The `PlanetScale` connector.
pub struct PlanetScale;

#[async_trait]
impl Connector for PlanetScale {
    fn id(&self) -> &'static str {
        "planetscale"
    }

    fn display_name(&self) -> &'static str {
        "PlanetScale"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "Service token as ID:token (read_invoices)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        let organization = config_text(config, "organization")?;
        if organization.len() > 100
            || !organization
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("organization is the PlanetScale organization slug".to_owned());
        }
        let currency = config_text(config, "currency")?;
        Currency::from_code(&currency.to_uppercase())
            .map_err(|_| "currency is an ISO 4217 code such as USD".to_owned())?;
        Ok(())
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let organization = config_text(ctx.config, "organization")
            .map_err(|detail| malformed(&detail))?
            .to_owned();
        let currency_code = config_text(ctx.config, "currency")
            .map_err(|detail| malformed(&detail))?
            .to_uppercase();
        let currency = Currency::from_code(&currency_code).map_err(|_| malformed("currency"))?;
        let months = month_windows(ctx.since, ctx.until).map_err(|detail| malformed(&detail))?;
        let headers = vec![("Authorization", ctx.credential.to_owned())];
        let mut out = SyncOutput::default();

        let mut invoices = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = format!(
                "https://api.planetscale.com/v1/organizations/{organization}/invoices?page={page}&per_page={PER_PAGE}"
            );
            let body = ctx.http.get_json(HOSTS, &url, &headers).await?;
            let batch = body
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| malformed("data is not a list"))?;
            invoices.extend(batch.iter().cloned());
            let next = body.get("next_page").and_then(Value::as_u64);
            if next.is_none() || batch.is_empty() {
                break;
            }
        }

        for item in &invoices {
            match invoice_from_item(
                item,
                &organization,
                currency.exponent(),
                &currency_code,
                ctx.since,
                ctx.until,
            ) {
                Ok(Some(invoice)) => out.invoices.push(invoice),
                Ok(None) => {}
                Err(note) => out.notes.push(note),
            }
        }
        for window in &months {
            out.usage.push(UsageRecord {
                period: window.period.clone(),
                resource_kind: "organization".to_owned(),
                resource_id: organization.clone(),
                resource_name: organization.clone(),
                quantity: 1,
            });
        }
        out.notes.push(
            "per-database cost is not available for this account: the invoice line items are not read, so invoices allocate by organization"
                .to_owned(),
        );
        out.notes.push(format!(
            "invoices carry no currency in the API; amounts are booked in {currency_code} as configured"
        ));
        Ok(out)
    }
}

fn invoice_from_item(
    item: &Value,
    organization: &str,
    exponent: u32,
    currency_code: &str,
    since: &str,
    until: &str,
) -> Result<Option<InvoiceRecord>, String> {
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "a PlanetScale invoice has no id and was skipped".to_owned())?;
    let start = item
        .get("billing_period_start")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let day = start.get(0..10).unwrap_or_default();
    if !is_date(day) {
        return Err(format!(
            "invoice {id} has no readable billing period start and was skipped"
        ));
    }
    if day < since || day > until {
        return Ok(None);
    }
    let total = item.get("total").unwrap_or(&Value::Null);
    let minor = amount_from_json(total, exponent)
        .map_err(|_| format!("invoice {id} has a total that is not decimal and was skipped"))?;
    if minor == 0 {
        return Ok(None);
    }
    if minor < 0 {
        return Err(format!(
            "invoice {id} is a credit and is not booked as a bill"
        ));
    }
    let total_minor = i64::try_from(minor)
        .map_err(|_| format!("invoice {id} is out of range and was skipped"))?;
    Ok(Some(InvoiceRecord {
        external_id: format!("planetscale-invoice:{organization}:{id}"),
        vendor_name: "PlanetScale".to_owned(),
        invoice_number: Some(sanitize_vendor_text(id)),
        issued_on: day.to_owned(),
        period: day.get(0..7).unwrap_or_default().to_owned(),
        currency: currency_code.to_owned(),
        total_minor,
        description: sanitize_vendor_text(&format!("PlanetScale invoice {id}")),
        hosted_url: None,
    }))
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOSTS[0].to_owned(),
        detail: detail.to_owned(),
    }
}
