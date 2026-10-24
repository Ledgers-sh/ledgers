//! The Supabase connector (issue #142, cloud providers): non-currency metrics.
//!
//! What the Management API gives for an organization, and what this connector
//! does with it:
//!
//! - `GET /v1/organizations/{slug}/projects`: the projects (`ref`, `name`).
//! - `GET /v1/projects/{ref}/billing/addons`: the add-ons a project pays for
//!   (`selected_addons[].variant.price` with `amount` and `interval`). The price
//!   has **no currency field**, so an add-on charge cannot be booked in a
//!   currency without guessing. It is reported in the run's notes, not booked.
//! - `GET /v1/projects/{ref}/analytics/endpoints/usage.api-counts?interval=1day`:
//!   request counts for the last day (REST, auth, realtime, storage). These are
//!   counts, not money.
//!
//! So a Supabase connection books **no bills and no usage** against the business
//! units. Its metrics appear in the run history of the Connected sources screen,
//! and no allocation rule applies because nothing is split. Source:
//! the `OpenAPI` document of the Management API, at `api.supabase.com/api/v1-json`.
//!
//! Auth: a personal access token. Supabase documents scoped personal access
//! tokens (limited to chosen organizations and permissions); use one limited to
//! the organization with read permissions. The exact permission names are not
//! listed in the API reference, so the app help text does not name them.

use crate::connector::{Connector, CredentialField, SyncContext, config_text};
use crate::http::{VendorError, percent_encode, sanitize_vendor_text};
use crate::model::SyncOutput;
use async_trait::async_trait;
use serde_json::Value;

const HOST: &str = "api.supabase.com";
const HOSTS: &[&str] = &[HOST];
/// Projects read per sync (two requests each); the rest are counted in a note.
const MAX_PROJECTS: usize = 25;

/// The Supabase connector.
pub struct Supabase;

#[async_trait]
impl Connector for Supabase {
    fn id(&self) -> &'static str {
        "supabase"
    }

    fn display_name(&self) -> &'static str {
        "Supabase"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "Personal access token (read access to the organization)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        let slug = config_text(config, "organization")?;
        if slug.len() > 100 || !slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("organization is the Supabase organization slug".to_owned());
        }
        Ok(())
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let slug = config_text(ctx.config, "organization").map_err(|detail| malformed(&detail))?;
        let headers = vec![("Authorization", format!("Bearer {}", ctx.credential))];
        let list = ctx
            .http
            .get_json(
                HOSTS,
                &format!(
                    "https://{HOST}/v1/organizations/{}/projects",
                    percent_encode(slug)
                ),
                &headers,
            )
            .await?;
        let projects = list
            .get("projects")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("the project list has no projects"))?;

        let mut out = SyncOutput::default();
        for project in projects.iter().take(MAX_PROJECTS) {
            let reference = project
                .get("ref")
                .and_then(Value::as_str)
                .filter(|r| !r.is_empty())
                .ok_or_else(|| malformed("a project has no ref"))?;
            let name = project
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(reference);
            let name = sanitize_vendor_text(name);
            let encoded = percent_encode(reference);

            let addons = ctx
                .http
                .get_json(
                    HOSTS,
                    &format!("https://{HOST}/v1/projects/{encoded}/billing/addons"),
                    &headers,
                )
                .await?;
            for addon in addons
                .get("selected_addons")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                out.notes.push(addon_note(&name, addon));
            }

            let usage = ctx
                .http
                .get_json(
                    HOSTS,
                    &format!("https://{HOST}/v1/projects/{encoded}/analytics/endpoints/usage.api-counts?interval=1day"),
                    &headers,
                )
                .await?;
            out.notes.push(usage_note(&name, &usage));
        }
        if projects.len() > MAX_PROJECTS {
            out.notes.push(format!(
                "{} more project(s) in the organization were not read in this sync",
                projects.len() - MAX_PROJECTS
            ));
        }
        out.notes.push(
            "Supabase metrics are not booked: the add-on prices carry no currency, and request counts are not money"
                .to_owned(),
        );
        Ok(out)
    }
}

/// One add-on as a run note. The price is shown as the API gives it, with its
/// interval, and labelled as not booked.
fn addon_note(project: &str, addon: &Value) -> String {
    let kind = addon
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("add-on");
    let variant = addon
        .pointer("/variant/id")
        .and_then(Value::as_str)
        .or_else(|| addon.pointer("/variant/name").and_then(Value::as_str))
        .unwrap_or("selected");
    let price = addon.pointer("/variant/price");
    let amount = price
        .and_then(|p| p.get("amount"))
        .map_or_else(|| "no price".to_owned(), Value::to_string);
    let interval = price
        .and_then(|p| p.get("interval"))
        .and_then(Value::as_str)
        .unwrap_or("unknown interval");
    sanitize_vendor_text(&format!(
        "{project}: {kind} {variant} at {amount} per {interval} (amount without currency, not booked)"
    ))
}

/// The last day's request counts as a run note. The counts are summed across
/// the returned points.
fn usage_note(project: &str, usage: &Value) -> String {
    let total = |field: &str| -> String {
        let sum: f64 = usage
            .get("result")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|point| point.get(field).and_then(Value::as_f64))
            .sum();
        format!("{sum:.0}")
    };
    sanitize_vendor_text(&format!(
        "{project}: last day REST {} requests, auth {}, realtime {}, storage {} (counts, not money)",
        total("total_rest_requests"),
        total("total_auth_requests"),
        total("total_realtime_requests"),
        total("total_storage_requests"),
    ))
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
