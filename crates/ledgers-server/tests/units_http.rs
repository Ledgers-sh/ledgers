//! Business units over HTTP: the person-only unit writes (an agent key is
//! refused and audited), the recharge endpoint, the unit scope of the
//! reports, the side-by-side summary, and the unit pin of an agent key
//! (a missing unit takes the pin; another unit is refused and audited).

mod common;

use common::http::{APP_ORIGIN, Auth, call};
use common::{COMPANY, Fixture, OWNER_SESSION, fixture_with_options, owner_session};
use cratefield_core::Statement;
use cratefield_core::axum::http::{Method, StatusCode};
use sea_query::Value as SeaValue;
use serde_json::{Value, json as j};

fn units_fixture() -> Fixture {
    let fx = fixture_with_options(Vec::new(), None, vec![APP_ORIGIN.to_owned()]);
    owner_session(&fx);
    fx
}

/// Creates a unit as the owner, expecting success.
fn create_unit(fx: &Fixture, code: &str, name: &str) {
    let reply = call(
        fx,
        Method::POST,
        "/v1/units",
        Auth::Session(OWNER_SESSION),
        true,
        Some(j!({ "code": code, "name": name })),
    );
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
}

/// Posts a shared cost accrued (credit 2030, not a protected account, so an
/// agent key may post it without approval), the cost line on `unit`.
fn post_cost(
    fx: &Fixture,
    key: &str,
    external_id: &str,
    unit: Option<&str>,
    minor: &str,
) -> common::http::Reply {
    let dims = match unit {
        Some(code) => j!({ "unit": code }),
        None => j!({}),
    };
    call(
        fx,
        Method::POST,
        "/v1/entries",
        Auth::Bearer(key),
        false,
        Some(j!({
            "entry": {
                "journal_id": "",
                "document_date": "2026-10-15",
                "posting_date": "2026-10-15",
                "tax_point_date": null,
                "base_currency": "EUR",
                "description": "Shared service",
                "source_documents": [],
                "external_id": external_id,
                "source_app": "fixture",
                "reverses": null,
                "lines": [
                    {"account_number": "5010", "side": "debit", "base": {"minor": minor, "currency": "EUR"},
                     "fx": null, "tax_code": null, "dimensions": dims, "memo": null},
                    {"account_number": "2030", "side": "credit", "base": {"minor": minor, "currency": "EUR"},
                     "fx": null, "tax_code": null, "dimensions": {}, "memo": null}
                ]
            }
        })),
    )
}

fn transfer_body(from: &str, to: &str, minor: &str) -> Value {
    j!({
        "from_unit": from,
        "to_unit": to,
        "amount": {"minor": minor, "currency": "EUR"},
        "document_date": "2026-10-16",
        "posting_date": "2026-10-16",
        "description": "Shared-cost recharge",
        "recovery_account": "4010",
        "cost_account": "5010"
    })
}

fn audit_denied(fx: &Fixture, command: &str) -> i64 {
    pollster::block_on(async {
        let rows = fx
            .deps
            .db
            .query(&Statement::with_values(
                "SELECT COUNT(*) AS n FROM audit_event WHERE company_id = ? AND payload LIKE ? AND payload LIKE '%denied%'"
                    .to_owned(),
                vec![SeaValue::from(COMPANY), SeaValue::from(format!("%{command}%"))],
            ))
            .await
            .expect("count");
        rows.rows[0].get::<i64>("n").expect("n")
    })
}

#[test]
fn unit_writes_are_person_only_and_an_agent_is_refused_and_audited() {
    let fx = units_fixture();
    create_unit(&fx, "fac", "Factory");

    let listed = call(
        &fx,
        Method::GET,
        "/v1/units",
        Auth::Session(OWNER_SESSION),
        false,
        None,
    );
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    let body = listed.json();
    let codes: Vec<&str> = body["units"]
        .as_array()
        .expect("units")
        .iter()
        .map(|unit| unit["code"].as_str().expect("code"))
        .collect();
    assert_eq!(codes, ["FAC"], "the unit is listed, upper-cased");

    let agent = call(
        &fx,
        Method::POST,
        "/v1/units",
        Auth::Bearer(&fx.keys.full),
        false,
        Some(j!({ "code": "LED", "name": "Ledgers" })),
    );
    assert_eq!(agent.status, StatusCode::FORBIDDEN, "{}", agent.text);
    assert!(
        audit_denied(&fx, "units.create") >= 1,
        "the refusal is audited as denied"
    );

    // Rename and archive are person-only too.
    let renamed = call(
        &fx,
        Method::POST,
        "/v1/units/FAC/rename",
        Auth::Session(OWNER_SESSION),
        true,
        Some(j!({ "name": "Factory GmbH", "colour": "#e9b44c" })),
    );
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text);
    assert_eq!(renamed.json()["unit"]["name"], "Factory GmbH");
    let archive_by_agent = call(
        &fx,
        Method::POST,
        "/v1/units/FAC/archive",
        Auth::Bearer(&fx.keys.full),
        false,
        Some(j!({})),
    );
    assert_eq!(
        archive_by_agent.status,
        StatusCode::FORBIDDEN,
        "{}",
        archive_by_agent.text
    );
}

#[test]
fn a_recharge_is_eliminated_from_the_consolidated_reports_and_visible_per_unit() {
    let fx = units_fixture();
    create_unit(&fx, "FAC", "Factory");
    create_unit(&fx, "LED", "Ledgers");
    let posted = post_cost(&fx, &fx.keys.full, "cost-1", Some("FAC"), "100000");
    assert_eq!(posted.status, StatusCode::OK, "{}", posted.text);
    assert!(
        posted.json().get("entry").is_some(),
        "posted, not held: {}",
        posted.text
    );

    let recharge = call(
        &fx,
        Method::POST,
        "/v1/transfers",
        Auth::Session(OWNER_SESSION),
        true,
        Some(transfer_body("FAC", "LED", "40000")),
    );
    assert_eq!(recharge.status, StatusCode::OK, "{}", recharge.text);
    assert_eq!(
        recharge.json()["entry"]["journal_id"],
        "general",
        "{}",
        recharge.text
    );

    let report = |query: &str| {
        let reply = call(
            &fx,
            Method::GET,
            &format!("/v1/reports/p-and-l?period=2026-10{query}"),
            Auth::Session(OWNER_SESSION),
            false,
            None,
        );
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
        reply.json()
    };
    let company = report("");
    assert_eq!(
        company["expenses"]["minor"], "100000",
        "the cost counted once: {company}"
    );
    assert_eq!(
        company["revenue"]["minor"], "0",
        "the recovery is eliminated: {company}"
    );
    let all = report("&unit=all");
    assert_eq!(all["expenses"]["minor"], "100000");
    let led = report("&unit=LED");
    assert_eq!(
        led["expenses"]["minor"], "40000",
        "ledgers carries its recharge: {led}"
    );
    let fac = report("&unit=FAC");
    assert_eq!(fac["revenue"]["minor"], "40000");
    assert_eq!(fac["expenses"]["minor"], "100000");

    let unknown = call(
        &fx,
        Method::GET,
        "/v1/reports/p-and-l?period=2026-10&unit=NOPE",
        Auth::Session(OWNER_SESSION),
        false,
        None,
    );
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);

    let summary = call(
        &fx,
        Method::GET,
        "/v1/reports/units-summary?period=2026-10",
        Auth::Session(OWNER_SESSION),
        false,
        None,
    );
    assert_eq!(summary.status, StatusCode::OK, "{}", summary.text);
    let body = summary.json();
    let rows = body["units"].as_array().expect("unit rows");
    assert_eq!(rows.len(), 2, "one row per unit: {body}");
    assert_eq!(body["eliminations"]["revenue"]["minor"], "-40000", "{body}");
    assert_eq!(
        body["eliminations"]["expenses"]["minor"], "-40000",
        "{body}"
    );
}

#[test]
fn a_pinned_agent_key_takes_its_unit_and_is_refused_for_another() {
    let fx = units_fixture();
    create_unit(&fx, "FAC", "Factory");
    create_unit(&fx, "LED", "Ledgers");
    let created = call(
        &fx,
        Method::POST,
        "/v1/keys",
        Auth::Session(OWNER_SESSION),
        true,
        Some(j!({
            "name": "ledgers bot",
            "tier": "post-within-limits",
            "limits": { "EUR": { "per_entry": "1000.00", "per_day": "5000.00" } },
            "unit": "LED"
        })),
    );
    assert_eq!(created.status, StatusCode::OK, "{}", created.text);
    let token = created.json()["token"].as_str().expect("token").to_owned();
    assert_eq!(created.json()["key"]["unit"], "LED");

    // No unit on the line: the pinned unit is filled in.
    let filled = post_cost(&fx, &token, "pin-1", None, "1000");
    assert_eq!(filled.status, StatusCode::OK, "{}", filled.text);
    assert!(
        filled.json().get("entry").is_some(),
        "posted, not held: {}",
        filled.text
    );
    let carried = pollster::block_on(fx.deps.db.query(&Statement::with_values(
        "SELECT COUNT(*) AS n FROM journal_line_dimension WHERE company_id = ? AND kind = 'unit' AND value = 'LED'"
            .to_owned(),
        vec![SeaValue::from(COMPANY)],
    )))
    .expect("count the pinned lines");
    assert!(
        carried.rows[0].get::<i64>("n").unwrap_or(0) >= 1,
        "the posted line carries the pinned unit"
    );

    // Another unit's line is refused and audited.
    let other = post_cost(&fx, &token, "pin-2", Some("FAC"), "1000");
    assert_eq!(other.status, StatusCode::FORBIDDEN, "{}", other.text);
    assert!(
        audit_denied(&fx, "entries.post") >= 1,
        "the refusal is audited"
    );

    // A recharge the pinned key may not make (the paying unit is not its own).
    let recharge = call(
        &fx,
        Method::POST,
        "/v1/transfers",
        Auth::Bearer(&token),
        false,
        Some(transfer_body("FAC", "LED", "100")),
    );
    assert_eq!(recharge.status, StatusCode::FORBIDDEN, "{}", recharge.text);
}

#[test]
fn a_pin_to_an_unknown_unit_is_refused_when_the_key_is_created() {
    let fx = units_fixture();
    let created = call(
        &fx,
        Method::POST,
        "/v1/keys",
        Auth::Session(OWNER_SESSION),
        true,
        Some(j!({
            "name": "ghost",
            "tier": "draft",
            "limits": {},
            "unit": "NOPE"
        })),
    );
    assert_eq!(created.status, StatusCode::NOT_FOUND, "{}", created.text);
}

#[test]
fn the_unit_registry_gives_the_expected_problem_statuses() {
    let fx = units_fixture();
    create_unit(&fx, "FAC", "Factory");
    let duplicate = call(
        &fx,
        Method::POST,
        "/v1/units",
        Auth::Session(OWNER_SESSION),
        true,
        Some(j!({ "code": "FAC", "name": "Again" })),
    );
    assert_eq!(duplicate.status, StatusCode::CONFLICT, "{}", duplicate.text);
    let bad_code = call(
        &fx,
        Method::POST,
        "/v1/units",
        Auth::Session(OWNER_SESSION),
        true,
        Some(j!({ "code": "A", "name": "Short" })),
    );
    assert_eq!(
        bad_code.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        bad_code.text
    );
}
