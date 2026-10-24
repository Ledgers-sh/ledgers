//! The Stripe connector (issue #142, payments): balance transactions as journal
//! entries.
//!
//! Source of truth: the balance transactions, `GET https://api.stripe.com/v1/balance_transactions`,
//! paged with `starting_after` while `has_more` (`limit=100`), within `created[gte]`
//! and `created[lt]`, with `expand[]=data.source` so each transaction carries its charge
//! or refund (for the presentment currency and amount). A restricted key with read
//! access to balance transactions, charges, refunds, payouts, disputes and invoices is
//! the least privilege. Connect sub-accounts are read with the `Stripe-Account` header.
//! Source: <https://docs.stripe.com/api/balance_transactions/list>
//!
//! Each balance transaction with amount `A`, fee `F` and net `N = A - F` becomes one
//! balanced entry in the settlement currency:
//!
//! | reporting category | revenue | fee expense | clearing |
//! |---|---|---|---|
//! | `charge` | `A` credit | `F` debit | `N` debit |
//! | `refund`, `dispute` | `A` (negative) | `F` debit | `N` debit (negative) |
//! | `fee` | none | `-A` (the charge) | `A` credit |
//! | anything else | suspense `A` | `F` debit | `N` debit |
//!
//! `payout` is not posted here: the payout leaves the clearing account when its bank
//! line is matched, through the bank rule (`bank.transactions.post`), so the clearing
//! account reaches zero when the bank side is booked. A category this connector does
//! not know goes to the configured suspense account and is flagged in the run notes, never
//! dropped. The presentment currency and amount go in the description; no FX line is booked.

use crate::connector::{Connector, CredentialField, SyncContext, config_text, is_date, next_day};
use crate::cost::{date_of_epoch, epoch_seconds};
use crate::http::{VendorError, percent_encode, sanitize_vendor_text};
use crate::model::{PlannedEntry, PlannedLine, SyncOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::fmt::Write as _;

const HOST: &str = "api.stripe.com";
const HOSTS: &[&str] = &[HOST];
/// Transactions per page (the endpoint's maximum).
const LIMIT: u32 = 100;
/// Pages followed per sync: 20,000 transactions. A longer window is reported in a note.
const MAX_PAGES: u32 = 200;

/// The accounts a Stripe connection posts to, from its config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accounts {
    /// The Stripe balance clearing account.
    pub clearing: String,
    /// The revenue account (charges, and refunds and disputes as reversals).
    pub revenue: String,
    /// The fee expense account (the common `expense_account` key).
    pub expense: String,
    /// The suspense account for a category this connector does not know.
    pub suspense: String,
}

/// The Stripe connector.
pub struct Stripe;

#[async_trait]
impl Connector for Stripe {
    fn id(&self) -> &'static str {
        "stripe"
    }

    fn display_name(&self) -> &'static str {
        "Stripe"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "Restricted API key (read: balance, charges, refunds, payouts, disputes, invoices)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        for key in ["clearing_account", "revenue_account", "suspense_account"] {
            config_text(config, key)?;
        }
        if let Some(account) = config.get("stripe_account").and_then(Value::as_str)
            && (!account.starts_with("acct_")
                || account.len() > 64
                || !account
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            return Err("stripe_account is a Connect account id, such as acct_1Example".to_owned());
        }
        Ok(())
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let accounts = accounts_of(ctx.config).map_err(|detail| malformed(&detail))?;
        let since = epoch_seconds(&format!("{}T00:00:00Z", ctx.since))
            .ok_or_else(|| malformed("since is not a date"))?;
        if !is_date(ctx.until) {
            return Err(malformed("until is not a date"));
        }
        let end = next_day(ctx.until).ok_or_else(|| malformed("until is not a date"))?;
        let until = epoch_seconds(&format!("{end}T00:00:00Z"))
            .ok_or_else(|| malformed("until is not a date"))?;
        let mut headers = vec![("Authorization", format!("Bearer {}", ctx.credential))];
        if let Some(account) = ctx.config.get("stripe_account").and_then(Value::as_str) {
            headers.push(("Stripe-Account", account.to_owned()));
        }

        let mut out = SyncOutput::default();
        let mut cursor: Option<String> = None;
        let mut truncated = false;
        for page in 0..MAX_PAGES {
            let mut url = format!(
                "https://{HOST}/v1/balance_transactions?limit={LIMIT}&created[gte]={since}&created[lt]={until}&expand[]=data.source"
            );
            if let Some(id) = &cursor {
                url.push_str("&starting_after=");
                url.push_str(&percent_encode(id));
            }
            let answer = ctx.http.get_json(HOSTS, &url, &headers).await?;
            let rows = answer
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| malformed("the answer has no data list"))?;
            for row in rows {
                match plan_transaction(row, &accounts) {
                    Ok(Some(entry)) => out.entries.push(entry),
                    Ok(None) => {}
                    Err(detail) => out.notes.push(sanitize_vendor_text(&detail)),
                }
            }
            if answer.get("has_more").and_then(Value::as_bool) != Some(true) {
                break;
            }
            cursor = rows
                .last()
                .and_then(|row| row.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                return Err(malformed("a page has more rows but no last id"));
            }
            if page + 1 == MAX_PAGES {
                truncated = true;
            }
        }
        if truncated {
            out.notes.push(format!(
                "the window holds more than {} transactions; the rest are read by a later sync",
                MAX_PAGES * LIMIT
            ));
        }
        let posted = out.entries.len();
        out.notes.push(format!(
            "{posted} balance transaction(s) planned as entries; payouts are left to the bank rule"
        ));
        out.notes.push("Stripe charges and refunds book in the settlement currency; the presentment amount is in each description".to_owned());
        Ok(out)
    }
}

fn accounts_of(config: &Value) -> Result<Accounts, String> {
    Ok(Accounts {
        clearing: config_text(config, "clearing_account")?.to_owned(),
        revenue: config_text(config, "revenue_account")?.to_owned(),
        expense: config_text(config, "expense_account")?.to_owned(),
        suspense: config_text(config, "suspense_account")?.to_owned(),
    })
}

/// One balance transaction as a balanced entry, or `None` when it posts nothing
/// (a payout, or a transaction with no amount).
///
/// # Errors
/// A sentence naming the transaction when its amount, fee or net does not add up,
/// or a field is missing. The transaction is then not posted, and the run notes it.
pub fn plan_transaction(bt: &Value, accounts: &Accounts) -> Result<Option<PlannedEntry>, String> {
    let id = bt
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "a balance transaction has no id and was skipped".to_owned())?;
    let amount = bt
        .get("amount")
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("balance transaction {id} has no amount"))?;
    let fee = bt.get("fee").and_then(Value::as_i64).unwrap_or(0);
    let net = bt
        .get("net")
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("balance transaction {id} has no net"))?;
    if fee < 0 || amount.checked_sub(fee) != Some(net) {
        return Err(format!(
            "balance transaction {id} does not add up: amount minus fee is not net"
        ));
    }
    let currency = bt
        .get("currency")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("balance transaction {id} has no currency"))?
        .to_uppercase();
    let category = bt
        .get("reporting_category")
        .and_then(Value::as_str)
        .unwrap_or("");
    if category == "payout" {
        return Ok(None);
    }
    let created = bt
        .get("created")
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("balance transaction {id} has no created time"))?;
    let posting_date = date_of_epoch(created);

    // The entry's three signed credit amounts: revenue, fee expense and clearing sum to zero.
    let (revenue, expense, clearing, suspense, label) = match category {
        "charge" => (Some(amount), Some(-fee), -net, false, "charge"),
        "refund" | "dispute" => (Some(amount), Some(-fee), -net, false, category),
        "fee" => (None, Some(amount), -amount, false, "fee"),
        other => (Some(amount), Some(-fee), -net, true, other),
    };
    let resource = resource_of(bt);
    let mut lines = Vec::new();
    let mut push = |account: &str, credit: i64, resource: Option<String>| {
        if credit == 0 {
            return;
        }
        lines.push(PlannedLine {
            account: account.to_owned(),
            debit: credit < 0,
            minor: credit.unsigned_abs().try_into().unwrap_or(i64::MAX),
            resource,
        });
    };
    let revenue_account = if suspense {
        accounts.suspense.as_str()
    } else {
        accounts.revenue.as_str()
    };
    if let Some(credit) = revenue {
        push(revenue_account, credit, resource.clone());
    }
    if let Some(credit) = expense {
        push(&accounts.expense, credit, resource.clone());
    }
    push(&accounts.clearing, clearing, resource.clone());
    if lines.is_empty() {
        return Ok(None);
    }
    let mut description = format!("Stripe {label} {id}");
    if let Some((currency_code, minor)) = presentment(bt)
        && currency_code != currency
    {
        let _ = write!(
            description,
            " presented as {currency_code} {minor} (minor units)"
        );
    }
    if suspense {
        description.push_str(" (unknown category: suspense)");
    }
    let entry = PlannedEntry {
        provider_id: format!("stripe-bt:{id}"),
        posting_date,
        currency,
        description: sanitize_vendor_text(&description),
        lines,
        suspense,
    };
    if !entry.balances() {
        return Err(format!(
            "balance transaction {id} did not balance as an entry"
        ));
    }
    Ok(Some(entry))
}

/// The allocation resource of a transaction: its product or unit metadata on the
/// charge or refund, else its Connect account, else none.
fn resource_of(bt: &Value) -> Option<String> {
    let metadata = bt.pointer("/source/metadata");
    metadata
        .and_then(|m| m.get("product").or_else(|| m.get("unit")))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The presentment currency and amount of the charge or refund, when expanded.
fn presentment(bt: &Value) -> Option<(String, i64)> {
    let source = bt.get("source")?;
    if !source.is_object() {
        return None;
    }
    let currency = source
        .get("currency")
        .and_then(Value::as_str)?
        .to_uppercase();
    let minor = source.get("amount").and_then(Value::as_i64)?;
    Some((currency, minor))
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
