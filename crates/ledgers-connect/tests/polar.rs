//! The Polar connector against recorded answers (issue #142, payments). No test reaches
//! Polar: each answer is an inline page, and every request is recorded.

mod support;

use ledgers_connect::connector::{Connector, SyncContext};
use ledgers_connect::http::VendorError;
use ledgers_connect::model::{PlannedEntry, SyncOutput};
use ledgers_connect::polar::Polar;
use serde_json::{Value, json};
use support::{Recorded, vendor};

const TOKEN: &str = "polar_oat_test_not_real";

fn config() -> Value {
    json!({"clearing_account": "1045", "revenue_account": "4010", "expense_account": "5100"})
}

fn orders_page(max_page: u32, page: u32) -> String {
    let items = if page == 1 {
        json!([{"id": "ord_1", "total_amount": 10000, "tax_amount": 0, "platform_fee_amount": 400,
                "currency": "eur", "created_at": "2026-08-01T10:00:00Z", "product_id": "prod_a"},
               {"id": "ord_old", "total_amount": 500, "tax_amount": 0, "platform_fee_amount": 0,
                "currency": "eur", "created_at": "2026-06-01T10:00:00Z", "product_id": null}])
    } else {
        json!([{"id": "ord_2", "total_amount": 2000, "tax_amount": 0, "platform_fee_amount": 80,
                "currency": "eur", "created_at": "2026-08-02T10:00:00Z", "product_id": "prod_a"}])
    };
    json!({"items": items, "pagination": {"total_count": 3, "max_page": max_page}}).to_string()
}

const REFUNDS: &str = r#"{"items":[{"id":"ref_1","amount":4000,"tax_amount":0,"currency":"eur","created_at":"2026-08-03T10:00:00Z","order_id":"ord_1"}],"pagination":{"total_count":1,"max_page":1}}"#;

#[tokio::test]
async fn sync_pages_orders_and_refunds_and_books_only_the_window() {
    let client = Recorded::script(vec![
        (200, &orders_page(2, 1)),
        (200, &orders_page(2, 2)),
        (200, REFUNDS),
    ]);
    let (http, _) = vendor(client.clone());
    let config = config();
    let out: SyncOutput = Polar
        .sync(&SyncContext {
            http: &http,
            credential: TOKEN,
            now: "2026-10-24T10:00:00Z",
            config: &config,
            since: "2026-08-01",
            until: "2026-08-31",
        })
        .await
        .expect("sync succeeds");
    let ids: Vec<&str> = out.entries.iter().map(|e| e.provider_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "polar-order:ord_1",
            "polar-order:ord_2",
            "polar-refund:ref_1"
        ],
        "the June order is outside the window"
    );
    assert!(out.entries.iter().all(PlannedEntry::balances));
    let requests = client.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[0]
            .1
            .starts_with("https://api.polar.sh/v1/orders/?limit=100&page=1"),
        "{}",
        requests[0].1
    );
    assert!(requests[1].1.contains("page=2"));
    assert!(requests[2].1.contains("/v1/refunds/"));
    assert!(requests.iter().all(|r| r.2 == format!("Bearer {TOKEN}")));
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("tax is inside revenue"))
    );
}

#[tokio::test]
async fn sync_maps_refusals_and_retries_throttling() {
    let client = Recorded::script(vec![(401, "{}")]);
    let (http, _) = vendor(client.clone());
    let config = config();
    let ctx = SyncContext {
        http: &http,
        credential: TOKEN,
        now: "2026-10-24T10:00:00Z",
        config: &config,
        since: "2026-08-01",
        until: "2026-08-31",
    };
    assert!(matches!(
        Polar.sync(&ctx).await,
        Err(VendorError::Refused { status: 401, .. })
    ));
    assert_eq!(client.calls(), 1);
    let client = Recorded::script(vec![(429, "{}"), (429, "{}"), (429, "{}")]);
    let (http, pause) = vendor(client.clone());
    let ctx = SyncContext {
        http: &http,
        credential: TOKEN,
        now: "2026-10-24T10:00:00Z",
        config: &config,
        since: "2026-08-01",
        until: "2026-08-31",
    };
    assert!(matches!(
        Polar.sync(&ctx).await,
        Err(VendorError::Status { status: 429, .. })
    ));
    assert_eq!(client.calls(), 3);
    assert_eq!(pause.0.lock().unwrap().len(), 2);
}
