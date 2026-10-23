//! The Factory Zero logo seed (group and venture logos) through its seed path:
//! every business unit, both companies and the group gain a logo, a second run
//! stores and attaches nothing new, and a step past the last is refused.
mod common;

use common::fixture;
use ledgers_demo::factory_zero;
use ledgers_documents::logo::check_logo;
use ledgers_server::demo_group;
use ledgers_server::demo_logos;

const FZ_COMPANY: &str = "01FZDEMOCOMPANY00000000000A";
const IP_COMPANY: &str = "01FZDEMOIPCOMPANY0000000000";
const LOGO_UNITS: [&str; 8] = ["FZ", "LDG", "SQZ", "LB", "CF", "OWL", "COL", "SEAL"];

/// One count query, read as an integer.
fn count(fx: &common::Fixture, sql: &str, values: Vec<sea_query::Value>) -> i64 {
    let rows = pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
        sql.to_owned(),
        values,
    )))
    .expect("count");
    rows.rows[0].get::<i64>("n").expect("the count")
}

/// The operating book, the group demo and the logo seed, run in order.
fn load_everything(fx: &common::Fixture) {
    pollster::block_on(fx.deps.db.execute(&cratefield_core::Statement::with_values(
        "INSERT INTO org_members (org_id, user_sub, role, invited_by, created_at) VALUES (?, 'person-home-owner', 'owner', NULL, ?)"
            .to_owned(),
        vec![
            sea_query::Value::from(common::COMPANY),
            sea_query::Value::from(common::NOW),
        ],
    )))
    .expect("the home owner");
    pollster::block_on(ledgers_server::demo::seed_company_book(
        &fx.deps,
        FZ_COMPANY,
        &factory_zero::book(2026),
    ))
    .expect("the operating book loads");
    pollster::block_on(demo_group::seed(
        &fx.deps,
        common::COMPANY,
        FZ_COMPANY,
        IP_COMPANY,
        2026,
    ))
    .expect("the group demo seeds");
}

#[test]
fn the_logo_seed_attaches_every_unit_company_and_group_logo() {
    let fx = fixture();
    load_everything(&fx);

    let first = pollster::block_on(demo_logos::seed(&fx.deps, FZ_COMPANY, IP_COMPANY))
        .expect("the logo seed runs");
    assert_eq!(first, 11, "eight units, two companies, one group");

    for code in LOGO_UNITS {
        let linked = count(
            &fx,
            "SELECT COUNT(*) AS n FROM dimension_value d JOIN document doc \
             ON doc.company_id = d.company_id AND doc.id = d.logo_document_id \
             WHERE d.company_id = ? AND d.kind = 'unit' AND d.code = ? AND doc.mime = 'image/png'",
            vec![
                sea_query::Value::from(FZ_COMPANY),
                sea_query::Value::from(code),
            ],
        );
        assert_eq!(linked, 1, "unit {code} has a stored PNG logo");
    }
    for company in [FZ_COMPANY, IP_COMPANY] {
        let profile = pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
            "SELECT value FROM company_setting WHERE company_id = ? AND key = 'company.profile'"
                .to_owned(),
            vec![sea_query::Value::from(company)],
        )))
        .expect("the profile");
        let text: String = profile.rows[0].get("value").expect("a profile");
        assert!(
            text.contains("\"logo_document_id\":\"") && !text.contains("\"logo_document_id\":\"\""),
            "{company} has a logo: {text}"
        );
    }
    let group_logo = count(
        &fx,
        "SELECT COUNT(*) AS n FROM company_group WHERE logo_document_id IS NOT NULL AND logo_company_id = ?",
        vec![sea_query::Value::from(FZ_COMPANY)],
    );
    assert_eq!(
        group_logo, 1,
        "the group logo names a document of the operating company"
    );

    // Every logo the seed stored is within the logo rule.
    let stored = pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
        "SELECT mime, size_bytes FROM document WHERE company_id = ?".to_owned(),
        vec![sea_query::Value::from(FZ_COMPANY)],
    )))
    .expect("documents");
    assert!(!stored.rows.is_empty());
    for row in &stored.rows {
        let mime: String = row.get("mime").expect("mime");
        let size: i64 = row.get("size_bytes").expect("size");
        assert_eq!(check_logo(&mime, size), Ok(()), "{mime} {size}");
    }
}

#[test]
fn a_second_logo_run_stores_and_attaches_nothing_new() {
    let fx = fixture();
    load_everything(&fx);
    pollster::block_on(demo_logos::seed(&fx.deps, FZ_COMPANY, IP_COMPANY)).expect("first run");
    let documents = count(
        &fx,
        "SELECT COUNT(*) AS n FROM document WHERE company_id = ?",
        vec![sea_query::Value::from(FZ_COMPANY)],
    );

    let again =
        pollster::block_on(demo_logos::seed(&fx.deps, FZ_COMPANY, IP_COMPANY)).expect("second run");
    assert_eq!(again, 11, "the same attachments are written again");
    assert_eq!(
        count(
            &fx,
            "SELECT COUNT(*) AS n FROM document WHERE company_id = ?",
            vec![sea_query::Value::from(FZ_COMPANY)],
        ),
        documents,
        "identical bytes register once"
    );
}

#[test]
fn the_logo_steps_run_in_order_and_the_step_parameter_is_checked() {
    let fx = fixture();
    load_everything(&fx);
    assert_eq!(demo_logos::parse_step(None), Ok(0));
    assert_eq!(demo_logos::parse_step(Some("1")), Ok(1));
    assert!(demo_logos::parse_step(Some("2")).is_err());
    assert!(demo_logos::parse_step(Some("-1")).is_err());
    assert!(demo_logos::parse_step(Some("one")).is_err());

    let deps = &fx.deps;
    let first =
        pollster::block_on(demo_logos::seed_step(deps, FZ_COMPANY, IP_COMPANY, 0)).expect("step 0");
    assert!(!first.done);
    assert_eq!(first.next, Some(1));
    let last =
        pollster::block_on(demo_logos::seed_step(deps, FZ_COMPANY, IP_COMPANY, 1)).expect("step 1");
    assert!(last.done);
    assert_eq!(last.next, None);
    assert!(
        pollster::block_on(demo_logos::seed_step(deps, FZ_COMPANY, IP_COMPANY, 2)).is_err(),
        "a step past the last is refused"
    );
}

#[test]
fn the_logo_seed_needs_the_group_demo_first() {
    let fx = fixture();
    pollster::block_on(ledgers_server::demo::seed_company_book(
        &fx.deps,
        FZ_COMPANY,
        &factory_zero::book(2026),
    ))
    .expect("the operating book loads");
    let refused = pollster::block_on(demo_logos::seed_step(&fx.deps, FZ_COMPANY, IP_COMPANY, 0));
    assert!(
        refused.is_err_and(|message| message.contains("group demo")),
        "the group must exist before its logo is set"
    );
}
