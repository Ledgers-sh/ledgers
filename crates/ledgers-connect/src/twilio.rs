//! The Twilio connector (usage costs, one invoice per month and currency).
//!
//! Cost: the Usage Records API, `GET /2010-04-01/Accounts/{AccountSid}/Usage/Records/Monthly.json`
//! with `StartDate` and `EndDate` (`YYYY-MM-DD`), `PageSize=1000` and
//! `IncludeSubaccounts=false`. Each record is one usage category in one month, with
//! `price` (a decimal string), `price_unit` (the currency Twilio bills in, usually
//! `USD`), `start_date` and `category` (`sms-outbound`, `calls`, `phonenumbers` and so
//! on). The aggregate `totalprice` category is skipped, because the categories already
//! add up to it. Paging follows `next_page_uri`, which must stay on `api.twilio.com`.
//!
//! Sources: <https://www.twilio.com/docs/usage/api/usage-record> (Usage Records, the
//! query parameters, the record fields and `next_page_uri`) and
//! <https://www.twilio.com/docs/iam/api/account> (the subaccount list and API key
//! authentication).
//!
//! Auth: HTTP Basic, the API key SID as the user name and the API key secret as the
//! password. Use a standard or restricted API key, never the account's auth token.
//!
//! Scope: one connection per account. A subaccount needs its own connection with
//! `subaccount_sid`, so that a subaccount's cost is never counted twice. Usage is
//! read with `IncludeSubaccounts=false`, so a parent connection holds only its own
//! charges.
//!
//! Balance: `GET /2010-04-01/Accounts/{AccountSid}/Balance.json` is read on each sync
//! for information only. It is a run note, never a bill. A child account has no
//! balance, so the parent's `account_sid` is always the one read. Whether the API key
//! may read it is not documented, so a refusal is a note, not a failed sync.
//!
//! Allocation: a usage row's resource is the account (or subaccount) SID. An allocation
//! rule keyed by that SID maps the connection's costs to a business unit; without one,
//! costs book to the connection's default unit and are listed in the mapping queue.

use crate::connector::{Connector, CredentialField, SyncContext, config_text, credential_text};
use crate::cost::{CostLine, exponent_of, micro_of_value, month_of, output, to_minor};
use crate::http::{VendorError, sanitize_vendor_text};
use crate::model::SyncOutput;
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use std::collections::BTreeMap;

const HOST: &str = "api.twilio.com";
const HOSTS: &[&str] = &[HOST];
/// Records per page (the API's maximum).
const PAGE_SIZE: usize = 1000;
/// Pages read per sync; a larger answer is noted, not read.
const MAX_PAGES: usize = 20;
/// The aggregate category, skipped because the categories sum to it.
const TOTAL_CATEGORY: &str = "totalprice";

/// Amounts in millionths by (month, currency), then by category.
type Categories = BTreeMap<(String, String), BTreeMap<String, i128>>;

/// The usage read for one scope: its cost lines and the amounts by category.
#[derive(Default)]
struct Usage {
    lines: Vec<CostLine>,
    categories: Categories,
    skipped: usize,
    truncated: bool,
}

/// The Twilio connector.
pub struct Twilio;

#[async_trait]
impl Connector for Twilio {
    fn id(&self) -> &'static str {
        "twilio"
    }

    fn display_name(&self) -> &'static str {
        "Twilio"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[
            CredentialField {
                name: "api_key_sid",
                label: "API key SID (starts with SK)",
            },
            CredentialField {
                name: "api_key_secret",
                label: "API key secret",
            },
        ]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        let account = config_text(config, "account_sid")?;
        if !is_account_sid(account) {
            return Err(
                "account_sid is the Twilio account SID, starting with AC and 32 hex characters"
                    .to_owned(),
            );
        }
        match config.get("subaccount_sid") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(sub)) if sub.trim().is_empty() => Ok(()),
            Some(Value::String(sub)) if is_account_sid(sub.trim()) => Ok(()),
            Some(_) => Err(
                "subaccount_sid is a subaccount SID, starting with AC and 32 hex characters"
                    .to_owned(),
            ),
        }
    }

    fn completed_periods_only(&self) -> bool {
        true
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let key_sid = credential_text(ctx.credential, "api_key_sid").map_err(|d| malformed(&d))?;
        let secret =
            credential_text(ctx.credential, "api_key_secret").map_err(|d| malformed(&d))?;
        let account = config_text(ctx.config, "account_sid")
            .map_err(|d| malformed(&d))?
            .to_owned();
        let scope = match ctx
            .config
            .get("subaccount_sid")
            .and_then(Value::as_str)
            .map(str::trim)
        {
            Some(sub) if !sub.is_empty() => sub.to_owned(),
            _ => account.clone(),
        };
        let headers = vec![(
            "Authorization",
            format!("Basic {}", STANDARD.encode(format!("{key_sid}:{secret}"))),
        )];

        let usage = read_usage(ctx, &scope, &headers).await?;
        let today = ctx.now.get(0..10).unwrap_or_default();
        let mut notes = Vec::new();
        let mut out = output("twilio", "Twilio", &usage.lines, today, &mut notes);
        describe_invoices(&mut out, &usage.categories);
        if usage.truncated {
            notes.push(format!(
                "more than {MAX_PAGES} usage pages; the rest were not read"
            ));
        }
        if usage.skipped > 0 {
            notes.push(format!(
                "{} usage record(s) had no readable price, currency or month and were skipped",
                usage.skipped
            ));
        }
        notes.push(balance_note(ctx, &account, &headers).await);
        out.notes = notes;
        Ok(out)
    }
}

/// Reads every monthly usage page for `scope`, following `next_page_uri`.
async fn read_usage(
    ctx: &SyncContext<'_>,
    scope: &str,
    headers: &[(&str, String)],
) -> Result<Usage, VendorError> {
    let mut usage = Usage::default();
    let mut url = format!(
        "https://{HOST}/2010-04-01/Accounts/{scope}/Usage/Records/Monthly.json?StartDate={}&EndDate={}&PageSize={PAGE_SIZE}&IncludeSubaccounts=false",
        ctx.since, ctx.until
    );
    for page_number in 1..=MAX_PAGES {
        let page = ctx.http.get_json(HOSTS, &url, headers).await?;
        let records = page
            .get("usage_records")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("the usage answer has no usage_records"))?;
        for record in records {
            add_record(&mut usage, record, scope);
        }
        let Some(next) = page.get("next_page_uri").and_then(Value::as_str) else {
            break;
        };
        if !next.starts_with("/2010-04-01/") || next.starts_with("//") {
            return Err(malformed("next_page_uri leaves the Twilio API"));
        }
        if page_number == MAX_PAGES {
            usage.truncated = true;
            break;
        }
        url = format!("https://{HOST}{next}");
    }
    Ok(usage)
}

/// Adds one usage record. The category `totalprice` is skipped; a record with no
/// readable month, currency or price is counted, never guessed.
fn add_record(usage: &mut Usage, record: &Value, scope: &str) {
    let category = record
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if category == TOTAL_CATEGORY {
        return;
    }
    let period = record
        .get("start_date")
        .and_then(Value::as_str)
        .and_then(month_of);
    let currency = record
        .get("price_unit")
        .and_then(Value::as_str)
        .map(str::to_uppercase)
        .filter(|c| c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase()));
    let micro = record.get("price").and_then(|p| micro_of_value(p).ok());
    let (Some(period), Some(currency), Some(micro), false) =
        (period, currency, micro, category.is_empty())
    else {
        usage.skipped += 1;
        return;
    };
    // The sign is ignored: the books take the size of the charge (see the module docs).
    let amount = micro.abs();
    *usage
        .categories
        .entry((period.clone(), currency.clone()))
        .or_default()
        .entry(category.to_owned())
        .or_insert(0) += amount;
    usage.lines.push(CostLine {
        period,
        currency,
        kind: "account",
        id: scope.to_owned(),
        name: scope.to_owned(),
        micro: amount,
    });
}

/// Names each invoice's categories in its description, in minor units.
fn describe_invoices(out: &mut SyncOutput, categories: &Categories) {
    for invoice in &mut out.invoices {
        let exponent = exponent_of(&invoice.currency);
        let parts: Vec<String> = categories
            .get(&(invoice.period.clone(), invoice.currency.clone()))
            .map(|map| {
                map.iter()
                    .map(|(category, micro)| format!("{category} {}", to_minor(*micro, exponent)))
                    .collect()
            })
            .unwrap_or_default();
        invoice.description = sanitize_vendor_text(&format!(
            "Twilio usage {} in {} (minor units by category: {})",
            invoice.period,
            invoice.currency,
            parts.join(", ")
        ));
    }
}

/// The account balance as a run note. Read from the parent account, which holds it;
/// a refusal or failure is a note, so it never fails the sync.
async fn balance_note(ctx: &SyncContext<'_>, account: &str, headers: &[(&str, String)]) -> String {
    let url = format!("https://{HOST}/2010-04-01/Accounts/{account}/Balance.json");
    match ctx.http.get_json(HOSTS, &url, headers).await {
        Ok(answer) => {
            let amount = answer
                .get("balance")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let currency = answer
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            sanitize_vendor_text(&format!(
                "account balance {amount} {currency} (information only, not booked)"
            ))
        }
        Err(err) => sanitize_vendor_text(&format!("account balance not read: {err}")),
    }
}

/// Whether `text` is an account or API key style SID: `AC` and 32 hex characters.
fn is_account_sid(text: &str) -> bool {
    text.len() == 34 && text.starts_with("AC") && text[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
