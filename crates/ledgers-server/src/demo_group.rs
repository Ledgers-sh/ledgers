//! The Factory Zero group demo (issue #128): an IP holding company that
//! licenses the software to the operating company (the demo book from
//! `ledgers-demo`) at 5% of its revenue.
//!
//! The seed creates the IP company (Singapore, SGD, owned by the deployment's
//! bootstrap owner), the group, the licence, and then, for every month of the
//! demo year, drafts and posts the period and settles every month but the
//! last, so the reconciliation shows one open month. It runs through the same
//! commands and posting path as a person would use, as the demo owner.
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
async fn output_as_owner<C>(deps: &Deps, company_id: &str, command: &C, input: &C::Input) -> Result<C::Output, String>
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
        let out: PostEntryOutput = output_as_owner(self.deps, company_id, &PostEntry, input).await?;
        Ok(out.entry.entry_id)
    }

    async fn reverse(&self, company_id: &str, external_id: &str, posting_date: &str) -> Result<(), String> {
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
    let audit = AuditContext::new(
        Actor::Person {
            person_id: DEMO_OWNER_ID.to_owned(),
        },
        deps.id_gen.ulid(),
        deps.id_gen.ulid(),
    );
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
        create_account(db, company_id, draft, stamp.clone(), &audit)
            .await
            .map_err(|err| format!("add account {number}: {err}"))?;
    }
    Ok(())
}

/// Seeds the group demo for the operating company `ops` (the demo book's
/// company) and a new IP company, over `year`'s twelve months.
///
/// # Errors
/// A message naming the step that failed: the company, group, agreement,
/// a period's draft, post or settlement.
#[allow(clippy::too_many_lines)]
pub async fn seed(
    deps: &Deps,
    home_company: &str,
    ops: &str,
    ip: &str,
    year: i32,
) -> Result<GroupSeedReport, String> {
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
            notes: Some("Demo licence: 5% of the operating company's revenue (invented)".to_owned()),
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

    let months: Vec<String> = (1..=12).map(|m| format!("{year:04}-{m:02}")).collect();
    for period in &months {
        run_as_person(
            deps,
            ops,
            &AgreementsRunPeriod,
            owner(),
            &RunPeriodInput {
                id: LICENCE_ID.to_owned(),
                periods: vec![period.clone()],
            },
        )
        .await
        .map_err(|err| format!("draft {period}: {err}"))?;
    }

    let sides = DemoSides { deps };
    let call = deps.id_gen.ulid();
    let now = dispatch::rfc3339_utc(deps.clock.now());
    let mut report = GroupSeedReport::default();
    for period in &months {
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
        if needs_post {
            orchestrate::post_period(db, &sides, LICENCE_ID, period, &call, &now)
                .await
                .map_err(|err| format!("post {period}: {err}"))?;
            report.posted += 1;
        }
    }
    // Every month but the last settles: the last one stays open for the reconciliation.
    for period in months.iter().take(months.len() - 1) {
        let docs = store::docs_for_period(db, LICENCE_ID, period)
            .await
            .map_err(|err| err.to_string())?;
        if docs.iter().all(|doc| doc.view.status == ledgers_group::model::DocStatus::Posted) {
            orchestrate::settle_period(db, &sides, LICENCE_ID, period, &call, &now)
                .await
                .map_err(|err| format!("settle {period}: {err}"))?;
            report.settled += 1;
        }
    }
    if let Some(last) = months.last() {
        let docs = store::docs_for_period(db, LICENCE_ID, last)
            .await
            .map_err(|err| err.to_string())?;
        if docs.iter().any(|doc| doc.view.status == ledgers_group::model::DocStatus::Posted) {
            report.open_period = Some(last.clone());
        }
    }
    Ok(report)
}

fn owner() -> OwnerAuth {
    OwnerAuth {
        person_id: DEMO_OWNER_ID.to_owned(),
    }
}
