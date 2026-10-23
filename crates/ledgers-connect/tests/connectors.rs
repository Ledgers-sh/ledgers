//! Connector contracts against recorded fixtures (issue #142). No test reaches a
//! vendor: every answer comes from `fixtures/`, and every request is recorded.

mod support;

use ledgers_connect::anthropic::Anthropic;
use ledgers_connect::cloudflare::Cloudflare;
use ledgers_connect::connector::{Connector, SyncContext, by_id, month_windows};
use ledgers_connect::http::VendorError;
use ledgers_connect::model::SyncOutput;
use ledgers_connect::planetscale::PlanetScale;
use serde_json::{Value, json};
use support::{Recorded, all_routes, vendor};

fn ctx<'a>(
    http: &'a ledgers_connect::http::VendorHttp,
    credential: &'a str,
    config: &'a Value,
    since: &'a str,
    until: &'a str,
) -> SyncContext<'a> {
    SyncContext {
        http,
        credential,
        config,
        since,
        until,
    }
}

async fn sync_with(
    connector: &dyn Connector,
    config: &Value,
    since: &str,
    until: &str,
) -> (Result<SyncOutput, VendorError>, std::sync::Arc<Recorded>) {
    let client = Recorded::new(all_routes());
    let (http, _) = vendor(client.clone());
    let out = connector
        .sync(&ctx(&http, "test-credential-value", config, since, until))
        .await;
    (out, client)
}

#[test]
fn the_three_connectors_are_registered_with_their_hosts_and_fields() {
    for (id, host) in [
        ("cloudflare", "api.cloudflare.com"),
        ("anthropic", "api.anthropic.com"),
        ("planetscale", "api.planetscale.com"),
    ] {
        let connector = by_id(id).expect("registered");
        assert_eq!(connector.allowed_hosts(), &[host]);
        assert_eq!(connector.credential_fields().len(), 1);
        assert_eq!(connector.credential_fields()[0].name, "token");
    }
    assert!(by_id("unknown").is_none());
}

#[test]
fn config_is_checked_before_a_sync() {
    assert!(
        Cloudflare
            .validate_config(&json!({"account_id": "abc123", "expense_account": "6100"}))
            .is_ok()
    );
    assert!(
        Cloudflare
            .validate_config(&json!({"account_id": "has space"}))
            .is_err()
    );
    assert!(Cloudflare.validate_config(&json!({})).is_err());
    assert!(
        PlanetScale
            .validate_config(&json!({"organization": "acme", "currency": "USD"}))
            .is_ok()
    );
    assert!(
        PlanetScale
            .validate_config(&json!({"organization": "bad/slug", "currency": "USD"}))
            .is_err()
    );
    assert!(
        PlanetScale
            .validate_config(&json!({"organization": "acme", "currency": "dollars"}))
            .is_err()
    );
    assert!(
        Anthropic.validate_config(&json!({})).is_err(),
        "expense account is required"
    );
}

#[tokio::test]
async fn cloudflare_imports_billed_charges_and_skips_credits_and_out_of_window_items() {
    let (out, client) = sync_with(
        &Cloudflare,
        &json!({"account_id": "0123456789abcdef0123456789abcdef", "expense_account": "6100"}),
        "2026-07-01",
        "2026-08-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    let mut ids: Vec<&str> = out
        .invoices
        .iter()
        .map(|i| i.external_id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "cloudflare-billing:cf-bill-2026-0701",
            "cloudflare-billing:cf-bill-2026-0801",
            "cloudflare-billing:cf-bill-2026-0815"
        ]
    );
    let august = out
        .invoices
        .iter()
        .find(|i| i.external_id.ends_with("0801"))
        .unwrap();
    assert_eq!(august.total_minor, 2_500, "25.00 USD is exact minor units");
    assert_eq!(august.period, "2026-08");
    assert_eq!(august.invoice_number.as_deref(), Some("CF-INV-2026-08"));
    assert_eq!(
        august.hosted_url.as_deref(),
        Some("https://dash.cloudflare.com/billing/invoices/inv-cf-2026-08")
    );
    let r2 = out
        .invoices
        .iter()
        .find(|i| i.external_id.ends_with("0815"))
        .unwrap();
    assert_eq!(r2.total_minor, 1_234);
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("credit") && n.contains("cf-credit-2026-0820"))
    );
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("R2 operations and storage are not read"))
    );
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("usage for 2026-07 is not available"))
    );
    assert!(
        out.notes
            .iter()
            .all(|n| !n.contains("test-credential-value"))
    );

    let workers: Vec<_> = out
        .usage
        .iter()
        .filter(|u| u.resource_kind == "worker")
        .collect();
    assert_eq!(workers.len(), 3);
    assert_eq!(workers.iter().map(|u| u.quantity).sum::<i64>(), 1_000_000);
    let d1 = out
        .usage
        .iter()
        .find(|u| u.resource_kind == "d1-database")
        .unwrap();
    assert_eq!(d1.quantity, 5_000, "reads plus writes");
    assert!(
        out.usage.iter().all(|u| u.period == "2026-08"),
        "the July query errored, so no July usage"
    );

    let requests = client.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .all(|(_, uri, _)| uri.starts_with("https://api.cloudflare.com/"))
    );
    assert!(requests.iter().any(|(method, uri, auth)| method == "GET"
        && uri.contains("/accounts/0123456789abcdef0123456789abcdef/billing/history")
        && auth == "Bearer test-credential-value"));
}

#[tokio::test]
async fn cloudflare_sync_is_repeatable_with_the_same_identifiers() {
    let config =
        json!({"account_id": "0123456789abcdef0123456789abcdef", "expense_account": "6100"});
    let (first, _) = sync_with(&Cloudflare, &config, "2026-07-01", "2026-08-31").await;
    let (second, _) = sync_with(&Cloudflare, &config, "2026-07-01", "2026-08-31").await;
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(
        first.invoices, second.invoices,
        "the same answer gives the same invoices"
    );
}

#[tokio::test]
async fn anthropic_sums_the_cost_report_exactly_and_rounds_once_per_month() {
    let (out, client) = sync_with(
        &Anthropic,
        &json!({"expense_account": "6200"}),
        "2026-07-01",
        "2026-07-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    assert_eq!(out.invoices.len(), 1);
    let july = &out.invoices[0];
    assert_eq!(july.external_id, "anthropic-cost:2026-07");
    assert_eq!(july.currency, "USD");
    // 123.78912 + 50.00001 = 173.78913 cents: rounded once, to 174 cents.
    assert_eq!(july.total_minor, 174);
    assert_eq!(july.period, "2026-07");

    let keys: Vec<(&str, i64)> = out
        .usage
        .iter()
        .map(|u| (u.resource_id.as_str(), u.quantity))
        .collect();
    assert!(keys.contains(&("apikey_ledgers_web", 1_500 + 200 + 500 + 1_000 + 300)));
    assert!(keys.contains(&("apikey_sqeezin", 600)));
    assert!(
        keys.contains(&("console", 150)),
        "usage without a key books to the console row"
    );

    let requests = client.requests.lock().unwrap();
    let cost = requests
        .iter()
        .find(|(_, uri, _)| uri.contains("cost_report"))
        .unwrap();
    assert!(
        cost.1.contains("ending_at=2026-08-01T00:00:00Z"),
        "the window is exclusive at the end"
    );
    assert_eq!(
        cost.2, "test-credential-value",
        "the admin key goes in x-api-key"
    );
}

#[tokio::test]
async fn anthropic_refuses_a_cost_report_in_another_currency() {
    let client = Recorded::new(vec![support::Route {
        url: "/organizations/cost_report",
        body: "",
        status: 200,
        answer: json!({"data":[{"starting_at":"2026-07-01T00:00:00Z","results":[{"amount":"100","currency":"EUR"}]}],"has_more":false,"next_page":null}).to_string(),
    }]);
    let (http, _) = vendor(client);
    let err = Anthropic
        .sync(&ctx(
            &http,
            "k",
            &json!({"expense_account": "6200"}),
            "2026-07-01",
            "2026-07-31",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, VendorError::NotAvailable(_)), "{err:?}");
}

#[tokio::test]
async fn planetscale_books_invoice_totals_and_says_what_it_cannot_split() {
    let (out, _) = sync_with(
        &PlanetScale,
        &json!({"organization": "acme-labs", "currency": "usd", "expense_account": "6300"}),
        "2026-07-01",
        "2026-08-31",
    )
    .await;
    let out = out.expect("sync succeeds");
    assert_eq!(out.invoices.len(), 2);
    let july = out.invoices.iter().find(|i| i.period == "2026-07").unwrap();
    assert_eq!(july.total_minor, 14_900);
    assert_eq!(july.currency, "USD", "the configured currency is used");
    assert_eq!(
        july.external_id,
        "planetscale-invoice:acme-labs:ps-inv-0007"
    );
    let august = out.invoices.iter().find(|i| i.period == "2026-08").unwrap();
    assert_eq!(august.total_minor, 16_250);
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("per-database cost is not available for this account"))
    );
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("invoices carry no currency"))
    );
    assert!(
        out.usage
            .iter()
            .all(|u| u.resource_kind == "organization" && u.quantity == 1)
    );
}

#[tokio::test]
async fn planetscale_window_filters_invoices_outside_it() {
    let (out, _) = sync_with(
        &PlanetScale,
        &json!({"organization": "acme-labs", "currency": "USD", "expense_account": "6300"}),
        "2026-08-01",
        "2026-08-31",
    )
    .await;
    let out = out.unwrap();
    assert_eq!(out.invoices.len(), 1);
    assert_eq!(out.invoices[0].period, "2026-08");
}

#[test]
fn windows_split_into_calendar_months() {
    assert_eq!(month_windows("2026-07-01", "2026-07-31").unwrap().len(), 1);
}
