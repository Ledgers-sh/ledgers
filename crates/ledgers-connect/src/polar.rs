//! The Polar connector (issue #142, payments): orders and refunds as balanced entries.
//!
//! Source: the orders and refunds of the organization, `GET https://api.polar.sh/v1/orders/`
//! and `GET https://api.polar.sh/v1/refunds/`, paged by `page` with `limit=100` while
//! `pagination.max_page` allows. Money is in integer minor units; the currency is lower
//! case. An organization access token with `orders:read` and `refunds:read` is the least
//! privilege. Source: <https://polar.sh/docs/api-reference/orders/list>
//!
//! Each order (total `T`, tax `X`, platform fee `F`) posts to the **Polar balance** clearing
//! account, in its currency:
//!
//! - revenue: `T - X` (or `T` with no tax account), credit
//! - tax: `X` credit, when a tax account is configured
//! - fee expense: `F`, debit
//! - Polar balance: `T - F`, debit
//!
//! Each refund (amount `R`, tax `Xr`) reverses it: revenue `R - Xr` debit, tax `Xr` debit,
//! Polar balance `R` credit.
//!
//! Polar has no payout endpoint in its public API, so the payouts are not read. A payout
//! reaches the bank as a bank line, and it clears the Polar balance when the bank line is
//! matched through the suggested bank rule. The clearing account reaches zero then.
//!
//! The balance is kept in the currency of the order. An order's product is its allocation
//! resource; a refund has no product of its own, so it allocates to the default unit.

use crate::connector::{Connector, CredentialField, SyncContext, config_text, is_date};
use crate::cost::{date_of_epoch, epoch_seconds};
use crate::http::{VendorError, sanitize_vendor_text};
use crate::model::{PlannedEntry, PlannedLine, SyncOutput};
use async_trait::async_trait;
use serde_json::Value;

const HOST: &str = "api.polar.sh";
const HOSTS: &[&str] = &[HOST];
/// Rows per page (the endpoint's maximum).
const LIMIT: u32 = 100;
/// Pages followed per list per sync.
const MAX_PAGES: u32 = 200;

/// The accounts a Polar connection posts to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accounts {
    /// The Polar balance clearing account.
    pub clearing: String,
    /// The revenue account.
    pub revenue: String,
    /// The fee expense account (the common `expense_account` key).
    pub expense: String,
    /// The tax account, when the connection keeps tax apart from revenue.
    pub tax: Option<String>,
}

/// The Polar connector.
pub struct Polar;

#[async_trait]
impl Connector for Polar {
    fn id(&self) -> &'static str {
        "polar"
    }

    fn display_name(&self) -> &'static str {
        "Polar"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "token",
            label: "Organization access token (orders:read, refunds:read)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        HOSTS
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        for key in ["clearing_account", "revenue_account"] {
            config_text(config, key)?;
        }
        if let Some(tax) = config.get("tax_account").and_then(Value::as_str)
            && tax.trim().is_empty()
        {
            return Err("tax_account is blank: leave it out or name a postable account".to_owned());
        }
        Ok(())
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let accounts = accounts_of(ctx.config).map_err(|detail| malformed(&detail))?;
        if !is_date(ctx.since) || !is_date(ctx.until) {
            return Err(malformed("the window is not two dates"));
        }
        let headers = vec![("Authorization", format!("Bearer {}", ctx.credential))];
        let mut out = SyncOutput::default();
        let mut orders_seen = 0_usize;
        let mut refunds_seen = 0_usize;

        for item in list(ctx, "orders", &headers).await? {
            let date = item
                .get("created_at")
                .and_then(Value::as_str)
                .and_then(|text| text.get(0..10))
                .unwrap_or_default();
            if date < ctx.since || date > ctx.until {
                continue;
            }
            orders_seen += 1;
            match plan_order(&item, &accounts) {
                Ok(Some(entry)) => out.entries.push(entry),
                Ok(None) => {}
                Err(detail) => out.notes.push(sanitize_vendor_text(&detail)),
            }
        }
        for item in list(ctx, "refunds", &headers).await? {
            let date = item
                .get("created_at")
                .and_then(Value::as_str)
                .and_then(|text| text.get(0..10))
                .unwrap_or_default();
            if date < ctx.since || date > ctx.until {
                continue;
            }
            refunds_seen += 1;
            match plan_refund(&item, &accounts) {
                Ok(Some(entry)) => out.entries.push(entry),
                Ok(None) => {}
                Err(detail) => out.notes.push(sanitize_vendor_text(&detail)),
            }
        }
        if accounts.tax.is_none() {
            out.notes
                .push("no tax account is configured: tax is inside revenue".to_owned());
        }
        out.notes.push(format!(
            "{orders_seen} order(s) and {refunds_seen} refund(s) in the window; Polar payouts are cleared by the bank rule"
        ));
        Ok(out)
    }
}

/// Every item of one list, following `page` while the answer says more pages exist.
async fn list(
    ctx: &SyncContext<'_>,
    resource: &str,
    headers: &[(&str, String)],
) -> Result<Vec<Value>, VendorError> {
    let mut items = Vec::new();
    for page in 1..=MAX_PAGES {
        let url = format!("https://{HOST}/v1/{resource}/?limit={LIMIT}&page={page}");
        let answer = ctx.http.get_json(HOSTS, &url, headers).await?;
        let batch = answer
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed(&format!("the {resource} list has no items")))?;
        items.extend(batch.iter().cloned());
        let max_page = answer
            .pointer("/pagination/max_page")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        if u64::from(page) >= max_page || batch.is_empty() {
            return Ok(items);
        }
    }
    Err(malformed(&format!(
        "the {resource} list is longer than Ledgers reads at once"
    )))
}

/// One order as a balanced entry, or `None` for an order with no amount.
///
/// # Errors
/// A sentence naming the order when its amounts are missing or do not add up.
pub fn plan_order(order: &Value, accounts: &Accounts) -> Result<Option<PlannedEntry>, String> {
    let id = order_id(order, "order")?;
    let total = amount(order, "total_amount", &id)?;
    let tax = order.get("tax_amount").and_then(Value::as_i64).unwrap_or(0);
    let fee = order
        .get("platform_fee_amount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if total == 0 {
        return Ok(None);
    }
    if tax < 0 || fee < 0 || tax > total || fee > total {
        return Err(format!("order {id} has a tax or fee outside its total"));
    }
    let currency = currency_of(order, &id)?;
    let posting_date = posting_date(order, &id)?;
    let resource = resource_of(order);
    // Credits, signed: revenue (and tax) credit the total; fee debits; the balance debits the net.
    let mut lines = Vec::new();
    let revenue_credit = match &accounts.tax {
        Some(tax_account) => {
            push(&mut lines, tax_account, tax, resource.clone());
            total - tax
        }
        None => total,
    };
    push(
        &mut lines,
        &accounts.revenue,
        revenue_credit,
        resource.clone(),
    );
    push(&mut lines, &accounts.expense, -fee, resource.clone());
    push(
        &mut lines,
        &accounts.clearing,
        -(total - fee),
        resource.clone(),
    );
    entry(
        format!("polar-order:{id}"),
        posting_date,
        currency,
        &format!("Polar order {id}"),
        lines,
    )
}

/// One refund as a balanced entry: it reverses revenue (and tax) and credits the Polar balance.
///
/// # Errors
/// A sentence naming the refund when its amount is missing or its tax is out of range.
pub fn plan_refund(refund: &Value, accounts: &Accounts) -> Result<Option<PlannedEntry>, String> {
    let id = order_id(refund, "refund")?;
    let amount_minor = amount(refund, "amount", &id)?;
    let tax = refund
        .get("tax_amount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if amount_minor == 0 {
        return Ok(None);
    }
    if tax < 0 || tax > amount_minor {
        return Err(format!("refund {id} has a tax outside its amount"));
    }
    let currency = currency_of(refund, &id)?;
    let posting_date = posting_date(refund, &id)?;
    let mut lines = Vec::new();
    let revenue_credit = match &accounts.tax {
        Some(tax_account) => {
            push(&mut lines, tax_account, -tax, None);
            -(amount_minor - tax)
        }
        None => -amount_minor,
    };
    push(&mut lines, &accounts.revenue, revenue_credit, None);
    push(&mut lines, &accounts.clearing, amount_minor, None);
    entry(
        format!("polar-refund:{id}"),
        posting_date,
        currency,
        &format!("Polar refund {id}"),
        lines,
    )
}

/// Adds the line for a signed credit amount: positive credits the account, negative debits it.
fn push(lines: &mut Vec<PlannedLine>, account: &str, credit: i64, resource: Option<String>) {
    if credit == 0 {
        return;
    }
    lines.push(PlannedLine {
        account: account.to_owned(),
        debit: credit < 0,
        minor: credit.unsigned_abs().try_into().unwrap_or(i64::MAX),
        resource,
    });
}

fn entry(
    provider_id: String,
    posting_date: String,
    currency: String,
    description: &str,
    lines: Vec<PlannedLine>,
) -> Result<Option<PlannedEntry>, String> {
    if lines.is_empty() {
        return Ok(None);
    }
    let entry = PlannedEntry {
        provider_id,
        posting_date,
        currency,
        description: sanitize_vendor_text(description),
        lines,
        suspense: false,
    };
    if !entry.balances() {
        return Err(format!("{} did not balance as an entry", entry.provider_id));
    }
    Ok(Some(entry))
}

fn order_id(item: &Value, kind: &str) -> Result<String, String> {
    item.get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("a Polar {kind} has no id and was skipped"))
}

fn amount(item: &Value, key: &str, id: &str) -> Result<i64, String> {
    item.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{id} has no whole {key} in minor units"))
}

fn currency_of(item: &Value, id: &str) -> Result<String, String> {
    item.get("currency")
        .and_then(Value::as_str)
        .filter(|code| code.len() == 3)
        .map(str::to_uppercase)
        .ok_or_else(|| format!("{id} has no currency"))
}

fn posting_date(item: &Value, id: &str) -> Result<String, String> {
    let created = item
        .get("created_at")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{id} has no created_at"))?;
    let day = created.get(0..10).unwrap_or_default();
    if is_date(day) {
        Ok(day.to_owned())
    } else {
        // An RFC 3339 timestamp with a time zone: the date in UTC.
        let seconds =
            epoch_seconds(created).ok_or_else(|| format!("{id} has an unreadable created_at"))?;
        Ok(date_of_epoch(seconds))
    }
}

fn resource_of(order: &Value) -> Option<String> {
    order
        .get("product_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn accounts_of(config: &Value) -> Result<Accounts, String> {
    Ok(Accounts {
        clearing: config_text(config, "clearing_account")?.to_owned(),
        revenue: config_text(config, "revenue_account")?.to_owned(),
        expense: config_text(config, "expense_account")?.to_owned(),
        tax: config
            .get("tax_account")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|account| !account.is_empty())
            .map(str::to_owned),
    })
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
    use serde_json::json;

    fn accounts(tax: Option<&str>) -> Accounts {
        Accounts {
            clearing: "1045".to_owned(),
            revenue: "4010".to_owned(),
            expense: "5100".to_owned(),
            tax: tax.map(str::to_owned),
        }
    }

    #[test]
    fn an_order_books_revenue_fee_and_the_net_to_the_polar_balance() {
        let order = json!({"id": "ord_1", "total_amount": 10000, "tax_amount": 0, "platform_fee_amount": 400,
                           "currency": "eur", "created_at": "2026-08-01T10:00:00Z", "product_id": "prod_a"});
        let entry = plan_order(&order, &accounts(None)).unwrap().unwrap();
        assert!(entry.balances());
        assert_eq!(entry.currency, "EUR");
        assert_eq!(entry.posting_date, "2026-08-01");
        let by = |account: &str| {
            entry
                .lines
                .iter()
                .find(|l| l.account == account)
                .cloned()
                .unwrap()
        };
        assert!(!by("4010").debit && by("4010").minor == 10000);
        assert!(by("5100").debit && by("5100").minor == 400);
        assert!(by("1045").debit && by("1045").minor == 9600);
        assert_eq!(by("4010").resource.as_deref(), Some("prod_a"));
    }

    #[test]
    fn tax_goes_to_its_own_account_when_one_is_configured() {
        let order = json!({"id": "ord_2", "total_amount": 12100, "tax_amount": 2100, "platform_fee_amount": 0,
                           "currency": "eur", "created_at": "2026-08-02T10:00:00Z"});
        let entry = plan_order(&order, &accounts(Some("2300")))
            .unwrap()
            .unwrap();
        assert!(entry.balances());
        let tax = entry.lines.iter().find(|l| l.account == "2300").unwrap();
        assert!(!tax.debit && tax.minor == 2100);
        let revenue = entry.lines.iter().find(|l| l.account == "4010").unwrap();
        assert!(!revenue.debit && revenue.minor == 10000);
    }

    #[test]
    fn a_refund_reverses_revenue_and_credits_the_polar_balance() {
        let refund = json!({"id": "ref_1", "amount": 4000, "tax_amount": 0, "currency": "eur",
                            "created_at": "2026-08-03T10:00:00Z", "order_id": "ord_1"});
        let entry = plan_refund(&refund, &accounts(None)).unwrap().unwrap();
        assert!(entry.balances());
        assert_eq!(entry.provider_id, "polar-refund:ref_1");
        let revenue = entry.lines.iter().find(|l| l.account == "4010").unwrap();
        assert!(revenue.debit && revenue.minor == 4000);
        let balance = entry.lines.iter().find(|l| l.account == "1045").unwrap();
        assert!(!balance.debit && balance.minor == 4000);
    }

    #[test]
    fn an_amount_that_is_not_whole_or_a_tax_out_of_range_is_refused() {
        let bad = json!({"id": "ord_3", "total_amount": "100.00", "currency": "eur", "created_at": "2026-08-01T00:00:00Z"});
        assert!(plan_order(&bad, &accounts(None)).is_err());
        let tax_over = json!({"id": "ord_4", "total_amount": 100, "tax_amount": 200, "currency": "eur", "created_at": "2026-08-01T00:00:00Z"});
        assert!(plan_order(&tax_over, &accounts(None)).is_err());
    }

    #[test]
    fn config_names_the_accounts_and_the_token_is_the_only_secret() {
        let config = json!({"clearing_account": "1045", "revenue_account": "4010", "expense_account": "5100"});
        assert!(Polar.validate_config(&config).is_ok());
        assert!(
            Polar
                .validate_config(&json!({"revenue_account": "4010"}))
                .is_err()
        );
        assert_eq!(Polar.allowed_hosts(), &["api.polar.sh"]);
        assert_eq!(Polar.credential_fields()[0].name, "token");
    }
}
