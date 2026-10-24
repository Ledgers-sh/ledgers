//! The Jira Cloud connector: seats in use, products, projects and a PLANNED seat cost.
//!
//! Research (October 2026). Atlassian's Commerce REST API has invoice endpoints
//! (`/v2/invoices`, `/v1/invoices/{id}/download`), but they are addressed by a
//! transaction account (`X-transaction-account`), which is the Marketplace vendor's
//! commerce account, and the documentation does not describe them as returning a
//! customer's own Jira Cloud subscription invoices. The Organization (admin) REST
//! API lists users and roles and has no price or invoice data. So no Atlassian API
//! is used here to book a cost. This connector therefore **books nothing**.
//!
//! What it reads, per site (`{site}.atlassian.net`):
//! - `GET /rest/api/3/users/search` (paged by `startAt` and `maxResults`): the
//!   active accounts of type `atlassian`, which are the seats in use.
//! - `GET /rest/api/3/applicationrole`: each product (Jira Software, Service
//!   Management, Work Management) with its licensed seats and its user count. It
//!   needs Jira administrator rights; without them the product breakdown is a note.
//! - `GET /rest/api/3/project/search` (paged by `startAt`): the projects and their
//!   keys. A project key is the allocation key: a rule maps the key to a unit.
//!
//! The planned monthly cost is `seats in use x price_per_seat`, where the price is
//! typed into the connection (`price_per_seat`, `price_currency`). It is a PLAN. It is
//! shown as a note and never booked as an expense. Actual charges come from the
//! bank feed or the invoice inbox.
//!
//! Auth: an API token, sent as HTTP Basic with the account email as the user name.
//! Least privilege: an account that can browse users and projects. OAuth 2.0 (3LO)
//! is not implemented.

use crate::connector::{Connector, CredentialField, SyncContext, config_text, credential_text};
use crate::cost::{decimal_of_minor, exponent_of, micro_of, to_minor};
use crate::http::{VendorError, sanitize_vendor_text};
use crate::model::{SyncOutput, UsageRecord};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ledgers_money::Currency;
use serde_json::Value;

/// Users per page (the API's maximum for the user search).
const USER_PAGE: usize = 1000;
/// Projects per page.
const PROJECT_PAGE: usize = 50;
/// Pages read per list; a larger site is noted, not read.
const MAX_PAGES: usize = 20;

/// The Jira connector.
pub struct Jira;

/// The site host (`example.atlassian.net`) from the `site` config key.
///
/// # Errors
/// A sentence when the site is not a `*.atlassian.net` host name.
pub fn site_host(text: &str) -> Result<String, String> {
    let host = text.trim().to_lowercase();
    let label = host.strip_suffix(".atlassian.net").unwrap_or_default();
    let valid = !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if valid {
        Ok(host)
    } else {
        Err("site is the Jira Cloud host, such as example.atlassian.net".to_owned())
    }
}

#[async_trait]
impl Connector for Jira {
    fn id(&self) -> &'static str {
        "jira"
    }

    fn display_name(&self) -> &'static str {
        "Jira (Atlassian)"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "email",
                label: "Account email",
            },
            CredentialField {
                name: "api_token",
                label: "API token",
            },
        ]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        // The site host is checked per connection (see `site_host`); no fixed host.
        &[]
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        site_host(config_text(config, "site")?)?;
        match config.get("price_per_seat") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(price)) if price.trim().is_empty() => Ok(()),
            Some(Value::String(price)) => {
                micro_of(price.trim())
                    .map_err(|_| "price_per_seat is a decimal amount, such as 8.60".to_owned())?;
                let currency = config_text(config, "price_currency")
                    .map_err(|_| "price_currency is required with price_per_seat".to_owned())?;
                Currency::from_code(&currency.to_uppercase())
                    .map(|_| ())
                    .map_err(|_| "price_currency is an ISO 4217 code, such as USD".to_owned())
            }
            Some(_) => Err("price_per_seat is text, a decimal amount".to_owned()),
        }
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let email = credential_text(ctx.credential, "email").map_err(|d| malformed(&d))?;
        let token = credential_text(ctx.credential, "api_token").map_err(|d| malformed(&d))?;
        let site = site_host(config_text(ctx.config, "site").map_err(|d| malformed(&d))?)
            .map_err(|d| malformed(&d))?;
        let hosts = [site.as_str()];
        let headers = vec![(
            "Authorization",
            format!("Basic {}", STANDARD.encode(format!("{email}:{token}"))),
        )];
        let mut notes = Vec::new();

        // Seats in use: active Atlassian accounts. A refused token fails the sync here.
        let seats = count_seats(ctx, &hosts, &headers, &site).await?;
        notes.push(format!(
            "Seats in use on {site}: {seats} active Atlassian account(s)"
        ));

        // Products: a token without Jira administrator rights gets a note instead.
        let roles_url = format!("https://{site}/rest/api/3/applicationrole");
        match ctx.http.get_json(&hosts, &roles_url, &headers).await {
            Ok(answer) => {
                for role in answer.as_array().into_iter().flatten() {
                    notes.push(product_note(role));
                }
            }
            Err(VendorError::Refused { status, .. }) => notes.push(format!(
                "Product seat counts not read: the token lacks Jira administrator rights ({status})"
            )),
            Err(other) => return Err(other),
        }

        // Projects: the allocation keys. Usage is a snapshot of the current month.
        let projects = list_projects(ctx, &hosts, &headers, &site).await?;
        let today = ctx.now.get(0..10).unwrap_or_default();
        let period = crate::cost::month_of(today).unwrap_or_default();
        let usage: Vec<UsageRecord> = projects
            .iter()
            .map(|(key, name)| UsageRecord {
                period: period.clone(),
                resource_kind: "jira-project".to_owned(),
                resource_id: key.clone(),
                resource_name: sanitize_vendor_text(name),
                quantity: 1,
            })
            .collect();
        notes.push(format!(
            "{} project(s) read: their keys are the allocation keys",
            projects.len()
        ));

        // The planned cost: a plan from the typed price, never a booked charge.
        if let Some(price) = config_price(ctx.config)? {
            let (micro, currency) = price;
            let exponent = exponent_of(&currency);
            let planned = to_minor(micro * i128::try_from(seats).unwrap_or(i128::MAX), exponent);
            // Built from our own numbers and text only, so it is not truncated as vendor text.
            notes.push(format!(
                "Planned monthly cost {} {currency}: {seats} seat(s) x {} {currency} per seat. This is a plan from the price you entered, not a charge, and it is not booked. Actual charges come from the bank feed or the invoice inbox.",
                decimal_of_minor(planned, exponent),
                decimal_of_minor(micro, 6)
            ));
        } else {
            notes.push("No price per seat is set, so no planned monthly cost is shown.".to_owned());
        }

        Ok(SyncOutput {
            invoices: Vec::new(),
            usage,
            notes,
            entries: Vec::new(),
            domains: Vec::new(),
        })
    }
}

/// The active Atlassian accounts on the site, paged. A page that is full may have
/// more; the pages read are capped and noted when the cap is reached.
async fn count_seats(
    ctx: &SyncContext<'_>,
    hosts: &[&str],
    headers: &[(&str, String)],
    site: &str,
) -> Result<usize, VendorError> {
    let mut seats = 0_usize;
    let mut start = 0_usize;
    for _ in 0..MAX_PAGES {
        let url = format!(
            "https://{site}/rest/api/3/users/search?startAt={start}&maxResults={USER_PAGE}"
        );
        let page = ctx.http.get_json(hosts, &url, headers).await?;
        let users = page
            .as_array()
            .ok_or_else(|| malformed("the user search answer is not a list"))?;
        seats += users
            .iter()
            .filter(|user| {
                user.get("accountType").and_then(Value::as_str) == Some("atlassian")
                    && user.get("active").and_then(Value::as_bool) == Some(true)
            })
            .count();
        if users.len() < USER_PAGE {
            break;
        }
        start += users.len();
    }
    Ok(seats)
}

/// The projects on the site as `(key, name)`, paged by `startAt` and `isLast`.
async fn list_projects(
    ctx: &SyncContext<'_>,
    hosts: &[&str],
    headers: &[(&str, String)],
    site: &str,
) -> Result<Vec<(String, String)>, VendorError> {
    let mut out = Vec::new();
    let mut start = 0_usize;
    for _ in 0..MAX_PAGES {
        let url = format!(
            "https://{site}/rest/api/3/project/search?startAt={start}&maxResults={PROJECT_PAGE}&orderBy=key"
        );
        let page = ctx.http.get_json(hosts, &url, headers).await?;
        let values = page
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("the project search answer has no values"))?;
        for project in values {
            if let (Some(key), Some(name)) = (
                project.get("key").and_then(Value::as_str),
                project.get("name").and_then(Value::as_str),
            ) {
                out.push((key.to_owned(), name.to_owned()));
            }
        }
        if page.get("isLast").and_then(Value::as_bool) != Some(false) || values.is_empty() {
            break;
        }
        start += values.len();
    }
    Ok(out)
}

/// One product's seats as a note: `Jira Software: 12 of 50 seats in use`.
fn product_note(role: &Value) -> String {
    let name = role
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("product");
    let used = role.get("userCount").and_then(Value::as_u64);
    let licensed = role.get("numberOfSeats").and_then(Value::as_u64);
    let unlimited = role.get("hasUnlimitedSeats").and_then(Value::as_bool) == Some(true);
    let line = match (used, licensed, unlimited) {
        (Some(used), _, true) => format!("{name}: {used} in use, unlimited seats"),
        (Some(used), Some(licensed), false) => format!("{name}: {used} of {licensed} seats in use"),
        (Some(used), None, false) => format!("{name}: {used} seats in use"),
        _ => format!("{name}: seat counts not given"),
    };
    sanitize_vendor_text(&line)
}

/// The price per seat as `(micro, currency)`, when the connection sets one.
fn config_price(config: &Value) -> Result<Option<(i128, String)>, VendorError> {
    let price = match config.get("price_per_seat").and_then(Value::as_str) {
        Some(text) if !text.trim().is_empty() => text.trim(),
        _ => return Ok(None),
    };
    let micro = micro_of(price).map_err(|err| malformed(&err.to_string()))?;
    let currency = config_text(config, "price_currency")
        .map_err(|d| malformed(&d))?
        .to_uppercase();
    Ok(Some((micro, currency)))
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: "atlassian".to_owned(),
        detail: detail.to_owned(),
    }
}
