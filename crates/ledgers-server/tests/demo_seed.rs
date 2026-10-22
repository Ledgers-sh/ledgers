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

/// The equity accounts of the fixture at year end, credit-positive: share
/// capital 2,500,000 (3010) and retained earnings 1,200,000 (3020). The bank
/// loan (2510, 15,000,000) is a liability in the generic chart.
const GOLDEN_EQUITY_ACCOUNTS: i64 = 3_700_000;

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
    assert_eq!(minor(&body["equity_accounts"]), GOLDEN_EQUITY_ACCOUNTS);
    assert_eq!(
        minor(&body["current_year_result"]),
        GOLDEN.fy_net_income,
        "the year's result is income minus expenses"
    );
    // Equity as presented includes the year's result, so the sheet balances on
    // its own: assets = liabilities + equity, and the check reads zero.
    assert_eq!(equity, GOLDEN_EQUITY_ACCOUNTS + GOLDEN.fy_net_income);
    assert_eq!(assets, liabilities + equity);
    assert_eq!(body["balanced"], true, "{body}");
    assert_eq!(minor(&body["difference"]), 0, "{body}");
}

#[test]
fn the_balance_sheet_mid_year_balances_against_the_fixture() {
    let f = fixture();
    seed_once(&f);
    let through = format!("{YEAR}-06-30");
    let book = halden::book(YEAR);
    let balances = book.balances_through(&through);
    let assets: i64 = balances
        .iter()
        .filter(|(account, _)| account.starts_with('1'))
        .map(|(_, balance)| balance)
        .sum();
    // Income and expense activity from 1 January to 30 June, credit-positive.
    let half_year_result: i64 = -balances
        .iter()
        .filter(|(account, _)| account.starts_with('4') || account.starts_with('5'))
        .map(|(_, balance)| balance)
        .sum::<i64>();
    assert_ne!(half_year_result, 0, "the fixture has activity by June");

    let response = f.send(
        Method::GET,
        &format!("/v1/reports/balance-sheet?as_of={through}"),
        &f.keys.full,
        None,
    );
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    let body = json(&response);
    assert_eq!(minor(&body["assets"]), assets);
    assert_eq!(minor(&body["current_year_result"]), half_year_result);
    assert_eq!(body["balanced"], true, "{body}");
    assert_eq!(minor(&body["difference"]), 0, "{body}");
    assert_eq!(
        minor(&body["equity"]),
        minor(&body["equity_accounts"]) + half_year_result
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

// ------------------------------------------------ the deployed seed route

use ledgers_server::demo::{SeedGate, seed_demo_gate};

#[test]
fn an_unset_seed_secret_is_not_found() {
    assert_eq!(
        seed_demo_gate(None, "anything", Some("1")),
        SeedGate::NotFound
    );
    assert_eq!(seed_demo_gate(Some(""), "", Some("1")), SeedGate::NotFound);
    assert_eq!(SeedGate::NotFound.status(), Some(404));
}

#[test]
fn a_wrong_or_missing_token_is_unauthorized() {
    assert_eq!(
        seed_demo_gate(Some("s3cret"), "wrong", Some("1")),
        SeedGate::Unauthorized
    );
    assert_eq!(
        seed_demo_gate(Some("s3cret"), "", Some("1")),
        SeedGate::Unauthorized
    );
    assert_eq!(SeedGate::Unauthorized.status(), Some(401));
}

#[test]
fn a_missing_allow_flag_is_forbidden() {
    for allow in [None, Some("0"), Some("true"), Some("")] {
        assert_eq!(
            seed_demo_gate(Some("s3cret"), "s3cret", allow),
            SeedGate::Forbidden,
            "{allow:?}"
        );
    }
    assert_eq!(SeedGate::Forbidden.status(), Some(403));
}

#[test]
fn the_right_token_and_the_flag_open_the_gate() {
    assert_eq!(
        seed_demo_gate(Some("s3cret"), "s3cret", Some("1")),
        SeedGate::Open
    );
    assert_eq!(SeedGate::Open.status(), None);
}

#[test]
fn the_internal_agent_loads_the_book_and_a_second_run_posts_nothing() {
    let f = fixture();
    let book = halden::book(YEAR);
    let first = pollster::block_on(demo::seed_internal(&f.deps, COMPANY, &book))
        .expect("the internal agent seeds the book");
    assert_eq!(first.posted, 153, "{first:?}");
    assert_eq!(first.reversed, 2, "{first:?}");
    assert_eq!(first.dimensions_created, 4, "{first:?}");

    let before = account_balances(&f);
    let second = pollster::block_on(demo::seed_internal(&f.deps, COMPANY, &book))
        .expect("a second run checks");
    assert_eq!(second.posted, 0, "{second:?}");
    assert_eq!(second.reversed, 0, "{second:?}");
    assert_eq!(second.dimensions_created, 0, "{second:?}");
    assert_eq!(second.skipped, first.posted + first.reversed, "{second:?}");
    assert_eq!(account_balances(&f), before, "nothing new was posted");

    // The trial balance is the fixture's golden year end.
    let expected: BTreeMap<String, i64> = GOLDEN
        .year_end_balances
        .iter()
        .map(|(account, balance)| ((*account).to_owned(), *balance))
        .collect();
    assert_eq!(account_balances(&f), expected);
}

#[test]
fn the_internal_agent_posts_agent_entries_and_person_entries() {
    let f = fixture();
    let book = halden::book(YEAR);
    pollster::block_on(demo::seed_internal(&f.deps, COMPANY, &book)).expect("seeds");
    let march = f.send(
        Method::GET,
        &format!("/v1/entries?period={YEAR}-03&limit=200"),
        &f.keys.full,
        None,
    );
    let entries = json(&march)["entries"].as_array().expect("entries").clone();
    let posted_by: Vec<&str> = entries
        .iter()
        .filter_map(|entry| entry["posted_by"].as_str())
        .collect();
    assert!(posted_by.contains(&"agent"), "{posted_by:?}");
    assert!(posted_by.contains(&"person"), "{posted_by:?}");
}

#[test]
fn check_passes_after_seeding_the_book() {
    let f = fixture();
    let book = halden::book(YEAR);
    pollster::block_on(demo::seed_internal(&f.deps, COMPANY, &book)).expect("seeds");
    let report = pollster::block_on(demo::check(&f.deps, COMPANY, &book)).expect("check passes");
    // The fixture has 153 post entries and 2 reversal entries (155 total)
    assert_eq!(report.fixture_entries_found, 153, "{report:?}");
    assert_eq!(report.fixture_reversals_found, 2, "{report:?}");
    assert_eq!(report.entries_verified, 153, "{report:?}");
    assert_eq!(report.reversals_verified, 2, "{report:?}");
}

#[test]
fn check_detects_a_broken_entry_balance() {
    let f = fixture();
    let book = halden::book(YEAR);
    pollster::block_on(demo::seed_internal(&f.deps, COMPANY, &book)).expect("seeds");

    // Drop the append-only triggers to allow tampering with journal_line
    let db = f.deps.db.as_ref();
    pollster::block_on(db.execute(&cratefield_core::Statement::new(
        "DROP TRIGGER journal_line_no_update",
    )))
    .expect("drop update trigger");
    pollster::block_on(db.execute(&cratefield_core::Statement::new(
        "DROP TRIGGER journal_line_no_delete",
    )))
    .expect("drop delete trigger");

    // Corrupt the first entry's first line: change its base_minor so it no longer balances
    pollster::block_on(
        db.execute(&cratefield_core::Statement::with_values(
            "UPDATE journal_line SET base_minor = 999999 WHERE entry_id = (SELECT id FROM journal_entry WHERE company_id = ? LIMIT 1)"
                .to_owned(),
            vec![sea_query::Value::from(COMPANY)],
        )),
    )
    .expect("corrupt entry");

    // check should now fail with a message about entry balance
    let result = pollster::block_on(demo::check(&f.deps, COMPANY, &book));
    assert!(result.is_err(), "check should detect the imbalance");
    let err = result.unwrap_err();
    assert!(
        err.contains("entry does not balance"),
        "error message should name the broken invariant: {err}"
    );
}

/// Trend months from `reports.trends`, by period, as the response carries them.
fn trend_months(f: &common::Fixture, from: &str, to: &str, dimension: Option<&str>) -> Value {
    let mut path = format!("/v1/reports/trends?from={from}&to={to}");
    if let Some(kind) = dimension {
        path = format!("{path}&dimension={kind}");
    }
    let response = f.send(Method::GET, &path, &f.keys.full, None);
    assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    json(&response)
}

#[test]
fn the_trend_months_add_up_to_the_golden_year_and_the_year_end_balances() {
    let f = fixture();
    seed_once(&f);
    let body = trend_months(&f, &format!("{YEAR}-01"), &format!("{YEAR}-12"), None);
    let months = body["months"].as_array().expect("months");
    assert_eq!(months.len(), 12);
    assert_eq!(body["currency"], "EUR");

    let (mut revenue, mut expenses) = (0_i64, 0_i64);
    for month in months {
        revenue += minor(&month["revenue"]);
        expenses += minor(&month["expenses"]);
        assert_eq!(
            minor(&month["net_result"]),
            minor(&month["revenue"]) - minor(&month["expenses"]),
            "{month}"
        );
    }
    assert_eq!(revenue, GOLDEN.fy_revenue);
    assert_eq!(expenses, GOLDEN.fy_expenses);

    let december = &months[11];
    let golden_cash: i64 = GOLDEN
        .year_end_balances
        .iter()
        .filter(|(account, _)| *account == "1010" || *account == "1020")
        .map(|(_, balance)| balance)
        .sum();
    let golden_payables: i64 = -GOLDEN
        .year_end_balances
        .iter()
        .filter(|(account, _)| *account == "2010")
        .map(|(_, balance)| balance)
        .sum::<i64>();
    assert_eq!(minor(&december["cash"]), golden_cash);
    assert_eq!(minor(&december["payables"]), golden_payables);
    let golden_receivables: i64 = GOLDEN
        .year_end_balances
        .iter()
        .filter(|(account, _)| *account == "1030")
        .map(|(_, balance)| balance)
        .sum();
    assert_eq!(minor(&december["receivables"]), golden_receivables);
}

#[test]
fn the_trend_months_match_the_monthly_profit_and_loss_and_the_book_at_month_end() {
    let f = fixture();
    seed_once(&f);
    let body = trend_months(&f, &format!("{YEAR}-01"), &format!("{YEAR}-12"), None);
    let book = halden::book(YEAR);
    for (index, month) in body["months"]
        .as_array()
        .expect("months")
        .iter()
        .enumerate()
    {
        let period = format!("{YEAR}-{:02}", index + 1);
        let pnl = f.send(
            Method::GET,
            &format!("/v1/reports/p-and-l?period={period}"),
            &f.keys.full,
            None,
        );
        let pnl = json(&pnl);
        assert_eq!(month["period"], period.as_str());
        assert_eq!(month["revenue"], pnl["revenue"], "{period}");
        assert_eq!(month["expenses"], pnl["expenses"], "{period}");

        let balances = book.balances_through(&periodend(&period));
        let cash = balances.get("1010").unwrap_or(&0) + balances.get("1020").unwrap_or(&0);
        let receivables = *balances.get("1030").unwrap_or(&0);
        let payables = -balances.get("2010").unwrap_or(&0);
        assert_eq!(minor(&month["cash"]), cash, "cash at {period}");
        assert_eq!(
            minor(&month["receivables"]),
            receivables,
            "receivables at {period}"
        );
        assert_eq!(minor(&month["payables"]), payables, "payables at {period}");
    }
}

/// The last day of a `YYYY-MM` period, for the book's `balances_through`.
fn periodend(period: &str) -> String {
    let month: u32 = period[5..].parse().expect("month");
    let last = match month {
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    format!("{period}-{last:02}")
}

#[test]
fn the_top_expense_accounts_rank_the_golden_expenses_and_split_by_cost_centre() {
    let f = fixture();
    seed_once(&f);
    let body = trend_months(
        &f,
        &format!("{YEAR}-01"),
        &format!("{YEAR}-12"),
        Some("cost_centre"),
    );

    let top = body["top_expense_accounts"].as_array().expect("top");
    let mut golden: Vec<(&str, i64)> = GOLDEN
        .year_end_balances
        .iter()
        .filter(|(account, balance)| account.starts_with('5') && *balance > 0)
        .map(|(account, balance)| (*account, *balance))
        .collect();
    golden.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let ranked: Vec<(&str, i64)> = top
        .iter()
        .map(|row| {
            (
                row["account"].as_str().expect("account"),
                minor(&row["amount"]),
            )
        })
        .collect();
    assert_eq!(
        ranked,
        golden
            .iter()
            .take(ranked.len())
            .copied()
            .collect::<Vec<_>>()
    );
    assert_eq!(ranked.len(), golden.len().min(8));
    assert_eq!(ranked[0], ("5020", 20_160_000));
    let shares: i64 = top
        .iter()
        .map(|row| row["share_bps"].as_i64().expect("bps"))
        .sum();
    assert!(
        shares <= 10_000 && shares > 9_000,
        "shares sum to {shares} bps"
    );
    assert_eq!(
        top[0]["share_bps"].as_i64(),
        Some(ledgers_ledger::report::share_bps(
            20_160_000,
            GOLDEN.fy_expenses
        ))
    );

    // Revenue and expenses per cost centre, from the book's tagged lines.
    let mut expected: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for entry in &book_entries() {
        for line in entry {
            for (kind, code) in &line.dimensions {
                if *kind != "cost_centre" {
                    continue;
                }
                let slot = expected.entry((*code).to_owned()).or_insert((0, 0));
                let signed = line.side.sign() * line.minor;
                if line.account.starts_with('4') {
                    slot.0 -= signed;
                } else if line.account.starts_with('5') {
                    slot.1 += signed;
                }
            }
        }
    }
    let actual: BTreeMap<String, (i64, i64)> = body["by_dimension"]
        .as_array()
        .expect("by_dimension")
        .iter()
        .map(|row| {
            (
                row["code"].as_str().expect("code").to_owned(),
                (minor(&row["revenue"]), minor(&row["expenses"])),
            )
        })
        .collect();
    assert!(!expected.is_empty(), "the fixture tags cost centres");
    assert_eq!(actual, expected);
    assert_eq!(body["dimension"], "cost_centre");
}

/// The posted lines of every entry in the fixture, with reversals resolved.
fn book_entries() -> Vec<Vec<ledgers_demo::DemoLine>> {
    let book = halden::book(YEAR);
    book.entries
        .iter()
        .filter_map(|entry| book.posted_lines(entry))
        .collect()
}

#[test]
fn trends_reject_a_reversed_range_an_unknown_dimension_and_an_overlong_range() {
    let f = fixture();
    seed_once(&f);
    for path in [
        format!("/v1/reports/trends?from={YEAR}-06&to={YEAR}-01"),
        format!("/v1/reports/trends?from={YEAR}-01&to={YEAR}-12&dimension=colour"),
        "/v1/reports/trends?from=2016-01&to=2026-12".to_owned(),
        format!("/v1/reports/trends?from={YEAR}-13&to={YEAR}-12"),
    ] {
        let response = f.send(Method::GET, &path, &f.keys.full, None);
        assert_eq!(response.status.as_u16(), 422, "{path}: {}", response.json());
    }
}

#[test]
fn the_trend_activity_counts_every_entry_by_who_posted_it() {
    let f = fixture();
    seed_once(&f);
    let body = trend_months(&f, &format!("{YEAR}-01"), &format!("{YEAR}-12"), None);
    let book = halden::book(YEAR);
    let (mut agent, mut human) = (0_i64, 0_i64);
    for entry in &book.entries {
        match entry.poster {
            ledgers_demo::Poster::Agent => agent += 1,
            ledgers_demo::Poster::Person => human += 1,
        }
    }
    let activity = body["activity"].as_array().expect("activity");
    let counted_agent: i64 = activity
        .iter()
        .map(|month| month["agent"].as_i64().expect("agent"))
        .sum();
    let counted_human: i64 = activity
        .iter()
        .map(|month| month["human"].as_i64().expect("human"))
        .sum();
    assert_eq!(counted_agent, agent);
    assert_eq!(counted_human, human);
    assert!(
        human > 0 && agent > 0,
        "the fixture has both kinds of poster"
    );
}
