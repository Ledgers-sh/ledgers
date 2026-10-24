//! The Spaceship connector (domains, renewals; no costs booked).
//!
//! Spaceship (spaceship.com) is the registrar for the domains. Its public API
//! (`https://spaceship.dev/api/v1`, reference at <https://docs.spaceship.dev>)
//! has **no billing, invoice, order or transaction endpoint** for the account.
//! The domain list is the only money-adjacent read, and it carries no charge:
//! `GET /v1/domains` gives `name`, `expirationDate`, `autoRenew` and the
//! lifecycle status, but no price. Renewal prices appear only on the
//! availability endpoints, for a name that is not yet registered.
//!
//! So this connector **books nothing**. A sync reads the domain list (paged by
//! `take` and `skip`, `total` in the answer) and returns it as metadata in
//! [`SyncOutput::domains`]. The renewals view reads the same list and shows the
//! domains that expire within 60 days, with their dates and auto-renew status.
//! Actual charges come from the bank feed or the invoice inbox, never from here.
//!
//! Auth: two headers, `X-Api-Key` and `X-Api-Secret`. The docs list the scopes
//! `domains:read` (for the list) and no others; a key with only that scope is
//! all Ledgers needs.

use crate::connector::{Connector, CredentialField, SyncContext, credential_text, is_date};
use crate::http::{VendorError, sanitize_vendor_text};
use crate::model::{DomainRecord, SyncOutput};
use async_trait::async_trait;
use serde_json::Value;

const HOST: &str = "spaceship.dev";
const HOSTS: &[&str] = &[HOST];
/// Domains per page (the API's maximum).
const TAKE: usize = 100;
/// Pages read per sync: 5,000 domains. A larger account is noted, not read.
const MAX_PAGES: usize = 50;

/// The days ahead the renewals view lists.
pub const RENEWAL_WINDOW_DAYS: i64 = 60;

/// The Spaceship connector.
pub struct Spaceship;

#[async_trait]
impl Connector for Spaceship {
    fn id(&self) -> &'static str {
        "spaceship"
    }

    fn display_name(&self) -> &'static str {
        "Spaceship"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "api_key",
                label: "API key",
            },
            CredentialField {
                name: "api_secret",
                label: "API secret",
            },
        ]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, _config: &Value) -> Result<(), String> {
        Ok(())
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let key =
            credential_text(ctx.credential, "api_key").map_err(|detail| malformed(&detail))?;
        let secret =
            credential_text(ctx.credential, "api_secret").map_err(|detail| malformed(&detail))?;
        let headers = vec![("X-Api-Key", key), ("X-Api-Secret", secret)];

        let mut out = SyncOutput::default();
        let mut total: Option<usize> = None;
        let mut skipped = 0_usize;
        let mut skip = 0_usize;
        let mut pages = 0_usize;
        loop {
            let url = format!("https://{HOST}/api/v1/domains?take={TAKE}&skip={skip}&orderBy=name");
            let page = ctx.http.get_json(HOSTS, &url, &headers).await?;
            let items = page
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| malformed("the domain list has no items"))?;
            total = page
                .get("total")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .or(total);
            for item in items {
                match domain_of(item) {
                    Some(record) => out.domains.push(record),
                    None => skipped += 1,
                }
            }
            pages += 1;
            skip += items.len();
            let done = items.is_empty() || total.is_some_and(|t| skip >= t);
            if done {
                break;
            }
            if pages >= MAX_PAGES {
                out.notes.push(format!(
                    "the account has more than {} domains; the rest were not read",
                    MAX_PAGES * TAKE
                ));
                break;
            }
        }
        out.notes.push(format!(
            "{} domain(s) read from Spaceship as metadata",
            out.domains.len()
        ));
        if skipped > 0 {
            out.notes.push(format!(
                "{skipped} domain(s) had no name or expiry date and were skipped"
            ));
        }
        out.notes.push(
            "Spaceship's API has no billing or invoice endpoint, and the domain list has no price: nothing is booked. Renewal charges come from the bank feed or the invoice inbox"
                .to_owned(),
        );
        Ok(out)
    }
}

/// One domain of the list as metadata. `None` when it has no name or no readable
/// expiry date; such an entry is counted, never guessed.
fn domain_of(item: &Value) -> Option<DomainRecord> {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(sanitize_vendor_text)
        .filter(|name| !name.is_empty())?;
    let expires_on = item
        .get("expirationDate")
        .and_then(Value::as_str)
        .and_then(|text| text.get(..10))
        .filter(|date| is_date(date))?
        .to_owned();
    let auto_renew = item
        .get("autoRenew")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let status = item
        .get("lifecycleStatus")
        .and_then(Value::as_str)
        .map(sanitize_vendor_text)
        .filter(|status| !status.is_empty());
    Some(DomainRecord {
        name,
        expires_on,
        auto_renew,
        status,
    })
}

/// The domains that expire within `days` of `today` (both `YYYY-MM-DD`), soonest
/// first. A domain already past its expiry is included, so it is never hidden.
#[must_use]
pub fn renewals_within(domains: &[DomainRecord], today: &str, days: i64) -> Vec<DomainRecord> {
    let Some(horizon) = add_days(today, days) else {
        return Vec::new();
    };
    let mut due: Vec<DomainRecord> = domains
        .iter()
        .filter(|domain| domain.expires_on.as_str() <= horizon.as_str())
        .cloned()
        .collect();
    due.sort_by(|a, b| a.expires_on.cmp(&b.expires_on).then(a.name.cmp(&b.name)));
    due
}

/// `date` (`YYYY-MM-DD`) moved by `days`, or `None` for a bad date.
fn add_days(date: &str, days: i64) -> Option<String> {
    let mut current = date.to_owned();
    let step = if days >= 0 {
        crate::connector::next_day
    } else {
        crate::connector::previous_day
    };
    for _ in 0..days.unsigned_abs() {
        current = step(&current)?;
    }
    Some(current)
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOST.to_owned(),
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(name: &str, expires_on: &str) -> DomainRecord {
        DomainRecord {
            name: name.to_owned(),
            expires_on: expires_on.to_owned(),
            auto_renew: false,
            status: None,
        }
    }

    #[test]
    fn renewals_are_inside_the_window_soonest_first_and_keep_past_expiry() {
        let domains = vec![
            domain("later.com", "2027-06-01"),
            domain("soon.in", "2026-11-20"),
            domain("lapsed.co", "2026-10-01"),
            domain("edge.app", "2026-12-24"),
        ];
        let due = renewals_within(&domains, "2026-10-25", RENEWAL_WINDOW_DAYS);
        let names: Vec<&str> = due.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["lapsed.co", "soon.in", "edge.app"]);
    }

    #[test]
    fn a_bad_today_lists_nothing() {
        assert!(renewals_within(&[domain("a.in", "2026-11-01")], "nope", 60).is_empty());
    }

    #[test]
    fn domain_rows_without_a_name_or_expiry_are_refused_not_guessed() {
        let ok = serde_json::json!({"name": "squeez.in", "expirationDate": "2027-03-04T00:00:00Z", "autoRenew": true, "lifecycleStatus": "active"});
        let record = domain_of(&ok).expect("a row with a name and expiry");
        assert_eq!(record.expires_on, "2027-03-04");
        assert!(record.auto_renew);
        assert_eq!(record.status.as_deref(), Some("active"));
        assert!(domain_of(&serde_json::json!({"expirationDate": "2027-03-04"})).is_none());
        assert!(domain_of(&serde_json::json!({"name": "a.in"})).is_none());
        assert!(
            domain_of(&serde_json::json!({"name": "a.in", "expirationDate": "soon"})).is_none()
        );
    }
}
