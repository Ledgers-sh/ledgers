//! The Stripe connector against recorded answers (issue #142, payments). No test
//! reaches Stripe: each answer is an inline page, and every request is recorded.

mod support;

use ledgers_connect::connector::{Connector, SyncContext};
use ledgers_connect::http::VendorError;
use ledgers_connect::model::{PlannedEntry, SyncOutput};
use ledgers_connect::stripe::{Accounts, Stripe, plan_transaction};
use serde_json::{Value, json};
use std::sync::Arc;
use support::{Recorded, RecordedPause, vendor};

const NOW: &str = "2026-10-24T10:00:00Z";
const TOKEN: &str = "rk_test_not_a_real_key";

fn accounts() -> Accounts {
    Accounts {
        clearing: "1040".to_owned(),
        revenue: "8400".to_owned(),
        expense: "6850".to_owned(),
        suspense: "1990".to_owned(),
        connect: None,
    }
}

fn config() -> Value {
    json!({
        "clearing_account": "1040",
        "revenue_account": "8400",
        "suspense_account": "1990",
        "expense_account": "6850"
    })
}

/// A charge of 100.00 EUR (presented as 110.00 USD) with a 3.20 fee, tagged with a product.
fn charge() -> Value {
    json!({
        "id": "txn_charge_1", "amount": 10000, "fee": 320, "net": 9680, "currency": "eur",
        "type": "charge", "reporting_category": "charge", "created": 1_785_542_400,
        "source": {"object": "charge", "id": "ch_1", "amount": 11000, "currency": "usd",
                   "metadata": {"product": "ledgers-pro"}}
    })
}

/// A refund of 40.00 EUR (presented as 44.00 USD).
fn refund() -> Value {
    json!({
        "id": "txn_refund_1", "amount": -4000, "fee": 0, "net": -4000, "currency": "eur",
        "type": "refund", "reporting_category": "refund", "created": 1_785_628_800,
        "source": {"object": "refund", "id": "re_1", "amount": 4400, "currency": "usd", "metadata": {}}
    })
}

/// A payout of 56.80 EUR: left to the bank rule.
fn payout() -> Value {
    json!({
        "id": "txn_payout_1", "amount": -5680, "fee": 0, "net": -5680, "currency": "eur",
        "type": "payout", "reporting_category": "payout", "created": 1_785_715_200,
        "source": {"object": "payout", "id": "po_1"}
    })
}

/// A category this connector does not know.
fn unknown() -> Value {
    json!({
        "id": "txn_tr_1", "amount": 500, "fee": 0, "net": 500, "currency": "eur",
        "type": "transfer", "reporting_category": "transfer", "created": 1_785_715_200,
        "source": null
    })
}

fn line_summary(entry: &PlannedEntry) -> Vec<(String, bool, i64)> {
    let mut lines: Vec<(String, bool, i64)> = entry
        .lines
        .iter()
        .map(|line| (line.account.clone(), line.debit, line.minor))
        .collect();
    lines.sort();
    lines
}

#[test]
fn a_charge_books_revenue_fee_and_the_net_to_clearing_and_balances() {
    let entry = plan_transaction(&charge(), &accounts())
        .expect("planned")
        .expect("an entry");
    assert!(entry.balances());
    assert_eq!(entry.provider_id, "stripe-bt:txn_charge_1");
    assert_eq!(entry.posting_date, "2026-08-01");
    assert_eq!(entry.currency, "EUR", "the settlement currency");
    assert!(!entry.suspense);
    assert_eq!(
        line_summary(&entry),
        vec![
            ("1040".to_owned(), true, 9680),
            ("6850".to_owned(), true, 320),
            ("8400".to_owned(), false, 10000),
        ]
    );
    assert!(
        entry
            .description
            .contains("presented as USD 11000 (minor units)"),
        "{}",
        entry.description
    );
    let resource = entry
        .lines
        .iter()
        .find(|l| l.account == "8400")
        .and_then(|l| l.resource.clone());
    assert_eq!(
        resource.as_deref(),
        Some("ledgers-pro"),
        "the product is the allocation resource"
    );
}

#[test]
fn a_refund_reverses_revenue_and_credits_clearing() {
    let entry = plan_transaction(&refund(), &accounts())
        .expect("planned")
        .expect("an entry");
    assert!(entry.balances());
    assert_eq!(
        line_summary(&entry),
        vec![
            ("1040".to_owned(), false, 4000),
            ("8400".to_owned(), true, 4000)
        ]
    );
    assert!(entry.description.contains("presented as USD 4400"));
}

#[test]
fn a_payout_posts_nothing_and_an_unknown_category_goes_to_suspense() {
    assert!(
        plan_transaction(&payout(), &accounts())
            .expect("planned")
            .is_none()
    );
    let entry = plan_transaction(&unknown(), &accounts())
        .expect("planned")
        .expect("an entry");
    assert!(entry.suspense);
    assert!(entry.balances());
    assert!(
        entry.lines.iter().any(|l| l.account == "1990"),
        "the suspense account holds it"
    );
    assert!(
        !entry.lines.iter().any(|l| l.account == "8400"),
        "it never reaches revenue"
    );
    assert!(entry.description.contains("unknown category: suspense"));
}

#[test]
fn a_transaction_that_does_not_add_up_is_refused_not_booked() {
    let mut bad = charge();
    bad["net"] = json!(9000);
    let problem = plan_transaction(&bad, &accounts()).expect_err("refused");
    assert!(problem.contains("does not add up"), "{problem}");
}

fn ctx<'a>(http: &'a ledgers_connect::http::VendorHttp, config: &'a Value) -> SyncContext<'a> {
    SyncContext {
        http,
        credential: TOKEN,
        now: NOW,
        config,
        since: "2026-08-01",
        until: "2026-08-31",
    }
}

#[tokio::test]
async fn sync_pages_the_balance_transactions_and_plans_one_entry_each() {
    let page_one =
        json!({"object": "list", "data": [charge(), refund()], "has_more": true}).to_string();
    let page_two =
        json!({"object": "list", "data": [payout(), unknown()], "has_more": false}).to_string();
    let client = Recorded::script(vec![(200, &page_one), (200, &page_two)]);
    let (http, _) = vendor(client.clone());
    let config = config();
    let out: SyncOutput = Stripe
        .sync(&ctx(&http, &config))
        .await
        .expect("sync succeeds");
    assert_eq!(
        out.entries.len(),
        3,
        "charge, refund and the unknown category; the payout is left to the bank rule"
    );
    assert!(out.entries.iter().all(PlannedEntry::balances));
    let requests = client.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].1.contains("created[gte]=1785542400"),
        "{}",
        requests[0].1
    );
    assert!(
        requests[0].1.contains("created[lt]=1788220800"),
        "{}",
        requests[0].1
    );
    assert!(
        requests[0].1.contains("expand[]=data.source"),
        "{}",
        requests[0].1
    );
    assert!(requests[0].1.contains("limit=100"));
    assert!(!requests[0].1.contains("starting_after"));
    assert!(
        requests[1].1.contains("starting_after=txn_refund_1"),
        "{}",
        requests[1].1
    );
    assert_eq!(requests[0].2, format!("Bearer {TOKEN}"));
}

#[tokio::test]
async fn sync_maps_refusals_and_retries_throttling_with_backoff() {
    let client = Recorded::script(vec![(401, "{}")]);
    let (http, _) = vendor(client.clone());
    let config = config();
    let out = Stripe.sync(&ctx(&http, &config)).await;
    assert!(matches!(out, Err(VendorError::Refused { status: 401, .. })));
    assert_eq!(client.calls(), 1);

    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let (http, pause): (_, Arc<RecordedPause>) = vendor(client.clone());
    let out = Stripe.sync(&ctx(&http, &config)).await;
    assert!(matches!(out, Err(VendorError::Status { status: 429, .. })));
    assert_eq!(
        client.calls(),
        3,
        "throttling is retried to the attempt limit"
    );
    let waits = pause.0.lock().unwrap().clone();
    assert_eq!(waits.len(), 2, "the waits between attempts are recorded");
    assert!(waits[1] > waits[0], "the wait grows between attempts");

    let client = Recorded::script(vec![(500, "{}"), (503, "{}"), (500, "{}")]);
    let (http, _) = vendor(client);
    let out = Stripe.sync(&ctx(&http, &config)).await;
    assert!(matches!(out, Err(VendorError::Status { status: 500, .. })));
}

#[test]
fn config_names_the_three_accounts_and_an_optional_connect_account() {
    assert!(Stripe.validate_config(&config()).is_ok());
    let mut no_clearing = config();
    no_clearing
        .as_object_mut()
        .unwrap()
        .remove("clearing_account");
    assert!(Stripe.validate_config(&no_clearing).is_err());
    let mut connect = config();
    connect["stripe_account"] = json!("acct_1Example");
    assert!(Stripe.validate_config(&connect).is_ok());
    connect["stripe_account"] = json!("cus_123");
    assert!(Stripe.validate_config(&connect).is_err());
    assert_eq!(Stripe.allowed_hosts(), &["api.stripe.com"]);
    assert_eq!(Stripe.credential_fields()[0].name, "token");
}

#[test]
fn a_connect_account_is_the_allocation_resource_when_a_charge_has_no_product() {
    let mut connected = accounts();
    connected.connect = Some("acct_1Example".to_owned());
    let mut plain = charge();
    plain["source"]["metadata"] = json!({});
    let entry = plan_transaction(&plain, &connected)
        .expect("planned")
        .expect("an entry");
    let revenue = entry
        .lines
        .iter()
        .find(|l| l.account == "8400")
        .expect("revenue line");
    assert_eq!(revenue.resource.as_deref(), Some("acct_1Example"));
    // A product still wins over the account.
    let tagged = plan_transaction(&charge(), &connected)
        .expect("planned")
        .expect("an entry");
    let revenue = tagged
        .lines
        .iter()
        .find(|l| l.account == "8400")
        .expect("revenue line");
    assert_eq!(revenue.resource.as_deref(), Some("ledgers-pro"));
}
