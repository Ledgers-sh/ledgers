//! The Factory Zero demo connections (issue #142): seeded through the same
//! commands as a real connection, from recorded fixtures, with no token. The
//! costs land in the units of the book, the unmapped resource lands in the
//! needs-mapping queue, and a second seed creates nothing.

mod common;

use common::fixture;
use ledgers_demo::factory_zero;
use ledgers_server::dispatch::ServerDispatch;
use sea_query::Value;

const FZ_COMPANY: &str = "01FZDEMOCOMPANY00000000000A";

fn scalar(db: &dyn cratefield_core::Database, sql: &str) -> i64 {
    let rows = pollster::block_on(db.query(&cratefield_core::Statement::new(sql.to_owned())))
        .unwrap_or_else(|err| panic!("{err}\n{sql}"));
    rows.rows
        .first()
        .and_then(|row| row.get::<i64>("n"))
        .unwrap_or(0)
}

#[test]
fn the_demo_connections_seed_once_and_split_the_costs_across_ventures() {
    let fx = fixture();
    let book = factory_zero::book(2026);
    pollster::block_on(ledgers_server::demo::seed_company_book(
        &fx.deps, FZ_COMPANY, &book,
    ))
    .expect("the book loads");
    let dispatch = ServerDispatch::new(FZ_COMPANY);
    let created =
        pollster::block_on(dispatch.seed_demo_connections(&fx.deps, 2026)).expect("seeds");
    assert_eq!(created, 3, "Cloudflare, Anthropic and PlanetScale");
    let again =
        pollster::block_on(dispatch.seed_demo_connections(&fx.deps, 2026)).expect("seeds again");
    assert_eq!(again, 0, "a second seed creates nothing");

    let db = fx.deps.db.as_ref();
    assert_eq!(
        scalar(
            db,
            "SELECT COUNT(*) AS n FROM connector_connection WHERE is_demo = 1"
        ),
        3
    );
    assert_eq!(
        scalar(
            db,
            "SELECT COUNT(*) AS n FROM connector_credential WHERE company_id = '01FZDEMOCOMPANY00000000000A'"
        ),
        0,
        "demo connections hold no token"
    );
    assert!(
        scalar(db, "SELECT COUNT(*) AS n FROM connector_invoice") >= 30,
        "12 months from each of three sources, less what is not invoiced"
    );
    assert_eq!(
        scalar(
            db,
            "SELECT COUNT(*) AS n FROM bill WHERE posted_entry_id IS NOT NULL"
        ),
        0,
        "the demo costs are drafts; nothing posts"
    );

    // Every invoice's splits sum to its net amount, and the costs reach several ventures.
    let mismatched = scalar(
        db,
        "SELECT COUNT(*) AS n FROM (SELECT i.id FROM connector_invoice i JOIN connector_split s ON s.invoice_id = i.id GROUP BY i.id, i.total_minor HAVING SUM(s.amount_minor) <> i.total_minor) AS bad",
    );
    assert_eq!(mismatched, 0, "every split sums to its invoice");
    let units = scalar(
        db,
        "SELECT COUNT(DISTINCT unit_code) AS n FROM connector_split",
    );
    assert!(units >= 5, "costs land in several ventures, got {units}");
    let ledgers = scalar(
        db,
        "SELECT COALESCE(SUM(amount_minor), 0) AS n FROM connector_split WHERE unit_code = 'LDG'",
    );
    assert!(ledgers > 0, "Ledgers carries its own hosting");

    // The unmapped worker is in the default unit, and the run log says so.
    let notes = pollster::block_on(db.query(&cratefield_core::Statement::with_values(
        "SELECT notes_json AS n FROM connector_run WHERE connection_id IN (SELECT id FROM connector_connection WHERE connector = 'cloudflare')".to_owned(),
        Vec::<Value>::new(),
    )))
    .expect("runs");
    let text: String = notes
        .rows
        .iter()
        .filter_map(|row| row.get::<String>("n"))
        .collect();
    assert!(
        text.contains("tmp-preview-worker"),
        "the unmapped resource is named: {text}"
    );
}
