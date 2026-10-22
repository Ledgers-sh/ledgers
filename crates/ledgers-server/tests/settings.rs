//! The Settings commands over HTTP: the company profile, accounting years and
//! periods, invoice numbering, tax settings, approval rules and the audit
//! read. Person only (agent keys get 403, and the attempt is audited), every
//! change audited with its command, validation refused with 422, and the
//! audit read's query count does not grow with the page's rows.

mod common;

use common::http::{Auth, Reply, call};
use common::{Fixture, OWNER_SESSION, fixture_with_options, owner_session};
use cratefield_core::axum::http::Method;
use ledgers_server::dispatch::{Deps, ServerDispatch};
use ledgers_server::routes::{Dispatch, Presented, SessionPresented};
use ledgers_server::timing::{CountingDb, DbTiming};
use serde_json::{Value, json};
use std::sync::Arc;

/// The app origin the owner session may call from.
const APP: &str = common::http::APP_ORIGIN;

fn as_owner(fx: &Fixture, method: Method, path: &str, body: Option<Value>) -> Reply {
    call(fx, method, path, Auth::Session(OWNER_SESSION), true, body)
}

fn as_agent(fx: &Fixture, method: Method, path: &str, body: Option<Value>) -> Reply {
    call(fx, method, path, Auth::Bearer(&fx.keys.full), true, body)
}

fn ok(reply: &Reply) -> Value {
    assert_eq!(reply.status.as_u16(), 200, "expected 200: {}", reply.text);
    reply.json()
}

fn refused(reply: &Reply, status: u16) -> String {
    assert_eq!(
        reply.status.as_u16(),
        status,
        "expected {status}: {}",
        reply.text
    );
    reply.text.clone()
}

/// The owner's fresh fixture, with the Settings screens' commands reachable.
fn owned() -> Fixture {
    let fx = fixture_with_options(Vec::new(), None, vec![APP.to_owned()]);
    owner_session(&fx);
    fx
}

/// The latest fiscal year of the company (2026 in the fixture).
fn latest_year(fx: &Fixture) -> i64 {
    let list = ok(&as_owner(fx, Method::GET, "/v1/fiscal-years", None));
    list["years"]
        .as_array()
        .expect("years")
        .iter()
        .filter_map(|year| year["label"].as_i64())
        .max()
        .expect("the fixture has a fiscal year")
}

/// Posts one entry dated `date` as the full-tier key (no approval needed).
fn post_on(fx: &Fixture, external_id: &str, date: &str) -> Reply {
    let mut body = common::entry_json(external_id, "5010", "4000", "100");
    body["entry"]["document_date"] = json!(date);
    body["entry"]["posting_date"] = json!(date);
    call(
        fx,
        Method::POST,
        "/v1/entries",
        Auth::Bearer(&fx.keys.full),
        true,
        Some(body),
    )
}

/// Every audit event the owner can read, newest first, as the audit read returns it.
fn audit_events(fx: &Fixture, input: &str) -> Vec<Value> {
    let path = if input.is_empty() {
        "/v1/audit-events".to_owned()
    } else {
        format!("/v1/audit-events?{input}")
    };
    ok(&as_owner(fx, Method::GET, &path, None))["events"]
        .as_array()
        .expect("events")
        .clone()
}

#[test]
fn company_reads_the_books_company_and_saves_its_profile() {
    let fx = owned();
    let read = ok(&as_owner(&fx, Method::GET, "/v1/company", None));
    assert_eq!(read["base_currency"], "EUR");
    assert_eq!(read["base_currency_locked"], false, "no entries yet");

    let bad_vat = as_owner(
        &fx,
        Method::POST,
        "/v1/company",
        Some(json!({
            "name": "Halden Works GmbH",
            "profile": {"country": "DE", "vat_id": "FR123456789"}
        })),
    );
    let text = refused(&bad_vat, 422);
    assert!(
        text.contains("DE"),
        "the VAT refusal names the country: {text}"
    );

    let saved = ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/company",
        Some(json!({
            "name": "Halden Works GmbH",
            "profile": {
                "legal_form": "GmbH",
                "vat_id": " de 123.456.789 ",
                "registration_number": "HRB 12345",
                "address_line": "Hauptstraße 1",
                "postal_code": "10115",
                "city": "Berlin",
                "country": "de",
                "contact_email": "books@halden.example"
            }
        })),
    ));
    assert_eq!(saved["name"], "Halden Works GmbH");
    assert_eq!(saved["profile"]["vat_id"], "DE123456789", "normalized");
    assert_eq!(saved["profile"]["country"], "DE");
    let again = ok(&as_owner(&fx, Method::GET, "/v1/company", None));
    assert_eq!(again["profile"]["registration_number"], "HRB 12345");
}

#[test]
fn company_update_is_audited_in_the_same_batch() {
    let fx = owned();
    ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/company",
        Some(
            json!({"name": "Halden Works GmbH", "profile": {"contact_email": "books@halden.example"}}),
        ),
    ));
    let events = audit_events(&fx, "command=company.update");
    assert_eq!(
        events.len(),
        1,
        "one allowed event per saved change: {events:?}"
    );
    assert_eq!(events[0]["outcome"], "allowed");
    assert_eq!(events[0]["actor_kind"], "person");
}

#[test]
fn a_company_with_entries_locks_its_base_currency() {
    let fx = owned();
    assert_eq!(post_on(&fx, "lock-1", "2026-10-15").status.as_u16(), 200);
    let read = ok(&as_owner(&fx, Method::GET, "/v1/company", None));
    assert_eq!(read["base_currency_locked"], true);
}

#[test]
fn agents_get_403_on_every_settings_command_and_the_attempt_is_audited() {
    let fx = owned();
    let attempts = [
        (Method::GET, "/v1/company", None),
        (
            Method::POST,
            "/v1/company",
            Some(json!({"name": "x", "profile": {}})),
        ),
        (Method::GET, "/v1/fiscal-years", None),
        (
            Method::POST,
            "/v1/fiscal-years",
            Some(json!({"label": 2030})),
        ),
        (
            Method::POST,
            "/v1/periods/close",
            Some(json!({"period": "2026-02"})),
        ),
        (Method::GET, "/v1/numbering", None),
        (
            Method::POST,
            "/v1/numbering/next",
            Some(json!({"journal_id": "general", "fiscal_year": 2026, "next_number": 9})),
        ),
        (Method::GET, "/v1/tax-settings", None),
        (
            Method::POST,
            "/v1/tax-settings",
            Some(json!({"small_business": true, "vat_basis": "cash"})),
        ),
        (Method::GET, "/v1/approval-settings", None),
        (
            Method::POST,
            "/v1/approval-settings",
            Some(json!({"low_risk_limits": {}, "request_ttl_hours": 2})),
        ),
        (Method::GET, "/v1/audit-events", None),
    ];
    for (method, path, body) in attempts {
        let reply = as_agent(&fx, method.clone(), path, body);
        assert_eq!(
            reply.status.as_u16(),
            403,
            "agent {method} {path}: {}",
            reply.text
        );
    }
    assert_eq!(
        ok(&as_owner(&fx, Method::GET, "/v1/company", None))["profile"]["contact_email"],
        "",
        "the refused write changed nothing"
    );
}

#[test]
fn fiscal_years_open_the_next_year_and_close_the_one_before_it() {
    let fx = owned();
    let current = latest_year(&fx);
    refused(
        &as_owner(
            &fx,
            Method::POST,
            "/v1/fiscal-years",
            Some(json!({"label": 1800})),
        ),
        422,
    );
    refused(
        &as_owner(
            &fx,
            Method::POST,
            "/v1/fiscal-years",
            Some(json!({"label": current})),
        ),
        422,
    );

    let opened = ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/fiscal-years",
        Some(json!({"label": current + 1})),
    ));
    assert_eq!(opened["label"], current + 1);
    assert_eq!(opened["closed"], false);
    assert_eq!(opened["periods"].as_array().map(Vec::len), Some(12));

    let closed = ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/fiscal-years/close",
        Some(json!({"label": current})),
    ));
    assert_eq!(closed["year"]["closed"], true, "{closed}");
    let list = ok(&as_owner(&fx, Method::GET, "/v1/fiscal-years", None));
    let year = list["years"]
        .as_array()
        .unwrap()
        .iter()
        .find(|y| y["label"] == current)
        .unwrap();
    assert_eq!(year["closed"], true);

    let events = audit_events(&fx, "command=fiscal_year.close");
    assert!(
        !events.is_empty(),
        "the year-end close is audited by the domain"
    );
}

#[test]
fn a_locked_month_refuses_postings_until_a_reason_reopens_it() {
    let fx = owned();
    let current = latest_year(&fx);
    ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/fiscal-years",
        Some(json!({"label": current + 1})),
    ));
    let locked = ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/periods/close",
        Some(json!({"period": "2026-03"})),
    ));
    assert_eq!(locked["state"], "closed");

    let refused_post = post_on(&fx, "march-1", "2026-03-05");
    assert!(
        refused_post.status.as_u16() >= 400,
        "a closed month refuses postings: {}",
        refused_post.text
    );

    refused(
        &as_owner(
            &fx,
            Method::POST,
            "/v1/periods/reopen",
            Some(json!({"period": "2026-03", "reason": "  "})),
        ),
        422,
    );
    let reopened = ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/periods/reopen",
        Some(json!({"period": "2026-03", "reason": "Late supplier invoice"})),
    ));
    assert_eq!(reopened["state"], "open");
    assert_eq!(post_on(&fx, "march-2", "2026-03-05").status.as_u16(), 200);
    refused(
        &as_owner(
            &fx,
            Method::POST,
            "/v1/periods/close",
            Some(json!({"period": "2026-13"})),
        ),
        422,
    );
}

#[test]
fn numbering_moves_forward_only_until_a_number_is_issued() {
    let fx = owned();
    let list = ok(&as_owner(&fx, Method::GET, "/v1/numbering", None));
    let journal = &list["journals"][0];
    assert_eq!(journal["code"], "GN");
    let year = latest_year(&fx);
    let sequence = journal["sequences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["fiscal_year"] == year)
        .unwrap()
        .clone();
    assert_eq!(sequence["next_number"], 1);
    assert_eq!(sequence["editable"], true);

    let jid = journal["id"].as_str().unwrap().to_owned();
    let moved = ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/numbering/next",
        Some(json!({"journal_id": jid, "fiscal_year": year, "next_number": 50})),
    ));
    assert_eq!(moved["next_number"], 50);
    let listed = ok(&as_owner(&fx, Method::GET, "/v1/numbering", None));
    let after = listed["journals"][0]["sequences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["fiscal_year"] == year)
        .unwrap()
        .clone();
    assert_eq!(after["next_number"], 50);

    let backwards = as_owner(
        &fx,
        Method::POST,
        "/v1/numbering/next",
        Some(json!({"journal_id": jid, "fiscal_year": year, "next_number": 10})),
    );
    assert!(refused(&backwards, 422).contains("forward"));

    assert_eq!(post_on(&fx, "num-1", "2026-10-15").status.as_u16(), 200);
    let issued = as_owner(
        &fx,
        Method::POST,
        "/v1/numbering/next",
        Some(json!({"journal_id": jid, "fiscal_year": year, "next_number": 99})),
    );
    assert!(
        refused(&issued, 422).contains("gap"),
        "numbers already issued are fixed"
    );
    let unknown = as_owner(
        &fx,
        Method::POST,
        "/v1/numbering/next",
        Some(json!({"journal_id": "nope", "fiscal_year": year, "next_number": 99})),
    );
    refused(&unknown, 422);
}

#[test]
fn tax_settings_show_german_rates_and_save_the_basis() {
    let fx = owned();
    let before = ok(&as_owner(&fx, Method::GET, "/v1/tax-settings", None));
    assert_eq!(before["small_business"], false);
    assert_eq!(before["vat_basis"], "accrual");
    assert_eq!(before["rate_table"], false, "no country yet");

    ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/company",
        Some(json!({"name": "Halden Works GmbH", "profile": {"country": "DE"}})),
    ));
    let german = ok(&as_owner(&fx, Method::GET, "/v1/tax-settings", None));
    assert_eq!(german["rate_table"], true);
    let percents: Vec<&str> = german["rates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["percent"].as_str().unwrap())
        .collect();
    assert_eq!(percents, vec!["19", "7"]);

    refused(
        &as_owner(
            &fx,
            Method::POST,
            "/v1/tax-settings",
            Some(json!({"small_business": false, "vat_basis": "monthly"})),
        ),
        422,
    );
    let saved = ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/tax-settings",
        Some(json!({"small_business": true, "vat_basis": "cash"})),
    ));
    assert_eq!(saved["small_business"], true);
    assert_eq!(saved["vat_basis"], "cash");
    let events = audit_events(&fx, "command=tax.set_settings");
    assert_eq!(events.len(), 1, "one audited settings change");
}

#[test]
fn approval_rules_default_to_the_shipped_values_and_validate_changes() {
    let fx = owned();
    let defaults = ok(&as_owner(&fx, Method::GET, "/v1/approval-settings", None));
    assert_eq!(
        defaults["low_risk_limits"],
        json!({"EUR": "100.00", "GBP": "100.00", "USD": "100.00"})
    );
    assert_eq!(defaults["request_ttl_hours"], 24);
    assert_eq!(defaults["fresh_sign_in_minutes"], 5);

    let whole = ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/approval-settings",
        Some(json!({"low_risk_limits": {"EUR": "100"}, "request_ttl_hours": 24})),
    ));
    assert_eq!(
        whole["low_risk_limits"]["EUR"], "100.00",
        "a whole amount is stored in the currency's decimals"
    );
    refused(
        &as_owner(
            &fx,
            Method::POST,
            "/v1/approval-settings",
            Some(json!({"low_risk_limits": {"JPY": "10.00"}, "request_ttl_hours": 24})),
        ),
        422,
    );
    refused(
        &as_owner(
            &fx,
            Method::POST,
            "/v1/approval-settings",
            Some(json!({"low_risk_limits": {}, "request_ttl_hours": 0})),
        ),
        422,
    );

    let saved = ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/approval-settings",
        Some(
            json!({"low_risk_limits": {"EUR": "250.00", "JPY": "30000"}, "request_ttl_hours": 48}),
        ),
    ));
    assert_eq!(
        saved["low_risk_limits"],
        json!({"EUR": "250.00", "JPY": "30000"})
    );
    assert_eq!(saved["request_ttl_hours"], 48);
    // The whole-amount save above and this one: each is one audited change.
    assert_eq!(
        audit_events(&fx, "command=approval_settings.update").len(),
        2
    );
}

#[test]
fn a_saved_expiry_sets_how_long_a_new_request_waits() {
    let fx = owned();
    ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/approval-settings",
        Some(json!({"low_risk_limits": {"EUR": "100.00"}, "request_ttl_hours": 2})),
    ));
    // A key with a 1000.00 EUR per-entry ceiling asks for approval above it.
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
    let body = common::entry_json("ttl-1", "5010", "4000", "100001");
    let reply = call(
        &fx,
        Method::POST,
        "/v1/entries",
        Auth::Bearer(&key),
        true,
        Some(body),
    );
    let answer = ok(&reply);
    assert_eq!(answer["status"], "needs-approval");
    // The fixture clock reads 2026-10-22T10:00:00Z: two hours later is noon.
    assert_eq!(answer["approval"]["expires_at"], "2026-10-22T12:00:00Z");
}

#[test]
fn audit_read_filters_by_actor_command_outcome_and_date_and_pages() {
    let fx = owned();
    assert_eq!(post_on(&fx, "audit-1", "2026-10-15").status.as_u16(), 200);
    as_agent(
        &fx,
        Method::POST,
        "/v1/company",
        Some(json!({"name": "x", "profile": {}})),
    );
    ok(&as_owner(
        &fx,
        Method::POST,
        "/v1/company",
        Some(json!({"name": "Halden Works GmbH", "profile": {}})),
    ));

    let agents = audit_events(&fx, "actor_kind=agent");
    assert!(!agents.is_empty() && agents.iter().all(|e| e["actor_kind"] == "agent"));
    assert!(
        agents
            .iter()
            .any(|e| e["command"] == "entries.post" && e["outcome"] == "allowed")
    );
    let people = audit_events(&fx, "actor_kind=person&command=company.update");
    assert_eq!(people.len(), 1);
    let entries = audit_events(&fx, "command=entries.post");
    assert!(entries.iter().all(|e| e["command"] == "entries.post"));

    let day = audit_events(&fx, "from=2026-10-22&to=2026-10-22");
    assert!(!day.is_empty(), "the fixed clock is 2026-10-22");
    let other_day = audit_events(&fx, "from=2020-01-01&to=2020-01-02");
    assert!(other_day.is_empty());

    let page_one = ok(&as_owner(
        &fx,
        Method::GET,
        "/v1/audit-events?limit=2",
        None,
    ));
    assert_eq!(page_one["events"].as_array().unwrap().len(), 2);
    let cursor = page_one["next_before_seq"].as_i64().expect("more events");
    let page_two = ok(&as_owner(
        &fx,
        Method::GET,
        &format!("/v1/audit-events?limit=2&before_seq={cursor}"),
        None,
    ));
    let first_seq = page_one["events"][1]["seq"].as_i64().unwrap();
    assert!(
        page_two["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["seq"].as_i64().unwrap() < first_seq)
    );

    refused(
        &as_owner(&fx, Method::GET, "/v1/audit-events?from=2026-13-01", None),
        422,
    );
    refused(
        &as_owner(
            &fx,
            Method::GET,
            "/v1/audit-events?from=2026-10-05&to=2026-10-01",
            None,
        ),
        422,
    );
    refused(
        &as_owner(&fx, Method::GET, "/v1/audit-events?actor_kind=robot", None),
        422,
    );
}

/// The statements one call runs, after one warm-up call.
fn statements_for(fx: &Fixture, command: &str, input: Value) -> u64 {
    run(&probe(fx), command, input.clone());
    let measured = probe(fx);
    run(&measured, command, input);
    measured.timing.snapshot().statements
}

/// The fixture's ports with a statement counter on the database.
struct Probe {
    deps: Deps,
    timing: Arc<DbTiming>,
}

fn probe(fx: &Fixture) -> Probe {
    let timing = Arc::new(DbTiming::default());
    let mut deps = fx.deps.clone();
    deps.db = Arc::new(CountingDb::new(deps.db.clone(), timing.clone()));
    Probe { deps, timing }
}

/// Runs one read command as the owner session, the way the HTTP adapter does.
fn run(probe: &Probe, command: &str, input: Value) -> Value {
    let presented = Presented::Session(SessionPresented {
        id: OWNER_SESSION.to_owned(),
        origin: Some(APP.to_owned()),
        safe: true,
    });
    pollster::block_on(
        ServerDispatch::new(common::COMPANY)
            .with_app_origins(vec![APP.to_owned()])
            .dispatch(&probe.deps, command, input, "req", Some(presented)),
    )
    .unwrap_or_else(|problem| panic!("{command} refused: {problem:?}"))
}

#[test]
fn list_reads_cost_the_same_statements_however_many_rows_they_hold() {
    let fx = owned();
    let small = statements_for(&fx, "audit.list", json!({"limit": 50}));
    for n in 0..60 {
        post_on(&fx, &format!("bulk-{n}"), "2026-10-15");
    }
    let large = statements_for(&fx, "audit.list", json!({"limit": 50}));
    assert_eq!(
        small, large,
        "audit.list runs the same statements for a few rows or sixty"
    );
    let numbering = statements_for(&fx, "numbering.list", json!({}));
    assert!(
        numbering <= 6,
        "numbering.list is a fixed set of queries, got {numbering}"
    );
}
