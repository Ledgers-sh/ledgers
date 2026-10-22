//! The demo company (issue #51) seeded through the real server: every entry
//! goes through the executor, the reports read back the golden totals, and a
//! second run posts nothing.

mod common;

use common::{COMPANY, Method, fixture, json};
use ledgers_demo::halden::{self, GOLDEN};
use ledgers_demo::{DemoEntry, DemoLine, EntryKind, Poster, Side};
use ledgers_server::demo::{self, SeedReport};
use serde_json::Value;
use std::collections::BTreeMap;

const YEAR: i32 = 2_026;

fn seed_once(f: &common::Fixture) -> SeedReport {
    let book = halden::book(YEAR);
    pollster::block_on(demo::seed(&f.deps, COMPANY, &f.keys.full, &book))
        .expect("the demo company seeds")
}

fn minor(value: &Value) -> i64 {
    value["minor"]
        .as_str()
        .expect("amounts are strings of minor units")
        .parse()
        .expect("an integer amount")
}

/// The signed balance (debit positive) the accounts listing reports.
fn account_balances(f: &common::Fixture) -> BTreeMap<String, i64> {
    let response = f.send(Method::GET, "/v1/accounts", &f.keys.full, None);
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    json(&response)["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .filter_map(|account| {
            let balance = account["balances"].as_array()?.first()?;
            Some((account["number"].as_str()?.to_owned(), minor(balance)))
        })
        .filter(|(_, balance)| *balance != 0)
        .collect()
}

#[test]
fn seeding_posts_the_whole_book_and_the_chart_matches_the_golden_balances() {
    let f = fixture();
    let report = seed_once(&f);
    assert_eq!(report.posted, 153, "{report:?}");
    assert_eq!(report.reversed, 2, "{report:?}");
    assert_eq!(report.skipped, 0, "{report:?}");
    assert_eq!(report.dimensions_created, 4, "{report:?}");

    let actual = account_balances(&f);
    let expected: BTreeMap<String, i64> = GOLDEN
        .year_end_balances
        .iter()
        .map(|(account, balance)| ((*account).to_owned(), *balance))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn the_monthly_profit_and_loss_adds_up_to_the_golden_year() {
    let f = fixture();
    seed_once(&f);
    let (mut revenue, mut expenses) = (0_i64, 0_i64);
    for month in 1..=12 {
        let response = f.send(
            Method::GET,
            &format!("/v1/reports/p-and-l?period={YEAR}-{month:02}"),
            &f.keys.full,
            None,
        );
        assert_eq!(response.status.as_u16(), 200, "{}", response.json());
        let body = json(&response);
        revenue += minor(&body["revenue"]);
        expenses += minor(&body["expenses"]);
    }
    assert_eq!(revenue, GOLDEN.fy_revenue);
    assert_eq!(expenses, GOLDEN.fy_expenses);
    assert_eq!(revenue - expenses, GOLDEN.fy_net_income);
}

#[test]
fn the_balance_sheet_at_year_end_balances_and_matches_the_golden_totals() {
    let f = fixture();
    seed_once(&f);
    let response = f.send(
        Method::GET,
        &format!("/v1/reports/balance-sheet?as_of={YEAR}-12-31"),
        &f.keys.full,
        None,
    );
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let body = json(&response);
    let (assets, liabilities, equity) = (
        minor(&body["assets"]),
        minor(&body["liabilities"]),
        minor(&body["equity"]),
    );
    let golden_assets: i64 = GOLDEN
        .year_end_balances
        .iter()
        .filter(|(account, _)| account.starts_with('1'))
        .map(|(_, balance)| balance)
        .sum();
    assert_eq!(assets, golden_assets);
    // The report's equity is the equity accounts only; the year's result sits
    // in the income and expense accounts. Assets equal liabilities, equity and
    // the year's net income together.
    assert_eq!(
        assets,
        liabilities + equity + GOLDEN.fy_net_income,
        "assets = liabilities + equity + current-year result"
    );
}

#[test]
fn a_dimension_split_adds_up_to_the_account_total() {
    let f = fixture();
    seed_once(&f);
    let mut cost_centre_sales = 0_i64;
    for month in 1..=12 {
        let response = f.send(
            Method::GET,
            &format!(
                "/v1/reports/account-balance-by-dimension?account=4010&dimension=cost_centre&period={YEAR}-{month:02}"
            ),
            &f.keys.full,
            None,
        );
        assert_eq!(response.status.as_u16(), 200, "{}", response.json());
        for row in json(&response)["rows"].as_array().expect("rows") {
            if row["dimension_value"] == "CC100" {
                cost_centre_sales += minor(&row["balance"]);
            }
        }
    }
    // Credit balance on sales: the fixture books all frames sales to CC100.
    assert_eq!(cost_centre_sales, -28_355_000);
}

#[test]
fn seeding_twice_posts_nothing_new() {
    let f = fixture();
    let first = seed_once(&f);
    let before = account_balances(&f);
    let second = seed_once(&f);
    assert_eq!(second.posted, 0, "{second:?}");
    assert_eq!(second.reversed, 0, "{second:?}");
    assert_eq!(second.dimensions_created, 0, "{second:?}");
    assert_eq!(second.skipped, first.posted + first.reversed, "{second:?}");
    assert_eq!(account_balances(&f), before);
}

#[test]
fn the_seeded_book_has_entries_from_an_agent_and_from_a_person() {
    let f = fixture();
    seed_once(&f);
    let march = f.send(
        Method::GET,
        &format!("/v1/entries?period={YEAR}-03&limit=200"),
        &f.keys.full,
        None,
    );
    assert_eq!(march.status.as_u16(), 200, "{}", march.json());
    let entries = json(&march)["entries"].as_array().expect("entries").clone();
    let posted_by: Vec<&str> = entries
        .iter()
        .filter_map(|entry| entry["posted_by"].as_str())
        .collect();
    assert!(posted_by.contains(&"agent"), "{posted_by:?}");
    assert!(posted_by.contains(&"person"), "{posted_by:?}");
}

#[test]
fn a_dev_company_with_no_entries_loads_the_demo_once() {
    let f = fixture();
    let first =
        pollster::block_on(demo::seed_if_empty(&f.deps, COMPANY, &f.keys.full)).expect("loads");
    assert_eq!(first.map(|r| r.posted), Some(153));
    let second =
        pollster::block_on(demo::seed_if_empty(&f.deps, COMPANY, &f.keys.full)).expect("checks");
    assert_eq!(second, None, "a company with entries is left alone");
}

#[test]
fn a_draft_key_cannot_seed_the_demo() {
    let f = fixture();
    let book = halden::book(YEAR);
    let result = pollster::block_on(demo::seed(&f.deps, COMPANY, &f.keys.draft, &book));
    assert!(
        result.is_err(),
        "a draft key must not post the agent entries"
    );
}

#[test]
fn an_agent_cannot_reverse_a_person_posted_entry() {
    // An owner entry that touches no protected account, so the only thing that
    // can stop the agent is the origin rule: a person's entry is `human`, and
    // an agent may not reverse it (docs/PLAN.md section 3, rule 7; the ledger
    // guard `reversal-no-human-by-agent`).
    let f = fixture();
    let entry = DemoEntry {
        external_id: "owner-expense-1".to_owned(),
        poster: Poster::Person,
        posting_date: format!("{YEAR}-03-10"),
        description: "Owner expense, unprotected accounts only".to_owned(),
        kind: EntryKind::Post {
            lines: vec![
                DemoLine {
                    account: "5060",
                    side: Side::Debit,
                    minor: 10_000,
                    tax_code: None,
                    dimensions: Vec::new(),
                    memo: None,
                },
                DemoLine {
                    account: "5030",
                    side: Side::Credit,
                    minor: 10_000,
                    tax_code: None,
                    dimensions: Vec::new(),
                    memo: None,
                },
            ],
        },
    };
    pollster::block_on(demo::post_as_owner(&f.deps, COMPANY, "EUR", &entry))
        .expect("the owner posts");
    let posted = f.send(
        Method::GET,
        "/v1/entries/owner-expense-1",
        &f.keys.full,
        None,
    );
    assert_eq!(json(&posted)["posted_by"], "person");

    let before = account_balances(&f);
    let response = f.send(
        Method::POST,
        "/v1/entries/owner-expense-1/reverse",
        &f.keys.full,
        Some("{}"),
    );
    let body = response.json();
    let reversed = response.status.as_u16() == 200 && body["status"] != "needs-approval";
    assert!(!reversed, "an agent reversed a person entry: {body}");
    assert_eq!(account_balances(&f), before, "nothing changed");
}

#[test]
fn an_owner_entry_that_was_reversed_is_recorded_as_person() {
    // The owner's duplicate invoice: posted as a person, then reversed.
    let f = fixture();
    seed_once(&f);
    let entry = f.send(
        Method::GET,
        "/v1/entries/halden-08-invoice-nordstrand-duplicate",
        &f.keys.full,
        None,
    );
    assert_eq!(entry.status.as_u16(), 200, "{}", entry.json());
    assert_eq!(json(&entry)["posted_by"], "person");
    assert!(!json(&entry)["reversed_by"].is_null(), "{}", entry.json());
}

#[test]
fn the_december_trial_balance_matches_the_fixture_activity() {
    let f = fixture();
    seed_once(&f);
    let book = halden::book(YEAR);
    let expected = book.activity_in_month(&format!("{YEAR}-12"));
    let response = f.send(
        Method::GET,
        &format!("/v1/reports/trial-balance?period={YEAR}-12"),
        &f.keys.full,
        None,
    );
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let body = json(&response);
    let mut actual: BTreeMap<String, i64> = BTreeMap::new();
    let (mut debits, mut credits) = (0_i64, 0_i64);
    for row in body["rows"].as_array().expect("rows") {
        let balance = minor(&row["balance"]);
        debits += minor(&row["debit"]);
        credits += minor(&row["credit"]);
        if balance != 0 {
            actual.insert(
                row["account_code"].as_str().expect("account").to_owned(),
                balance,
            );
        }
    }
    assert_eq!(debits, credits, "the trial balance balances");
    let expected: BTreeMap<String, i64> = expected
        .into_iter()
        .map(|(account, balance)| (account.to_owned(), balance))
        .collect();
    assert_eq!(actual, expected);
}
