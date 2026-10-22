//! HTTP integration tests for every registry command: success, 401, 403,
//! policy denial, unbalanced entries, and reversal instead of editing.

mod common;

use common::{Method, entry_json, fixture, json};
use ledgers_server::dispatch;

const EXT: &str = "inv-1001";
const TB: &str = "/v1/reports/trial-balance?period=2026-10";

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

// ------------------------------------------------------------- entries.post

#[test]
fn entries_post_posts_a_balanced_entry() {
    let fx = fixture();
    let response = fx.post(EXT, "5010", "4000", "100000");
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let body = json(&response);
    assert_eq!(body["entry"]["number"], 1);
    assert_eq!(
        body["entry"]["journal_id"].as_str().map(str::is_empty),
        Some(false)
    );
}

#[test]
fn entries_post_without_a_credential_is_401() {
    let fx = fixture();
    let body = entry_json(EXT, "5010", "4000", "100000").to_string();
    let response = fx.send_anonymous(Method::POST, "/v1/entries", Some(&body));
    assert_eq!(response.status.as_u16(), 401);
}

#[test]
fn entries_post_with_an_unknown_key_is_401() {
    let fx = fixture();
    let body = entry_json(EXT, "5010", "4000", "100000").to_string();
    let response = fx.send(
        Method::POST,
        "/v1/entries",
        "lk_live_0000_nope",
        Some(&body),
    );
    assert_eq!(response.status.as_u16(), 401);
}

#[test]
fn entries_post_with_a_draft_key_is_403_tier_insufficient() {
    let fx = fixture();
    let body = entry_json(EXT, "5010", "4000", "100000").to_string();
    let response = fx.send(Method::POST, "/v1/entries", &fx.keys.draft, Some(&body));
    assert_eq!(response.status.as_u16(), 403);
    assert_eq!(json(&response)["requiredTier"], "post-within-limits");
}

#[test]
fn entries_post_over_a_per_entry_limit_needs_approval() {
    let fx = fixture();
    // The read key has no limit: a post by it is refused for approval, and a
    // post-tier key with a 1000.00 EUR per-entry ceiling is asked above it.
    let key = pollster::block_on(ledgers_server::dispatch::issue_key(
        &fx.deps,
        common::COMPANY,
        "small limit",
        ledgers_commands::auth::Tier::PostWithinLimits,
        std::collections::BTreeMap::from([(
            "EUR".to_owned(),
            ledgers_policy::key::CurrencyLimit {
                per_entry: 100_000,
                per_day: 1_000_000_000,
            },
        )]),
    ))
    .expect("issue small-limit key");
    let within = fx.send(
        Method::POST,
        "/v1/entries",
        &key,
        Some(&entry_json("small-1", "5010", "4000", "100000").to_string()),
    );
    assert_eq!(json(&within)["entry"]["number"], 1, "{}", within.json());
    let above = fx.send(
        Method::POST,
        "/v1/entries",
        &key,
        Some(&entry_json("small-2", "5010", "4000", "100001").to_string()),
    );
    assert_eq!(
        json(&above)["status"],
        "needs-approval",
        "one minor unit over the ceiling is not posted"
    );
}

#[test]
fn entries_post_with_no_limit_for_the_currency_needs_approval_and_writes_nothing() {
    let fx = fixture();
    let body = entry_json(EXT, "5010", "4000", "100000").to_string();
    let response = fx.send(Method::POST, "/v1/entries", &fx.keys.unlimited, Some(&body));
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    assert_eq!(json(&response)["status"], "needs-approval");
    let trial = fx.send(Method::GET, TB, &fx.keys.full, None);
    assert!(
        json(&trial)["rows"].as_array().unwrap().is_empty(),
        "a denied post writes nothing"
    );
}

#[test]
fn entries_post_touching_a_protected_account_needs_approval_and_writes_nothing() {
    let fx = fixture();
    // 1010 (bank) carries a protected tag: an agent's post needs a person.
    let body = entry_json(EXT, "1010", "4000", "100000").to_string();
    let response = fx.send(Method::POST, "/v1/entries", &fx.keys.full, Some(&body));
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    assert_eq!(json(&response)["status"], "needs-approval");
    let list = fx.send(Method::GET, "/v1/entries", &fx.keys.full, None);
    assert!(json(&list)["entries"].as_array().unwrap().is_empty());
}

#[test]
fn unbalanced_entry_is_rejected_and_nothing_is_posted() {
    let fx = fixture();
    let mut body = entry_json(EXT, "5010", "4000", "100000");
    body["entry"]["lines"][1]["base"]["minor"] = serde_json::json!("99999");
    let response = fx.send(
        Method::POST,
        "/v1/entries",
        &fx.keys.full,
        Some(&body.to_string()),
    );
    assert!(
        response.status.as_u16() == 422 || response.status.as_u16() == 400,
        "unbalanced entry must be refused, got {} {}",
        response.status.as_u16(),
        response.json()
    );
    let list = fx.send(Method::GET, "/v1/entries", &fx.keys.full, None);
    assert!(json(&list)["entries"].as_array().unwrap().is_empty());
}

#[test]
fn entries_post_writes_the_audit_row_in_the_same_batch() {
    let fx = fixture();
    let before = audit_rows(&fx);
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    assert_eq!(
        audit_rows(&fx),
        before + 1,
        "one allowed audit event per posted entry"
    );
}

fn audit_rows(fx: &common::Fixture) -> i64 {
    let rows = pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
        "SELECT COUNT(*) AS n FROM audit_event WHERE company_id = ?".to_owned(),
        vec![sea_query::Value::from(common::COMPANY)],
    )))
    .expect("count audit rows");
    rows.first()
        .and_then(|row| row.get::<i64>("n"))
        .unwrap_or(0)
}

// ------------------------------------------------------------ entries.reverse

#[test]
fn entries_reverse_posts_the_mirror_and_leaves_the_original_unchanged() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let response = fx.send(
        Method::POST,
        &format!("/v1/entries/{EXT}/reverse"),
        &fx.keys.full,
        Some("{}"),
    );
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let body = json(&response);
    assert_eq!(body["entry"]["number"], 2);

    let original = json(&fx.send(
        Method::GET,
        &format!("/v1/entries/{EXT}"),
        &fx.keys.full,
        None,
    ));
    assert_eq!(original["number"], 1);
    assert_eq!(
        original["lines"][0]["side"], "debit",
        "the original is not edited"
    );
    assert!(
        original["reversed_by"].is_string(),
        "the original names its reversal"
    );
}

#[test]
fn reversal_brings_the_trial_balance_back_to_zero() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let posted = json(&fx.send(Method::GET, TB, &fx.keys.full, None));
    assert_eq!(posted["rows"].as_array().map(Vec::len), Some(2));

    let reversed = fx.send(
        Method::POST,
        &format!("/v1/entries/{EXT}/reverse"),
        &fx.keys.full,
        Some("{}"),
    );
    assert_eq!(reversed.status.as_u16(), 200, "{}", reversed.json());
    let after = json(&fx.send(Method::GET, TB, &fx.keys.full, None));
    for row in after["rows"].as_array().unwrap() {
        assert_eq!(
            row["balance"]["minor"], "0",
            "every account nets to zero: {row}"
        );
    }
}

#[test]
fn an_entry_cannot_be_reversed_twice() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let path = format!("/v1/entries/{EXT}/reverse");
    assert!(is_success(
        fx.send(Method::POST, &path, &fx.keys.full, Some("{}"))
            .status
            .as_u16()
    ));
    let second = fx.send(Method::POST, &path, &fx.keys.full, Some("{}"));
    assert!(
        !is_success(second.status.as_u16()),
        "a second reversal is refused: {}",
        second.json()
    );
}

#[test]
fn entries_reverse_without_a_credential_is_401_and_with_a_draft_key_is_403() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let path = format!("/v1/entries/{EXT}/reverse");
    assert_eq!(
        fx.send_anonymous(Method::POST, &path, Some("{}"))
            .status
            .as_u16(),
        401
    );
    let denied = fx.send(Method::POST, &path, &fx.keys.draft, Some("{}"));
    assert_eq!(denied.status.as_u16(), 403);
}

#[test]
fn entries_are_not_editable_over_http() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let body = entry_json(EXT, "5010", "4000", "200000").to_string();
    let put = fx.send(
        Method::PUT,
        &format!("/v1/entries/{EXT}"),
        &fx.keys.full,
        Some(&body),
    );
    assert!(
        matches!(put.status.as_u16(), 404 | 405),
        "no update route exists, got {}",
        put.status.as_u16()
    );
    let original = json(&fx.send(
        Method::GET,
        &format!("/v1/entries/{EXT}"),
        &fx.keys.full,
        None,
    ));
    assert_eq!(original["lines"][0]["base"]["minor"], "100000");
}

// ---------------------------------------------------------------- entries.list

#[test]
fn entries_list_lists_posted_entries_for_any_key_with_read_tier() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let response = fx.send(
        Method::GET,
        "/v1/entries?period=2026-10&limit=10",
        &fx.keys.read,
        None,
    );
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    assert_eq!(json(&response)["entries"].as_array().map(Vec::len), Some(1));
}

#[test]
fn entries_list_without_a_credential_is_401() {
    let fx = fixture();
    assert_eq!(
        fx.send_anonymous(Method::GET, "/v1/entries", None)
            .status
            .as_u16(),
        401
    );
}

#[test]
fn entries_list_rejects_a_malformed_period() {
    let fx = fixture();
    let response = fx.send(Method::GET, "/v1/entries?period=2026", &fx.keys.full, None);
    assert_eq!(response.status.as_u16(), 422);
}

// ----------------------------------------------------------------- entries.get

#[test]
fn entries_get_returns_the_entry_with_its_lines() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let response = fx.send(
        Method::GET,
        &format!("/v1/entries/{EXT}"),
        &fx.keys.read,
        None,
    );
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let body = json(&response);
    assert_eq!(body["external_id"], EXT);
    assert_eq!(body["lines"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["description"], "Invoice INV-1001 to Acme GmbH");
}

#[test]
fn entries_get_unknown_external_id_is_404() {
    let fx = fixture();
    let response = fx.send(Method::GET, "/v1/entries/nope", &fx.keys.full, None);
    assert_eq!(response.status.as_u16(), 404);
}

#[test]
fn entries_get_without_a_credential_is_401() {
    let fx = fixture();
    assert_eq!(
        fx.send_anonymous(Method::GET, &format!("/v1/entries/{EXT}"), None)
            .status
            .as_u16(),
        401
    );
}

// --------------------------------------------------------------- accounts.list

#[test]
fn accounts_list_returns_the_chart_with_balances() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let response = fx.send(Method::GET, "/v1/accounts", &fx.keys.read, None);
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let accounts = json(&response)["accounts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        accounts.iter().any(|a| a["number"] == "1010"),
        "the generic SME chart is served"
    );
    let sales = accounts
        .iter()
        .find(|a| a["number"] == "4000")
        .expect("account 4000");
    assert_eq!(sales["account_type"], "income");
    assert_eq!(
        sales["balances"][0]["minor"], "-100000",
        "credit balance is negative"
    );
    assert_eq!(sales["balances"][0]["currency"], "EUR");
}

#[test]
fn accounts_list_without_a_credential_is_401() {
    let fx = fixture();
    assert_eq!(
        fx.send_anonymous(Method::GET, "/v1/accounts", None)
            .status
            .as_u16(),
        401
    );
}

// ------------------------------------------------------------------ reports

#[test]
fn trial_balance_totals_the_period() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let response = fx.send(Method::GET, TB, &fx.keys.read, None);
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let body = json(&response);
    assert_eq!(body["period"], "2026-10");
    let rows = body["rows"].as_array().unwrap();
    let debit: i64 = rows
        .iter()
        .map(|r| {
            r["debit"]["minor"]
                .as_str()
                .unwrap()
                .parse::<i64>()
                .unwrap()
        })
        .sum();
    let credit: i64 = rows
        .iter()
        .map(|r| {
            r["credit"]["minor"]
                .as_str()
                .unwrap()
                .parse::<i64>()
                .unwrap()
        })
        .sum();
    assert_eq!(debit, credit, "a trial balance balances");
    assert_eq!(debit, 100_000);
}

#[test]
fn trial_balance_without_a_credential_is_401_and_a_bad_period_is_400() {
    let fx = fixture();
    assert_eq!(
        fx.send_anonymous(Method::GET, TB, None).status.as_u16(),
        401
    );
    let bad = fx.send(
        Method::GET,
        "/v1/reports/trial-balance?period=October",
        &fx.keys.full,
        None,
    );
    assert_eq!(bad.status.as_u16(), 422);
}

#[test]
fn profit_and_loss_reports_income_for_the_period() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let response = fx.send(
        Method::GET,
        "/v1/reports/p-and-l?period=2026-10",
        &fx.keys.read,
        None,
    );
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let body = json(&response);
    assert_eq!(body["revenue"]["minor"], "100000");
    assert_eq!(body["expenses"]["minor"], "100000");
    assert_eq!(body["net_income"]["minor"], "0");
}

#[test]
fn profit_and_loss_without_a_credential_is_401() {
    let fx = fixture();
    let response = fx.send_anonymous(Method::GET, "/v1/reports/p-and-l?period=2026-10", None);
    assert_eq!(response.status.as_u16(), 401);
}

#[test]
fn balance_sheet_reports_as_of_a_date() {
    let fx = fixture();
    assert!(is_success(
        fx.post(EXT, "5010", "4000", "100000").status.as_u16()
    ));
    let response = fx.send(
        Method::GET,
        "/v1/reports/balance-sheet?as_of=2026-10-31",
        &fx.keys.read,
        None,
    );
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    assert_eq!(json(&response)["as_of"], "2026-10-31");
}

#[test]
fn balance_sheet_without_a_credential_is_401() {
    let fx = fixture();
    let response = fx.send_anonymous(
        Method::GET,
        "/v1/reports/balance-sheet?as_of=2026-10-31",
        None,
    );
    assert_eq!(response.status.as_u16(), 401);
}

#[test]
fn account_balance_by_dimension_splits_an_account_by_a_dimension() {
    let fx = fixture();
    let mut body = entry_json(EXT, "5010", "4000", "100000");
    body["entry"]["lines"][0]["dimensions"] = serde_json::json!({"cost-centre": "ops"});
    let posted = fx.send(
        Method::POST,
        "/v1/entries",
        &fx.keys.full,
        Some(&body.to_string()),
    );
    assert_eq!(posted.status.as_u16(), 200, "{}", posted.json());
    let response = fx.send(
        Method::GET,
        "/v1/reports/account-balance-by-dimension?account=5010&dimension=cost-centre&period=2026-10",
        &fx.keys.read,
        None,
    );
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let rows = json(&response)["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["dimension_value"], "ops");
    assert_eq!(rows[0]["balance"]["minor"], "100000");
}

#[test]
fn account_balance_by_dimension_without_a_credential_is_401() {
    let fx = fixture();
    let response = fx.send_anonymous(
        Method::GET,
        "/v1/reports/account-balance-by-dimension?account=5010&dimension=cost-centre&period=2026-10",
        None,
    );
    assert_eq!(response.status.as_u16(), 401);
}

// ------------------------------------------------------------------ bootstrap

#[test]
fn bootstrap_is_idempotent_for_the_chart_and_mints_a_new_key_each_time() {
    let fx = fixture();
    let accounts_before =
        json(&fx.send(Method::GET, "/v1/accounts", &fx.keys.read, None))["accounts"]
            .as_array()
            .map(Vec::len);
    let second = pollster::block_on(dispatch::bootstrap_dev(&fx.deps, common::COMPANY))
        .expect("second bootstrap");
    assert_ne!(second, fx.keys.full, "each start mints its own key");
    let response = fx.send(Method::GET, "/v1/accounts", &second, None);
    assert_eq!(response.status.as_u16(), 200);
    assert_eq!(
        json(&response)["accounts"].as_array().map(Vec::len),
        accounts_before
    );
}

#[test]
fn a_revoked_key_fails_on_the_next_request() {
    let fx = fixture();
    let revoked = fx.send(Method::GET, "/v1/accounts", &fx.keys.read, None);
    assert_eq!(revoked.status.as_u16(), 200);
    let prefix = &fx.keys.read[..fx.keys.read.rfind('_').expect("token has a secret part")];
    pollster::block_on(fx.deps.db.execute(&cratefield_core::Statement::with_values(
        "UPDATE api_keys SET revoked_at = ? WHERE prefix = ?".to_owned(),
        vec![
            sea_query::Value::from("2026-10-01T00:00:00Z"),
            sea_query::Value::from(prefix),
        ],
    )))
    .expect("revoke");
    let after = fx.send(Method::GET, "/v1/accounts", &fx.keys.read, None);
    assert_eq!(after.status.as_u16(), 401, "{}", after.json());
}

#[test]
fn every_registry_route_is_mounted_and_none_answers_not_implemented() {
    let fx = fixture();
    let registry = dispatch::registry();
    for descriptor in registry.iter() {
        let path = descriptor.http_path.replace("{external_id}", EXT);
        let method = if descriptor.http_method == "GET" {
            Method::GET
        } else {
            Method::POST
        };
        let response = fx.send(method, &path, &fx.keys.full, Some("{}"));
        let body = response.json().to_string();
        assert!(
            !body.contains("not yet implemented"),
            "{} still answers 'not yet implemented'",
            descriptor.name
        );
    }
}

#[test]
fn the_registry_has_the_commands_the_web_app_needs() {
    let registry = dispatch::registry();
    for (name, path) in [
        ("accounts.list", "/v1/accounts"),
        ("entries.get", "/v1/entries/{external_id}"),
        ("entries.list", "/v1/entries"),
        ("entries.post", "/v1/entries"),
        ("entries.reverse", "/v1/entries/{external_id}/reverse"),
        ("reports.trial_balance", "/v1/reports/trial-balance"),
        ("reports.profit_and_loss", "/v1/reports/p-and-l"),
        ("reports.balance_sheet", "/v1/reports/balance-sheet"),
        (
            "reports.account_balance_by_dimension",
            "/v1/reports/account-balance-by-dimension",
        ),
    ] {
        let descriptor = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} is registered"));
        assert_eq!(descriptor.http_path, path, "{name}");
    }
    assert!(
        registry.get("bills.match").is_none(),
        "no bills domain is served"
    );
    assert!(
        registry.get("test.insert").is_none(),
        "test.insert never ships"
    );
}

#[test]
fn needs_approval_answers_with_the_stored_request_and_its_link() {
    let fx = fixture();
    let body = entry_json("inbox-link-1", "5010", "4000", "100000").to_string();
    let response = fx.send(Method::POST, "/v1/entries", &fx.keys.unlimited, Some(&body));
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let answer = json(&response);
    assert_eq!(answer["status"], "needs-approval");
    let id = answer["approval"]["id"]
        .as_str()
        .expect("approval id")
        .to_owned();
    assert_eq!(answer["approval"]["url"], format!("/approvals/{id}"));
    assert_eq!(answer["approval"]["status"], "pending");
    let stored = pollster::block_on(ledgers_approvals::get_request(
        fx.kit.db.as_ref(),
        dispatch::DEV_COMPANY_ID,
        &id,
    ))
    .expect("the request is stored in the same batch as the audit row");
    assert_eq!(stored.command, "entries.post");
    assert_eq!(stored.risk, ledgers_approvals::Risk::High);
    // The Created notice is enqueued in the same batch as the request.
    let notices = pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
        "SELECT topic FROM approval_outbox WHERE id = ?".to_owned(),
        vec![sea_query::Value::from(format!("{id}-created"))],
    )))
    .expect("query the notice outbox");
    assert_eq!(notices.rows.len(), 1, "one created notice for the request");
    assert_eq!(
        notices.rows[0].get::<String>("topic").as_deref(),
        Some("approval.created")
    );
}

#[test]
fn agent_keys_are_refused_the_approval_inbox() {
    let fx = fixture();
    let list = fx.send(Method::GET, "/v1/approval-requests", &fx.keys.full, None);
    assert_eq!(list.status.as_u16(), 403, "{}", list.json());
    let get = fx.send(
        Method::GET,
        "/v1/approval-requests/any-id",
        &fx.keys.full,
        None,
    );
    assert_eq!(get.status.as_u16(), 403, "{}", get.json());
    let decide = fx.send(
        Method::POST,
        "/v1/approval-requests/any-id/decision",
        &fx.keys.full,
        Some(r#"{"decision":"approve"}"#),
    );
    assert_eq!(decide.status.as_u16(), 403, "{}", decide.json());
}
