//! The fixture balances, reverses only what it posted, and reproduces the
//! golden totals. The server tests run the same fixture through the real
//! executor and reports (ledgers-server, `demo_seed`).

use ledgers_demo::halden::{self, GOLDEN};
use ledgers_demo::{EntryKind, Poster, balance_of, vat_19};
use std::collections::BTreeSet;

#[test]
fn every_entry_balances() {
    let book = halden::book(2026);
    for entry in &book.entries {
        if let EntryKind::Post { lines } = &entry.kind {
            assert_eq!(
                balance_of(lines),
                0,
                "entry {} does not balance",
                entry.external_id
            );
            assert!(lines.len() >= 2, "entry {} has one line", entry.external_id);
        }
    }
}

#[test]
fn external_ids_are_unique() {
    let book = halden::book(2026);
    let ids: BTreeSet<&str> = book
        .entries
        .iter()
        .map(|entry| entry.external_id.as_str())
        .collect();
    assert_eq!(ids.len(), book.entries.len());
}

#[test]
fn every_reversal_targets_an_earlier_posted_entry() {
    let book = halden::book(2026);
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut reversals = 0;
    for entry in &book.entries {
        if let EntryKind::Reverse { of } = &entry.kind {
            reversals += 1;
            assert!(
                seen.contains(of.as_str()),
                "{} reverses {of}, which is not an earlier entry",
                entry.external_id
            );
            let original = book.entry(of).expect("target exists");
            assert!(
                matches!(original.kind, EntryKind::Post { .. }),
                "{of} is itself a reversal"
            );
        }
        seen.insert(entry.external_id.as_str());
    }
    assert!(reversals >= 2, "the fixture shows reversals: {reversals}");
}

#[test]
fn both_agent_and_person_post() {
    let book = halden::book(2026);
    let agent = book
        .entries
        .iter()
        .filter(|e| e.poster == Poster::Agent)
        .count();
    let person = book
        .entries
        .iter()
        .filter(|e| e.poster == Poster::Person)
        .count();
    assert!(agent > 0, "no agent entries");
    assert!(person > 0, "no person entries");
}

#[test]
fn every_standard_rate_line_carries_the_vat_it_should() {
    // The sales invoices: receivable gross = net + 19 percent of net.
    let book = halden::book(2026);
    let invoice = book
        .entry("halden-03-invoice-nordstrand")
        .expect("March frames invoice");
    let EntryKind::Post { lines } = &invoice.kind else {
        panic!("invoice is a post");
    };
    let net = lines
        .iter()
        .find(|l| l.account == "4010")
        .expect("revenue line")
        .minor;
    let output = lines
        .iter()
        .find(|l| l.account == "2020")
        .expect("output VAT line")
        .minor;
    assert_eq!(output, vat_19(net));
    assert_eq!(vat_19(1_890_000), 359_100);
    assert_eq!(
        vat_19(50),
        10,
        "9.5 cents rounds to 10 (half to even: 9.5 -> 10)"
    );
    assert_eq!(vat_19(10), 2, "1.9 cents rounds to 2");
}

#[test]
fn golden_year_end_balances_match_the_fixture() {
    let book = halden::book(2026);
    let balances = book.balances_through("2026-12-31");
    let expected: Vec<(&str, i64)> = GOLDEN.year_end_balances.to_vec();
    let actual: Vec<(&str, i64)> = balances.into_iter().collect();
    assert_eq!(actual, expected);
}

#[test]
fn golden_year_totals_match_the_fixture() {
    let book = halden::book(2026);
    let balances = book.balances_through("2026-12-31");
    let revenue: i64 = balances
        .iter()
        .filter(|(account, _)| account.starts_with('4'))
        .map(|(_, balance)| -balance)
        .sum();
    let expenses: i64 = balances
        .iter()
        .filter(|(account, _)| account.starts_with('5'))
        .map(|(_, balance)| *balance)
        .sum();
    assert_eq!(revenue, GOLDEN.fy_revenue);
    assert_eq!(expenses, GOLDEN.fy_expenses);
    assert_eq!(revenue - expenses, GOLDEN.fy_net_income);
}

#[test]
fn the_books_balance_as_a_whole() {
    let book = halden::book(2026);
    let total: i64 = book.balances_through("2026-12-31").values().sum();
    assert_eq!(total, 0, "debits equal credits across the year");
}

#[test]
fn the_bank_never_goes_overdrawn_at_a_month_end() {
    let book = halden::book(2026);
    let mut running = 0;
    for month in 1..=12 {
        let key = format!("2026-{month:02}");
        running += book
            .activity_in_month(&key)
            .get("1020")
            .copied()
            .unwrap_or(0);
        assert!(
            running >= 0,
            "bank overdrawn at the end of {key}: {running}"
        );
    }
}

#[test]
fn the_fixture_names_its_company_and_currency() {
    let book = halden::book(2026);
    assert_eq!(book.company, "Halden Works GmbH");
    assert_eq!(book.currency, "EUR");
    assert_eq!(book.dimensions.len(), 4);
}

#[test]
fn vat_is_the_german_standard_rate_from_the_tax_crate_and_money_rounding() {
    use ledgers_money::{Amount, Currency, Rate, Rounding};
    use ledgers_tax::rate::{Jurisdiction, RateCategory};

    let date = time::Date::from_calendar_date(2026, time::Month::June, 1).expect("a date");
    let table = ledgers_tax::pack_by_id("de")
        .expect("the German pack")
        .rate_table(&[])
        .expect("the German rates");
    let standard = table
        .rate_on(&Jurisdiction::new("DE"), RateCategory::Standard, date)
        .expect("a standard rate in 2026")
        .rate;
    assert_eq!(standard, Rate::parse("0.19").expect("a rate"));

    let eur = Currency::from_code("EUR").expect("EUR");
    for net in [1, 9, 10, 50, 51, 18_900, 123_456, 2_140_000, 12_000_000] {
        let by_money = Amount::from_i64(net, eur)
            .mul_rate(standard, Rounding::HalfEven)
            .expect("VAT")
            .to_i64()
            .expect("minor units");
        assert_eq!(vat_19(net), by_money, "VAT on {net} minor units");
    }
}
