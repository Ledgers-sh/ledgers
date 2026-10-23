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
use ledgers_server::dispatch::Deps;
use ledgers_server::timing::{CountingDb, DbTiming};
use std::sync::Arc;

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

fn assert_reconciled(fx: &common::Fixture) {
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
    assert!(
        pair.receivable_minor > 0,
        "the open month is still owed: {pair:?}"
    );
    assert_eq!(pair.receivable_minor, pair.payable_minor);
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
    pollster::block_on(ledgers_server::demo::seed_company_book(
        &fx.deps, FZ_COMPANY, &book,
    ))
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

    assert_reconciled(&fx);
}

/// The most database round trips one step may make. A Worker request has a time
/// budget and each round trip is a network hop to D1, so a step stays small.
const STEP_CALL_BOUND: u64 = 400;

/// The fixture's dependencies with every database call counted.
fn counted(fx: &common::Fixture) -> (Deps, Arc<DbTiming>) {
    let timing = Arc::new(DbTiming::default());
    let deps = Deps {
        db: Arc::new(CountingDb::new(
            Arc::clone(&fx.deps.db),
            Arc::clone(&timing),
        )),
        clock: Arc::clone(&fx.deps.clock),
        id_gen: Arc::clone(&fx.deps.id_gen),
        rate_limiter: fx.deps.rate_limiter.clone(),
        blob: fx.deps.blob.clone(),
        http: fx.deps.http.clone(),
    };
    (deps, timing)
}

/// The deployment's bootstrap owner and the operating book, as the seed route
/// finds them before the group steps run.
fn load_operating_book(fx: &common::Fixture) {
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
}

/// Runs steps `0..GROUP_STEPS` in order on `deps`, returning the posted and
/// settled counts and the last step's open period.
fn run_all_steps(deps: &Deps, timing: &DbTiming) -> (usize, usize, Option<String>) {
    let mut posted = 0;
    let mut settled = 0;
    let mut open = None;
    for step in 0..demo_group::GROUP_STEPS {
        let before = timing.snapshot().calls;
        let out = pollster::block_on(demo_group::seed_step(
            deps,
            common::COMPANY,
            FZ_COMPANY,
            IP_COMPANY,
            2026,
            step,
        ))
        .expect("the step runs");
        let calls = timing.snapshot().calls - before;
        assert!(
            calls <= STEP_CALL_BOUND,
            "step {step} made {calls} round trips"
        );
        assert_eq!(out.step, step);
        assert_eq!(out.done, step == demo_group::GROUP_STEPS - 1, "{out:?}");
        assert_eq!(
            out.next,
            (step + 1 < demo_group::GROUP_STEPS).then_some(step + 1),
            "{out:?}"
        );
        posted += out.posted;
        settled += out.settled;
        if out.open_period.is_some() {
            open = out.open_period;
        }
    }
    (posted, settled, open)
}

#[test]
fn each_group_step_is_bounded_and_the_steps_finish_the_demo_in_order() {
    let fx = fixture();
    load_operating_book(&fx);
    let (deps, timing) = counted(&fx);
    assert!(
        !pollster::block_on(demo_group::operating_book_present(&deps, IP_COMPANY)).unwrap(),
        "the IP company has no book before the steps run"
    );
    assert!(pollster::block_on(demo_group::operating_book_present(&deps, FZ_COMPANY)).unwrap());

    let (posted, settled, open) = run_all_steps(&deps, &timing);
    assert_eq!(posted, 12, "every month posts once");
    assert_eq!(settled, 11, "every month but the last settles");
    assert_eq!(open.as_deref(), Some("2026-12"));
}

#[test]
fn repeated_group_steps_post_and_settle_nothing_new() {
    let fx = fixture();
    load_operating_book(&fx);
    let (deps, timing) = counted(&fx);
    run_all_steps(&deps, &timing);

    let (posted, settled, open) = run_all_steps(&deps, &timing);
    assert_eq!((posted, settled), (0, 0), "a second pass changes nothing");
    assert_eq!(open.as_deref(), Some("2026-12"));
}

#[test]
fn a_step_past_the_last_is_refused_and_the_step_parameter_is_checked() {
    let fx = fixture();
    load_operating_book(&fx);
    let (deps, _timing) = counted(&fx);
    let refused = pollster::block_on(demo_group::seed_step(
        &deps,
        common::COMPANY,
        FZ_COMPANY,
        IP_COMPANY,
        2026,
        demo_group::GROUP_STEPS,
    ));
    assert!(refused.is_err(), "{refused:?}");

    assert_eq!(demo_group::parse_step(None), Ok(0));
    assert_eq!(demo_group::parse_step(Some("0")), Ok(0));
    assert_eq!(demo_group::parse_step(Some("23")), Ok(23));
    for bad in ["24", "-1", "x", "", "1.5"] {
        assert!(
            demo_group::parse_step(Some(bad)).is_err(),
            "{bad:?} is refused"
        );
    }
}

#[test]
fn a_rerun_of_a_loaded_operating_book_reads_it_in_bounded_round_trips() {
    let fx = fixture();
    let book = factory_zero::book(2026);
    pollster::block_on(ledgers_server::demo::seed_company_book(
        &fx.deps, FZ_COMPANY, &book,
    ))
    .expect("the operating book loads");

    let (deps, timing) = counted(&fx);
    let report = pollster::block_on(ledgers_server::demo::seed_company_book(
        &deps, FZ_COMPANY, &book,
    ))
    .expect("the re-run");
    assert_eq!(report.posted, 0, "{report:?}");
    assert!(report.skipped > 200, "{report:?}");
    let calls = timing.snapshot().calls;
    assert!(
        calls <= 100,
        "a re-run of {} entries made {calls} round trips",
        report.skipped
    );
}

#[test]
fn the_steps_reconcile_like_the_one_go_seed() {
    let fx = fixture();
    load_operating_book(&fx);
    let (deps, timing) = counted(&fx);
    run_all_steps(&deps, &timing);
    assert_reconciled(&fx);
}

/// Counts the operating company's accounts with `number` in the books.
fn accounts_numbered(fx: &common::Fixture, company: &str, number: &str) -> i64 {
    let rows = pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
        "SELECT COUNT(*) AS n FROM account WHERE company_id = ? AND number = ?".to_owned(),
        vec![
            sea_query::Value::from(company),
            sea_query::Value::from(number),
        ],
    )))
    .expect("count accounts");
    rows.rows[0].get::<i64>("n").expect("the count")
}

#[test]
fn the_setup_adds_every_group_account_a_chart_lacks_in_one_step() {
    let fx = fixture();
    load_operating_book(&fx);
    // An operating chart that lacks two of the licence's accounts, as a chart
    // loaded before they existed does.
    pollster::block_on(fx.deps.db.execute(&cratefield_core::Statement::with_values(
        "DELETE FROM account WHERE company_id = ? AND number IN (?, ?)".to_owned(),
        vec![
            sea_query::Value::from(FZ_COMPANY),
            sea_query::Value::from("5120"),
            sea_query::Value::from("2060"),
        ],
    )))
    .expect("remove two accounts");
    assert_eq!(accounts_numbered(&fx, FZ_COMPANY, "5120"), 0);

    let (deps, _timing) = counted(&fx);
    pollster::block_on(demo_group::seed_step(
        &deps,
        common::COMPANY,
        FZ_COMPANY,
        IP_COMPANY,
        2026,
        0,
    ))
    .expect("step 0 adds both accounts, one event each");
    assert_eq!(accounts_numbered(&fx, FZ_COMPANY, "5120"), 1);
    assert_eq!(accounts_numbered(&fx, FZ_COMPANY, "2060"), 1);
}
