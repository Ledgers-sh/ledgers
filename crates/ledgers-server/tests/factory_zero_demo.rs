//! The Factory Zero demo loaded through the seed path into its own company:
//! it passes every invariant check, seeding again posts nothing, the
//! consolidated goldens hold, and the units comparison eliminates exactly
//! the holding's recoveries.

mod common;

use common::fixture;
use ledgers_demo::factory_zero::{self, GOLDEN};
use ledgers_ledger::report;

const FZ_COMPANY: &str = "01FZDEMOCOMPANY00000000000A";

#[test]
fn factory_zero_loads_checks_and_reconciles() {
    let fx = fixture();
    let book = factory_zero::book(2026);
    let first = pollster::block_on(ledgers_server::demo::seed_company_book(
        &fx.deps, FZ_COMPANY, &book,
    ))
    .expect("the book loads");
    assert_eq!(
        first.posted,
        book.entries.len() - 1,
        "every entry but the reversal posts"
    );
    assert_eq!(first.reversed, 1, "{first:?}");
    assert_eq!(first.units_created, 8, "{first:?}");

    let checked = pollster::block_on(ledgers_server::demo::check(&fx.deps, FZ_COMPANY, &book))
        .expect("every invariant holds");
    assert_eq!(checked.reversals_verified, 1);

    let again = pollster::block_on(ledgers_server::demo::seed_company_book(
        &fx.deps, FZ_COMPANY, &book,
    ))
    .expect("a second run");
    assert_eq!(again.posted, 0, "a second run posts nothing: {again:?}");
    assert_eq!(again.units_created, 0);

    let db = fx.deps.db.as_ref();
    // Consolidated golden totals for the year: the holding's recoveries and
    // the ventures' recharges are eliminated.
    let mut revenue = 0_i128;
    let mut expenses = 0_i128;
    for month in 1..=12 {
        let period = format!("2026-{month:02}");
        let pnl =
            pollster::block_on(report::profit_and_loss(db, FZ_COMPANY, &period)).expect("p&l");
        revenue += pnl.revenue.minor();
        expenses += pnl.expenses.minor();
    }
    assert_eq!(
        i64::try_from(revenue).expect("fits"),
        GOLDEN.fy_revenue,
        "consolidated revenue"
    );
    assert_eq!(
        i64::try_from(expenses).expect("fits"),
        GOLDEN.fy_expenses,
        "consolidated expenses"
    );

    // June, side by side: the eliminations are the recoveries (23,300 SGD),
    // on both the revenue and the expense side.
    let june =
        pollster::block_on(report::units_summary(db, FZ_COMPANY, "2026-06")).expect("summary");
    assert_eq!(june.units.len(), 8, "one row per unit");
    assert_eq!(
        june.eliminations.revenue.minor(),
        -2_330_000,
        "recoveries eliminated"
    );
    assert_eq!(
        june.eliminations.expenses.minor(),
        -2_330_000,
        "recharges eliminated"
    );
    let sum_revenue: i128 = june.units.iter().map(|row| row.revenue.minor()).sum();
    assert_eq!(
        sum_revenue + june.eliminations.revenue.minor(),
        june.consolidated.revenue.minor()
    );
}
