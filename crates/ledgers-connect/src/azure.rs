//! The Microsoft Azure connector (issue #142, cloud providers).
//!
//! Token: the Entra ID client-credentials flow,
//! `POST https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token` with
//! `scope=https://management.azure.com/.default`. The app registration needs the
//! **Cost Management Reader** role on the subscription.
//!
//! Cost: the Cost Management query at subscription scope,
//! `POST https://management.azure.com/subscriptions/{id}/providers/Microsoft.CostManagement/query?api-version=2025-03-01`,
//! one request per calendar month, `type: Usage` (actual cost), `granularity: None`,
//! aggregated as `PreTaxCost` and grouped by `ResourceGroup`. The answer's columns
//! are matched by name and its rows are positional; `properties.nextLink` pages.
//! Source: <https://learn.microsoft.com/rest/api/cost-management/query/usage>
//!
//! Allocation: by resource group (`kind: resource-group`). A cost with no resource
//! group books to `unattributed`. Costs sum per month and currency.

use crate::connector::{
    Connector, CredentialField, SyncContext, config_text, credential_text, month_windows,
};
use crate::cost::{CostLine, micro_of_value, output};
use crate::http::{VendorError, form_body, percent_encode};
use crate::model::SyncOutput;
use async_trait::async_trait;
use serde_json::{Value, json};

const LOGIN_HOST: &str = "login.microsoftonline.com";
const MANAGEMENT_HOST: &str = "management.azure.com";
const HOSTS: &[&str] = &[LOGIN_HOST, MANAGEMENT_HOST];
const API_VERSION: &str = "2025-03-01";
const SCOPE: &str = "https://management.azure.com/.default";
/// Pages followed per month.
const MAX_PAGES: u32 = 50;

/// The Azure Cost Management connector.
pub struct AzureCost;

#[async_trait]
impl Connector for AzureCost {
    fn id(&self) -> &'static str {
        "azure"
    }

    fn display_name(&self) -> &'static str {
        "Microsoft Azure"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "client_secret",
            label: "Client secret of the app registration (Cost Management Reader)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        guid(
            config,
            "tenant_id",
            "tenant_id is the Entra ID tenant (a GUID)",
        )?;
        guid(
            config,
            "client_id",
            "client_id is the app registration's client ID (a GUID)",
        )?;
        guid(
            config,
            "subscription_id",
            "subscription_id is the subscription to read (a GUID)",
        )?;
        Ok(())
    }

    fn completed_periods_only(&self) -> bool {
        true
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let secret = credential_text(ctx.credential, "client_secret")
            .map_err(|detail| malformed(&detail))?;
        let tenant = config_text(ctx.config, "tenant_id").map_err(|detail| malformed(&detail))?;
        let client = config_text(ctx.config, "client_id").map_err(|detail| malformed(&detail))?;
        let subscription =
            config_text(ctx.config, "subscription_id").map_err(|detail| malformed(&detail))?;
        if !is_guid(tenant) || !is_guid(client) || !is_guid(subscription) {
            return Err(malformed(
                "the tenant, client or subscription id is not a GUID",
            ));
        }
        let months = month_windows(ctx.since, ctx.until).map_err(|detail| malformed(&detail))?;
        let token = access_token(ctx, tenant, client, &secret).await?;
        let headers = vec![("Authorization", format!("Bearer {token}"))];

        let mut lines = Vec::new();
        let mut skipped = 0_usize;
        for month in &months {
            let url = format!(
                "https://{MANAGEMENT_HOST}/subscriptions/{}/providers/Microsoft.CostManagement/query?api-version={API_VERSION}",
                percent_encode(subscription)
            );
            let body = json!({
                "type": "Usage",
                "timeframe": "Custom",
                "timePeriod": {"from": format!("{}T00:00:00Z", month.first), "to": format!("{}T23:59:59Z", month.last)},
                "dataset": {
                    "granularity": "None",
                    "aggregation": {"totalCost": {"name": "PreTaxCost", "function": "Sum"}},
                    "grouping": [{"type": "Dimension", "name": "ResourceGroup"}]
                }
            });
            let mut answer = ctx.http.post_json(HOSTS, &url, &headers, &body).await?;
            for _ in 0..MAX_PAGES {
                read_page(&answer, &month.period, &mut lines, &mut skipped)?;
                match answer
                    .pointer("/properties/nextLink")
                    .and_then(Value::as_str)
                {
                    Some(next) if !next.is_empty() => {
                        answer = ctx.http.post_json(HOSTS, next, &headers, &body).await?;
                    }
                    _ => break,
                }
            }
        }
        let today = ctx.now.get(0..10).unwrap_or_default();
        let mut notes = Vec::new();
        let mut out = output("azure", "Microsoft Azure", &lines, today, &mut notes);
        if skipped > 0 {
            notes.push(format!(
                "{skipped} row(s) had no readable cost or currency and were skipped"
            ));
        }
        notes.push("allocated by resource group; a cost with no resource group books to the default unit as unattributed".to_owned());
        out.notes = notes;
        Ok(out)
    }
}

fn guid(config: &Value, key: &str, message: &str) -> Result<(), String> {
    match config_text(config, key) {
        Ok(text) if is_guid(text) => Ok(()),
        _ => Err(message.to_owned()),
    }
}

/// An 8-4-4-4-12 hexadecimal GUID.
fn is_guid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

async fn access_token(
    ctx: &SyncContext<'_>,
    tenant: &str,
    client: &str,
    secret: &str,
) -> Result<String, VendorError> {
    let url = format!(
        "https://{LOGIN_HOST}/{}/oauth2/v2.0/token",
        percent_encode(tenant)
    );
    let body = form_body(&[
        ("client_id", client),
        ("client_secret", secret),
        ("grant_type", "client_credentials"),
        ("scope", SCOPE),
    ]);
    let answer = ctx.http.post_form(HOSTS, &url, &[], &body).await?;
    answer
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| malformed("the token answer has no access_token"))
}

/// One page of a query answer: rows matched to the columns by name.
fn read_page(
    answer: &Value,
    period: &str,
    lines: &mut Vec<CostLine>,
    skipped: &mut usize,
) -> Result<(), VendorError> {
    let columns: Vec<&str> = answer
        .pointer("/properties/columns")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("the answer has no columns"))?
        .iter()
        .filter_map(|column| column.get("name").and_then(Value::as_str))
        .collect();
    let position = |name: &str| columns.iter().position(|column| *column == name);
    let cost_at = position("PreTaxCost")
        .or_else(|| position("Cost"))
        .ok_or_else(|| malformed("the answer has no cost column"))?;
    let currency_at =
        position("Currency").ok_or_else(|| malformed("the answer has no currency column"))?;
    let group_at = position("ResourceGroup");
    for row in answer
        .pointer("/properties/rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let cells = row
            .as_array()
            .ok_or_else(|| malformed("a row is not a list"))?;
        let cost = cells.get(cost_at).map(micro_of_value);
        let currency = cells
            .get(currency_at)
            .and_then(Value::as_str)
            .map(str::to_uppercase);
        match (cost, currency) {
            (Some(Ok(micro)), Some(currency)) if !currency.is_empty() => {
                let group = group_at
                    .and_then(|index| cells.get(index))
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty());
                let id = group.unwrap_or("unattributed");
                lines.push(CostLine {
                    period: period.to_owned(),
                    currency,
                    kind: "resource-group",
                    id: id.to_owned(),
                    name: id.to_owned(),
                    micro,
                });
            }
            _ => *skipped += 1,
        }
    }
    Ok(())
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: MANAGEMENT_HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
