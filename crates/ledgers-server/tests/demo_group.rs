//! The Factory Zero group demo (issue #128) through its seed path: the
//! operating book loads, the IP company and the licence follow, a second run
//! posts nothing, and the reconciliation shows exactly the last month open.
mod common;

use common::fixture;
use ledgers_commands::auth::{Authenticator, Credential, Principal, Role};
use ledgers_commands::{CommandError, DefaultPolicy, Execute, Executed, Executor};
use ledgers_demo::factory_zero;
use ledgers_group::commands::ReconciliationInput;
use ledgers_group::reports::ReconciliationReport;
use ledgers_server::demo_group;

const FZ_COMPANY: &str = "01FZDEMOCOMPANY00000000000A";
const IP_COMPANY: &str = "01FZDEMOIPCOMPANY0000000000";

/// An authenticator for the test owner.
struct Owner;

#[async_trait::async_trait]
impl Authenticator for Owner {
    async fn authenticate(&self, _credential: &Credential) -> Result<Principal, CommandError> {
        Ok(Principal::Person {
            person_id: "test-owner".to_owned(),
            role: Role::Owner,
        })
    }
}

#[test]
fn the_group_demo_posts_a_year_settles_eleven_months_and_is_idempotent() {
    let fx = fixture();
    // The deployment's bootstrap owner: the home company's owner, who the demo
    // makes an owner of the IP company as well.
    pollster::block_on(fx.deps.db.execute(&cratefield_core::Statement::with_values(
        "INSERT INTO org_members (org_id, user_sub, role, invited_by, created_at) VALUES (?, 'person-home-owner', 'owner', NULL, ?)"
            .to_owned(),
        vec![
            sea_query::Value::from(common::COMPANY),
            sea_query::Value::from(common::NOW),
        ],
    )))
    .expect("the home owner");
    let book = factory_zero::book(2026);
    pollster::block_on(ledgers_server::demo::seed_company_book(&fx.deps, FZ_COMPANY, &book))
        .expect("the operating book loads");

    let first = pollster::block_on(demo_group::seed(
        &fx.deps,
        common::COMPANY,
        FZ_COMPANY,
        IP_COMPANY,
        2026,
    ))
    .expect("the group demo seeds");
    assert_eq!(first.posted, 12, "{first:?}");
    assert_eq!(first.settled, 11, "{first:?}");
    assert_eq!(first.open_period.as_deref(), Some("2026-12"), "{first:?}");

    let again = pollster::block_on(demo_group::seed(
        &fx.deps,
        common::COMPANY,
        FZ_COMPANY,
        IP_COMPANY,
        2026,
    ))
    .expect("a second run");
    assert_eq!(again.posted, 0, "a second run posts nothing: {again:?}");
    assert_eq!(again.settled, 0, "{again:?}");
    assert_eq!(again.open_period.as_deref(), Some("2026-12"));

    let report = ReconciliationReport {
        accessible: vec![FZ_COMPANY.to_owned(), IP_COMPANY.to_owned()],
    };
    let executor = Executor::new(fx.deps.db.as_ref(), Owner, DefaultPolicy);
    let out = pollster::block_on(executor.run(
        &report,
        "owner".to_owned(),
        ledgers_commands::CommandContext {
            company_id: FZ_COMPANY.to_owned(),
            request_id: "recon".to_owned(),
            event_id: "recon-event".to_owned(),
            now: "2026-10-23T10:00:00Z".to_owned(),
        },
        &ReconciliationInput {
            id: demo_group::GROUP_ID.to_owned(),
        },
        Execute::Run,
    ))
    .expect("reconciliation");
    let Executed::Done(out) = out else {
        panic!("the report runs");
    };
    assert_eq!(out.pairs.len(), 1);
    let pair = &out.pairs[0];
    assert_eq!(pair.balance_difference_minor, 0, "{pair:?}");
    assert_eq!(pair.income_difference_minor, 0, "{pair:?}");
    assert_eq!(pair.status, "balanced");
    assert_eq!(pair.open_periods, vec!["2026-12".to_owned()], "{pair:?}");
    assert!(pair.receivable_minor > 0, "the open month is still owed: {pair:?}");
    assert_eq!(pair.receivable_minor, pair.payable_minor);
}
