//! The Vercel connector (issue #142, cloud providers).
//!
//! Cost: the FOCUS billing charges, `GET https://api.vercel.com/v1/billing/charges`
//! with `from` (inclusive) and `to` (exclusive) as ISO 8601 UTC, optionally
//! `teamId`. The answer is newline-delimited JSON, one FOCUS 1.3 charge per
//! line. Source: <https://vercel.com/docs/rest-api/reference/endpoints/billing/list-focus-billing-charges>
//!
//! Each charge has `BilledCost` (the invoiced amount), `BillingCurrency`,
//! `ChargePeriodStart`, `ChargeCategory`, `ServiceName` and `Tags`. Projects
//! are attributed by `Tags.ProjectId` (and `Tags.ProjectName`); a charge with
//! no project tag books as `unattributed`.
//!
//! What is booked: `Usage`, `Tax`, `Adjustment` and `Credit` charges. A
//! `Purchase` is a prepaid credit bought, which is cash, not an expense, so it
//! is counted in a note and not booked.
//!
//! Auth: a bearer access token. The endpoint is open to team members with the
//! Billing or Enterprise Viewer role; no narrower token scope is documented, so
//! the app help text asks for a token of a user with the Billing role.
//!
//! Invoices and a separate usage endpoint are not in the REST API, so Vercel
//! invoices are not imported; the charges are the source of truth.

use crate::connector::{Connector, CredentialField, SyncContext, next_day};
use crate::cost::{CostLine, micro_of, month_of, output};
use crate::http::VendorError;
use crate::model::SyncOutput;
use async_trait::async_trait;
use serde_json::Value;

const HOST: &str = "api.vercel.com";
const HOSTS: &[&str] = &[HOST];

/// The Vercel connector.
pub struct Vercel;

#[async_trait]
impl Connector for Vercel {
    fn id(&self) -> &'static str {
        "vercel"
    }

    fn display_name(&self) -> &'static str {
        "Vercel"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "Access token (team Billing role)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        match config.get("team_id") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(team)) => {
                let valid = !team.is_empty()
                    && team.len() <= 64
                    && team.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
                if valid {
                    Ok(())
                } else {
                    Err("team_id is the Vercel team id, such as team_abc123".to_owned())
                }
            }
            Some(_) => Err("team_id is the Vercel team id, such as team_abc123".to_owned()),
        }
    }

    fn completed_periods_only(&self) -> bool {
        true
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let end = next_day(ctx.until).ok_or_else(|| malformed("until is not a date"))?;
        let mut url = format!(
            "https://{HOST}/v1/billing/charges?from={}T00:00:00Z&to={end}T00:00:00Z",
            ctx.since
        );
        if let Some(team) = ctx.config.get("team_id").and_then(Value::as_str) {
            url.push_str("&teamId=");
            url.push_str(team);
        }
        let headers = vec![("Authorization", format!("Bearer {}", ctx.credential))];
        let rows = ctx.http.get_lines(HOSTS, &url, &headers).await?;
        let mut lines = Vec::new();
        let mut purchases = 0_usize;
        let mut skipped = 0_usize;
        for row in rows.as_array().into_iter().flatten() {
            match cost_line(row) {
                Ok(Some(line)) => lines.push(line),
                Ok(None) => purchases += 1,
                Err(()) => skipped += 1,
            }
        }
        let today = ctx.now.get(0..10).unwrap_or_default();
        let mut notes = Vec::new();
        let mut out = output("vercel", "Vercel", &lines, today, &mut notes);
        if purchases > 0 {
            notes.push(format!(
                "{purchases} prepaid credit purchase(s) are not booked: they are cash, not an expense"
            ));
        }
        if skipped > 0 {
            notes.push(format!(
                "{skipped} charge(s) had no readable amount or period and were skipped"
            ));
        }
        notes.push(
            "Vercel invoices are not in the REST API; the charges are the source of truth"
                .to_owned(),
        );
        out.notes = notes;
        Ok(out)
    }
}

/// One FOCUS charge as a cost line. `Ok(None)` for a purchase, which is not
/// booked. `Err` for a charge without a readable amount or period.
fn cost_line(row: &Value) -> Result<Option<CostLine>, ()> {
    let category = row
        .get("ChargeCategory")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if category == "Purchase" {
        return Ok(None);
    }
    let period = row
        .get("ChargePeriodStart")
        .and_then(Value::as_str)
        .and_then(month_of)
        .ok_or(())?;
    let currency = row
        .get("BillingCurrency")
        .and_then(Value::as_str)
        .ok_or(())?
        .to_uppercase();
    let text = match row.get("BilledCost").ok_or(())? {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return Err(()),
    };
    let micro = micro_of(&text).map_err(|_| ())?;
    let project = row
        .pointer("/Tags/ProjectId")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty());
    let name = row
        .pointer("/Tags/ProjectName")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .or(project)
        .unwrap_or("unattributed");
    Ok(Some(CostLine {
        period,
        currency,
        kind: "project",
        id: project.unwrap_or("unattributed").to_owned(),
        name: name.to_owned(),
        micro,
    }))
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
