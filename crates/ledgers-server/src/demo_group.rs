//! The Factory Zero group demo (issue #128): an IP holding company that
//! licenses the software to the operating company (the demo book from
//! `ledgers-demo`) at 5% of its revenue.
//!
//! The demo is a fixed sequence of steps ([`GROUP_STEPS`]): the IP company
//! (Singapore, SGD, owned by the deployment's bootstrap owner), the group and
//! the licence; then, for every month of the demo year, a draft and post; then
//! a settlement of every month but the last, so the reconciliation shows one
//! open month. [`seed_step`] runs one step, so the Worker can run the demo one
//! bounded request at a time; [`seed`] runs all of them in one go. Both go
//! through the same commands and posting path as a person would use, as the
//! demo owner.
//!
//! Idempotent: a company, group or agreement that exists is left alone, a
//! period that is already posted or settled is skipped, and the accounts a
//! company's chart lacks are added once.

use crate::demo::{DEMO_OWNER_ID, OwnerAuth, grant_bootstrap_owner, run_as_person};
use crate::dispatch::{self, Deps};
use async_trait::async_trait;
use ledgers_audit::{Actor, AuditContext};
use ledgers_commands::{CommandContext, Execute, Executed, Executor};
use ledgers_group::commands::{
    AgreementsCreate, AgreementsCreateInput, AgreementsRunPeriod, GroupsCreate, GroupsCreateInput,
    MemberInput, RunPeriodInput,
};
use ledgers_group::model::{AccountsInput, Basis, GroupKind, Kind};
use ledgers_group::orchestrate::{self, SideExecutor};
use ledgers_group::store;
use ledgers_ledger::account::AccountDraft;
use ledgers_ledger::commands::{create_account, list_accounts};
use ledgers_ledger::surface::{PostEntry, PostEntryInput, PostEntryOutput};
use ledgers_policy::KeyPolicy;

/// The IP company's name (invented demo name).
pub const IP_COMPANY_NAME: &str = "Factory Zero IP Pte. Ltd. (demo)";
/// The group's id and name.
pub const GROUP_ID: &str = "01DEMOFZGROUP0000000000000";
/// The group's name.
pub const GROUP_NAME: &str = "Factory Zero Group";
/// The licence's id.
pub const LICENCE_ID: &str = "01DEMOFZLICENCE000000000000";
/// The business unit the licence's royalty expense posts under: the demo
/// book's holding and factory unit, which carries the shared costs.
pub const OPERATING_UNIT: &str = "FZ";
/// The licence rate: 5% of the operating company's revenue, in basis points.
pub const LICENCE_RATE_BP: i64 = 500;

/// The accounts the licence posts to, added to a chart that lacks them.
const GROUP_ACCOUNTS: [&str; 6] = ["1080", "1090", "2060", "2070", "4060", "5120"];

/// What the seed did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupSeedReport {
    /// Periods posted by this run.
    pub posted: usize,
    /// Periods settled by this run.
    pub settled: usize,
    /// The period left posted and unsettled, when there is one.
    pub open_period: Option<String>,
}

/// Runs one command as the demo owner and returns its output.
async fn output_as_owner<C>(
    deps: &Deps,
    company_id: &str,
    command: &C,
    input: &C::Input,
) -> Result<C::Output, String>
where
    C: ledgers_commands::Command + Sync,
    C::Input: Sync,
{
    let policy: KeyPolicy = dispatch::load_policy(deps.db.as_ref(), company_id, deps.clock.now())
        .await
        .map_err(|err| err.to_string())?;
    let ctx = CommandContext {
        company_id: company_id.to_owned(),
        request_id: format!("demo-group-{}", deps.id_gen.ulid()),
        event_id: deps.id_gen.ulid(),
        now: dispatch::rfc3339_utc(deps.clock.now()),
    };
    let executor = Executor::new(
        deps.db.as_ref(),
        OwnerAuth {
            person_id: DEMO_OWNER_ID.to_owned(),
        },
        policy,
    );
    match executor
        .run(command, DEMO_OWNER_ID.to_owned(), ctx, input, Execute::Run)
        .await
        .map_err(|err| err.to_string())?
    {
        Executed::Done(output) => Ok(output),
        _ => Err("the demo command did not run to completion".to_owned()),
    }
}

/// The two companies' executors, as the demo owner.
struct DemoSides<'a> {
    deps: &'a Deps,
}

#[async_trait]
impl SideExecutor for DemoSides<'_> {
    async fn post(&self, company_id: &str, input: &PostEntryInput) -> Result<String, String> {
        let out: PostEntryOutput =
            output_as_owner(self.deps, company_id, &PostEntry, input).await?;
        Ok(out.entry.entry_id)
    }

    async fn reverse(
        &self,
        company_id: &str,
        external_id: &str,
        posting_date: &str,
    ) -> Result<(), String> {
        let input = crate::dispatch::EntriesReverseInput {
            external_id: external_id.to_owned(),
            posting_date: Some(posting_date.to_owned()),
        };
        run_as_person(
            self.deps,
            company_id,
            &crate::dispatch::EntriesReverse,
            OwnerAuth {
                person_id: DEMO_OWNER_ID.to_owned(),
            },
            &input,
        )
        .await
    }
}

/// Adds the group's accounts to `company_id` when its chart lacks them. A
/// chart created before they existed gets them; a complete chart is untouched.
async fn ensure_accounts(deps: &Deps, company_id: &str) -> Result<(), String> {
    let db = deps.db.as_ref();
    let existing = list_accounts(db, company_id, true)
        .await
        .map_err(|err| err.to_string())?;
    let template = ledgers_ledger::generic_sme_template().map_err(|err| err.to_string())?;
    let stamp = dispatch::rfc3339_utc(deps.clock.now());
    for number in GROUP_ACCOUNTS {
        if existing.iter().any(|entry| entry.account.number == number) {
            continue;
        }
        let chart = template
            .accounts
            .iter()
            .find(|account| account.number == number)
            .ok_or_else(|| format!("the bundled chart has no account {number}"))?;
        let draft = AccountDraft {
            number: chart.number.clone(),
            name_key: chart.name.clone(),
            account_type: chart.account_type,
            normal_balance: Some(chart.normal_balance),
            currency: chart.currency,
            parent: chart.parent.clone(),
            tags: chart.tags.clone(),
            display_name: None,
        };
        // One audit event per account: each change is keyed by its event id.
        let audit = AuditContext::new(
            Actor::Person {
                person_id: DEMO_OWNER_ID.to_owned(),
            },
            deps.id_gen.ulid(),
            deps.id_gen.ulid(),
        );
        create_account(db, company_id, draft, stamp.clone(), &audit)
            .await
            .map_err(|err| format!("add account {number}: {err}"))?;
    }
    Ok(())
}

/// The number of steps the group demo takes. Step 0 creates the IP company,
/// the group and the licence. Steps 1 to 12 draft and post one month each.
/// Steps 13 to 23 settle months 1 to 11; month 12 stays open for the
/// reconciliation. A step is one bounded batch of work, so each one fits in a
/// single Worker request.
pub const GROUP_STEPS: u32 = 24;

/// The last step's number.
const LAST_STEP: u32 = GROUP_STEPS - 1;

/// What one step did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupStep {
    /// The step that ran.
    pub step: u32,
    /// Whether this was the last step, so the demo is complete.
    pub done: bool,
    /// The step to run next, `None` once the demo is complete.
    pub next: Option<u32>,
    /// Periods posted by this step (0 or 1).
    pub posted: usize,
    /// Periods settled by this step (0 or 1).
    pub settled: usize,
    /// On the last step: the period left posted and unsettled, when there is one.
    pub open_period: Option<String>,
}

/// Whether the operating company already holds a book. The group demo posts
/// against that book, so a step refuses to run without one.
///
/// # Errors
/// A message when the database read fails.
pub async fn operating_book_present(deps: &Deps, ops: &str) -> Result<bool, String> {
    let rows = deps
        .db
        .query(&cratefield_core::Statement::with_values(
            "SELECT 1 AS found FROM journal_entry WHERE company_id = ? LIMIT 1".to_owned(),
            vec![sea_query::Value::from(ops)],
        ))
        .await
        .map_err(|err| format!("read the operating book: {err}"))?;
    Ok(!rows.rows.is_empty())
}

/// Reads the `step` query parameter of the group demo route: absent means
/// step 0.
///
/// # Errors
/// A message when the value is not a whole number below [`GROUP_STEPS`].
pub fn parse_step(raw: Option<&str>) -> Result<u32, String> {
    let Some(raw) = raw else {
        return Ok(0);
    };
    raw.parse::<u32>()
        .ok()
        .filter(|step| *step < GROUP_STEPS)
        .ok_or_else(|| format!("step must be a whole number from 0 to {LAST_STEP}"))
}

/// The period of the demo's `index`th month (1 to 12) of `year`.
fn month(year: i32, index: u32) -> String {
    format!("{year:04}-{index:02}")
}

/// Runs one step of the group demo. Steps run in order, one per call, and
/// each is idempotent: a company, group, agreement or period that is already
/// there is left alone. The operating book must be present (see
/// [`operating_book_present`]).
///
/// # Errors
/// A message naming the step that failed: the company, group, agreement,
/// a period's draft, post or settlement, or a step past [`GROUP_STEPS`].
pub async fn seed_step(
    deps: &Deps,
    home_company: &str,
    ops: &str,
    ip: &str,
    year: i32,
    step: u32,
) -> Result<GroupStep, String> {
    if step >= GROUP_STEPS {
        return Err(format!("step {step} is past the last step {LAST_STEP}"));
    }
    let mut outcome = GroupStep {
        step,
        ..GroupStep::default()
    };
    match step {
        0 => setup(deps, home_company, ops, ip).await?,
        1..=12 => {
            let period = month(year, step);
            outcome.posted = usize::from(draft_and_post(deps, ops, &period).await?);
        }
        _ => {
            let period = month(year, step - 12);
            outcome.settled = usize::from(settle(deps, &period).await?);
        }
    }
    outcome.done = step == LAST_STEP;
    outcome.next = (!outcome.done).then_some(step + 1);
    if outcome.done {
        outcome.open_period = open_period(deps, &month(year, 12)).await?;
    }
    Ok(outcome)
}

/// Seeds the whole group demo in one go: every step of [`seed_step`] in order.
/// The native path (and the tests) use this; the Worker runs the steps one
/// request at a time.
///
/// # Errors
/// As [`seed_step`].
pub async fn seed(
    deps: &Deps,
    home_company: &str,
    ops: &str,
    ip: &str,
    year: i32,
) -> Result<GroupSeedReport, String> {
    let mut report = GroupSeedReport::default();
    for step in 0..GROUP_STEPS {
        let outcome = seed_step(deps, home_company, ops, ip, year, step).await?;
        report.posted += outcome.posted;
        report.settled += outcome.settled;
        if outcome.open_period.is_some() {
            report.open_period = outcome.open_period;
        }
    }
    Ok(report)
}

/// Step 0: the IP company with its book, the group and the licence.
async fn setup(deps: &Deps, home_company: &str, ops: &str, ip: &str) -> Result<(), String> {
    let db = deps.db.as_ref();
    dispatch::ensure_company(deps, ip, IP_COMPANY_NAME, "SGD").await?;
    dispatch::seed_company_with_journal(deps, ip, DEMO_OWNER_ID, &format!("general-{ip}")).await?;
    deps.db
        .execute(&cratefield_core::Statement::with_values(
            "INSERT INTO company_setting (company_id, key, value, updated_at) \
             SELECT ?, 'country_pack', 'sg', ? WHERE NOT EXISTS \
             (SELECT 1 FROM company_setting WHERE company_id = ? AND key = 'country_pack')"
                .to_owned(),
            vec![
                sea_query::Value::from(ip),
                sea_query::Value::from(dispatch::rfc3339_utc(deps.clock.now())),
                sea_query::Value::from(ip),
            ],
        ))
        .await
        .map_err(|err| err.to_string())?;
    grant_bootstrap_owner(deps, home_company, ip).await?;
    ensure_accounts(deps, ops).await?;
    ensure_accounts(deps, ip).await?;

    if store::group_by_id(db, GROUP_ID)
        .await
        .map_err(|err| err.to_string())?
        .is_none()
    {
        let input = GroupsCreateInput {
            id: GROUP_ID.to_owned(),
            name: GROUP_NAME.to_owned(),
            members: vec![
                MemberInput {
                    company_id: ops.to_owned(),
                    kind: GroupKind::Operating,
                    ownership_bp: 10_000,
                },
                MemberInput {
                    company_id: ip.to_owned(),
                    kind: GroupKind::IpHolding,
                    ownership_bp: 10_000,
                },
            ],
            created_by: DEMO_OWNER_ID.to_owned(),
        };
        run_as_person(deps, ops, &GroupsCreate, owner(), &input)
            .await
            .map_err(|err| format!("create the group: {err}"))?;
    }

    if store::agreement_sides(db, LICENCE_ID)
        .await
        .map_err(|err| err.to_string())?
        .is_empty()
    {
        let input = AgreementsCreateInput {
            id: LICENCE_ID.to_owned(),
            group_id: GROUP_ID.to_owned(),
            licensor_company_id: ip.to_owned(),
            licensee_company_id: ops.to_owned(),
            kind: Kind::Licence,
            basis: Basis::PctOfRevenueBp,
            rate_bp: Some(LICENCE_RATE_BP),
            fixed_minor: None,
            currency: "SGD".to_owned(),
            treaty_override_bp: None,
            notes: Some(
                "Demo licence: 5% of the operating company's revenue (invented)".to_owned(),
            ),
            licensor: AccountsInput {
                royalty_account: "4060".to_owned(),
                ic_account: "1080".to_owned(),
                wht_account: None,
                bank_account: "1020".to_owned(),
                unit: None,
            },
            licensee: AccountsInput {
                royalty_account: "5120".to_owned(),
                ic_account: "2060".to_owned(),
                wht_account: None,
                bank_account: "1020".to_owned(),
                unit: Some(OPERATING_UNIT.to_owned()),
            },
            created_by: DEMO_OWNER_ID.to_owned(),
        };
        run_as_person(deps, ops, &AgreementsCreate, owner(), &input)
            .await
            .map_err(|err| format!("create the licence: {err}"))?;
    }
    Ok(())
}

/// Drafts `period`, then posts it when it is still a draft (or failed).
/// Returns whether this call posted the period.
async fn draft_and_post(deps: &Deps, ops: &str, period: &str) -> Result<bool, String> {
    run_as_person(
        deps,
        ops,
        &AgreementsRunPeriod,
        owner(),
        &RunPeriodInput {
            id: LICENCE_ID.to_owned(),
            periods: vec![period.to_owned()],
        },
    )
    .await
    .map_err(|err| format!("draft {period}: {err}"))?;

    let db = deps.db.as_ref();
    // A period still in draft (or failed) posts; a posted one is left alone.
    let docs = store::docs_for_period(db, LICENCE_ID, period)
        .await
        .map_err(|err| err.to_string())?;
    let needs_post = docs.iter().any(|doc| {
        matches!(
            doc.view.status,
            ledgers_group::model::DocStatus::Draft | ledgers_group::model::DocStatus::Failed
        )
    });
    if !needs_post {
        return Ok(false);
    }
    let sides = DemoSides { deps };
    let call = deps.id_gen.ulid();
    let now = dispatch::rfc3339_utc(deps.clock.now());
    orchestrate::post_period(db, &sides, LICENCE_ID, period, &call, &now)
        .await
        .map_err(|err| format!("post {period}: {err}"))?;
    Ok(true)
}

/// Settles `period` when both sides are posted. Returns whether this call
/// settled it.
async fn settle(deps: &Deps, period: &str) -> Result<bool, String> {
    let db = deps.db.as_ref();
    let docs = store::docs_for_period(db, LICENCE_ID, period)
        .await
        .map_err(|err| err.to_string())?;
    if !docs
        .iter()
        .all(|doc| doc.view.status == ledgers_group::model::DocStatus::Posted)
    {
        return Ok(false);
    }
    let sides = DemoSides { deps };
    let call = deps.id_gen.ulid();
    let now = dispatch::rfc3339_utc(deps.clock.now());
    orchestrate::settle_period(db, &sides, LICENCE_ID, period, &call, &now)
        .await
        .map_err(|err| format!("settle {period}: {err}"))?;
    Ok(true)
}

/// The last month's period when it is posted and unsettled.
async fn open_period(deps: &Deps, period: &str) -> Result<Option<String>, String> {
    let docs = store::docs_for_period(deps.db.as_ref(), LICENCE_ID, period)
        .await
        .map_err(|err| err.to_string())?;
    Ok(docs
        .iter()
        .any(|doc| doc.view.status == ledgers_group::model::DocStatus::Posted)
        .then(|| period.to_owned()))
}

fn owner() -> OwnerAuth {
    OwnerAuth {
        person_id: DEMO_OWNER_ID.to_owned(),
    }
}
