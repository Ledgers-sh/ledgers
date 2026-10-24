//! The Factory Zero demo book: its entries balance, every profit-and-loss
//! line carries a unit, the recharges net to zero, and the consolidated
//! golden totals hold (the intercompany recharges are eliminated).

use ledgers_demo::factory_zero::{self, GOLDEN};
use ledgers_demo::{balance_of, is_intercompany};

fn lines_of(entry: &ledgers_demo::DemoEntry) -> &[ledgers_demo::DemoLine] {
    entry.kind.lines().unwrap_or(&[])
}

#[test]
fn every_entry_balances_and_the_book_has_eight_units() {
    let book = factory_zero::book(2026);
    assert_eq!(book.company, "Factory Zero Pte. Ltd. (demo)");
    assert_eq!(book.currency, "SGD");
    assert_eq!(book.units.len(), 8, "the holding and seven ventures");
    for entry in &book.entries {
        if let Some(lines) = entry.kind.lines() {
            assert_eq!(balance_of(lines), 0, "entry {} balances", entry.external_id);
        }
    }
}

#[test]
fn every_profit_and_loss_line_carries_a_unit() {
    let book = factory_zero::book(2026);
    for entry in &book.entries {
        for line in lines_of(entry) {
            let pnl = matches!(line.account.as_bytes().first(), Some(b'4'..=b'9'));
            if pnl {
                assert!(
                    line.dimensions.iter().any(|(kind, _)| *kind == "unit"),
                    "{}: {} has no unit",
                    entry.external_id,
                    line.account
                );
            }
        }
    }
}

#[test]
fn every_recharge_is_intercompany_and_nets_its_profit_and_loss_to_zero() {
    let book = factory_zero::book(2026);
    let recharges: Vec<_> = book
        .entries
        .iter()
        .filter(|entry| entry.external_id.ends_with("shared-recharge"))
        .collect();
    assert_eq!(recharges.len(), 12 * 7, "seven ventures, twelve months");
    for entry in recharges {
        assert!(is_intercompany(entry));
        let pnl: i64 = lines_of(entry)
            .iter()
            .filter(|line| matches!(line.account.as_bytes().first(), Some(b'4'..=b'9')))
            .map(|line| line.side.sign() * line.minor)
            .sum();
        assert_eq!(pnl, 0, "{} nets its P&L to zero", entry.external_id);
    }
}

#[test]
fn the_holding_recovers_exactly_the_shared_costs() {
    let book = factory_zero::book(2026);
    let shared: i64 = (18_000 + 4_200 + 1_100) * 100 * 12;
    let recovered: i64 = book
        .entries
        .iter()
        .filter(|entry| entry.external_id.ends_with("shared-recharge"))
        .flat_map(|entry| lines_of(entry).iter())
        .filter(|line| line.account == "4030")
        .map(|line| line.minor)
        .sum();
    assert_eq!(recovered, shared, "the allocation key recovers every cent");
}

#[test]
fn consolidated_golden_totals_hold() {
    let book = factory_zero::book(2026);
    let balances = book.balances_through("2026-12-31");
    let revenue: i64 = balances
        .iter()
        .filter(|(account, _)| account.starts_with('4') && **account != "4030")
        .map(|(_, balance)| -balance)
        .sum();
    let expenses: i64 = balances
        .iter()
        .filter(|(account, _)| account.starts_with('5'))
        .map(|(_, balance)| *balance)
        .sum();
    assert_eq!(
        revenue, GOLDEN.fy_revenue,
        "consolidated subscription revenue"
    );
    assert_eq!(expenses, GOLDEN.fy_expenses, "consolidated expenses");
    assert!(
        !balances.contains_key("1070") && !balances.contains_key("2050"),
        "the intercompany accounts net to nothing consolidated"
    );
}
