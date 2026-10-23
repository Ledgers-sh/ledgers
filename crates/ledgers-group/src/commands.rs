//! The group and agreement commands: create, list, read, update, terminate
//! and run a period's drafts. Posting and settling run through
//! [`crate::orchestrate`], because each side's entry goes through a different
//! company's executor.
//!
//! Every command is human-only, and the server checks the person's role in
//! every company a command touches before it runs (docs/permissions.md). The
//! views a command returns show one company's own side of an agreement, and a
//! counterparty only when the person may see that company.

use crate::model::{
    Accounts, AccountsInput, AgreementView, Basis, BasisInputs, DocStatus, GroupKind, Kind,
    MAX_PERIODS, NAME_MAX, PeriodAmounts, PeriodDocView, Side, WhtBasis, parse_period,
    period_amounts, withholding_for,
};
use crate::orchestrate::intercompany_id;
use crate::store::{self, AgreementRow, GroupRow, MemberRow};
use cratefield_core::{Database, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_ledger::account::AccountType;
use ledgers_ledger::commands::list_accounts;
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The most groups one list returns.
const MAX_LIST: usize = 200;

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(message.into())
}

/// One member as a create or update call names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberInput {
    /// The member company.
    pub company_id: String,
    /// What the member is in the group.
    pub kind: GroupKind,
    /// The ownership the group holds in the member, in basis points (`10000` = 100%).
    pub ownership_bp: i64,
}

/// One member as a read returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberView {
    /// The member company.
    pub company_id: String,
    /// The company's name.
    pub company_name: String,
    /// What the member is in the group.
    pub kind: GroupKind,
    /// The ownership in basis points.
    pub ownership_bp: i64,
}

/// One group as a read returns it. Members the person may not see are left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GroupView {
    /// The group id.
    pub id: String,
    /// The group name.
    pub name: String,
    /// The members the person may see.
    pub members: Vec<MemberView>,
    /// True when some member is not shown to the person.
    pub has_hidden_members: bool,
    /// Who created it.
    pub created_by: String,
    /// When created.
    pub created_at: String,
    /// When last changed.
    pub updated_at: String,
}

/// Input of `groups.create`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GroupsCreateInput {
    /// The group id; the server fills it in when empty.
    #[serde(default)]
    pub id: String,
    /// The group name.
    pub name: String,
    /// The members: at least two companies.
    pub members: Vec<MemberInput>,
    /// Who creates it; the server fills it in.
    #[serde(default)]
    pub created_by: String,
}

/// `groups.create`: a group of at least two companies. The person must own
/// every member (the server checks that before the command runs).
pub struct GroupsCreate;

/// Input of `groups.list`, which the server filters by what the person sees.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct GroupsListInput {}

/// Output of `groups.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GroupsListOutput {
    /// The groups the company belongs to.
    pub groups: Vec<GroupView>,
}

/// `groups.list`, with the companies the person may see.
pub struct GroupsList {
    /// The companies the person may see.
    pub accessible: Vec<String>,
}

/// Input of `groups.update`: a new name and, when given, the whole member list.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GroupsUpdateInput {
    /// The group id, from the path.
    pub id: String,
    /// A new name.
    pub name: Option<String>,
    /// The new member list, replacing the old one; at least two companies.
    pub members: Option<Vec<MemberInput>>,
}

/// `groups.update`.
pub struct GroupsUpdate {
    /// The companies the person may see, for the view it returns.
    pub accessible: Vec<String>,
}

/// Input of `agreements.create`. The server fills in `id` and `created_by`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AgreementsCreateInput {
    /// The agreement id; the server fills it in when empty.
    #[serde(default)]
    pub id: String,
    /// The group both companies belong to.
    pub group_id: String,
    /// The company that owns the rights and receives the income.
    pub licensor_company_id: String,
    /// The company that pays and books the expense.
    pub licensee_company_id: String,
    /// The kind of agreement.
    pub kind: Kind,
    /// How the period amount is worked out.
    pub basis: Basis,
    /// The rate in basis points, for the percentage bases.
    pub rate_bp: Option<i64>,
    /// The fixed monthly amount in minor units, for the fixed basis.
    pub fixed_minor: Option<i64>,
    /// The currency: the base currency of both companies.
    pub currency: String,
    /// A treaty rate in basis points that replaces the country pack's rate.
    pub treaty_override_bp: Option<i64>,
    /// Transfer pricing and reference notes.
    pub notes: Option<String>,
    /// The licensor's accounts.
    pub licensor: AccountsInput,
    /// The licensee's accounts.
    pub licensee: AccountsInput,
    /// Who creates it; the server fills it in.
    #[serde(default)]
    pub created_by: String,
}

/// Output of `agreements.create`: both sides, as their companies hold them.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AgreementsCreateOutput {
    /// The licensor's row.
    pub licensor: AgreementView,
    /// The licensee's row.
    pub licensee: AgreementView,
}

/// `agreements.create`.
pub struct AgreementsCreate;

/// Input of `agreements.list`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgreementsListInput {}

/// Output of `agreements.list`: the company's own side of each agreement.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AgreementsListOutput {
    /// The agreements.
    pub agreements: Vec<AgreementView>,
}

/// `agreements.list`, with the companies the person may see.
pub struct AgreementsList {
    /// The companies the person may see.
    pub accessible: Vec<String>,
}

/// Input of `agreements.get` and `agreements.terminate`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AgreementIdInput {
    /// The agreement id, from the path.
    pub id: String,
}

/// Output of `agreements.get`: the company's side and its period documents.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AgreementsGetOutput {
    /// The company's side of the agreement.
    pub agreement: AgreementView,
    /// The company's documents, one per period.
    pub periods: Vec<PeriodDocView>,
}

/// `agreements.get`.
pub struct AgreementsGet {
    /// The companies the person may see.
    pub accessible: Vec<String>,
}

/// Input of `agreements.update`: the terms and accounts, replacing the old.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AgreementsUpdateInput {
    /// The agreement id, from the path.
    pub id: String,
    /// The kind.
    pub kind: Kind,
    /// The basis.
    pub basis: Basis,
    /// The rate in basis points, for the percentage bases.
    pub rate_bp: Option<i64>,
    /// The fixed monthly amount, for the fixed basis.
    pub fixed_minor: Option<i64>,
    /// The currency.
    pub currency: String,
    /// The treaty rate in basis points, if any.
    pub treaty_override_bp: Option<i64>,
    /// The notes.
    pub notes: Option<String>,
    /// The licensor's accounts.
    pub licensor: AccountsInput,
    /// The licensee's accounts.
    pub licensee: AccountsInput,
}

/// `agreements.update`: refused for an agreement whose accounts a posted
/// period uses. The view it returns names the counterparty when the person may
/// see it.
pub struct AgreementsUpdate {
    /// The companies the person may see.
    pub accessible: Vec<String>,
}

/// `agreements.terminate`: no more periods run after this.
pub struct AgreementsTerminate {
    /// The companies the person may see.
    pub accessible: Vec<String>,
}

/// Input of `agreements.run_period`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RunPeriodInput {
    /// The agreement id, from the path.
    pub id: String,
    /// The periods to draft, each `YYYY-MM`.
    pub periods: Vec<String>,
}

/// Output of `agreements.post_period` and `agreements.settle_period`: the
/// documents of each period, licensor first.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PeriodsOutput {
    /// The agreement.
    pub agreement_id: String,
    /// One entry per period, in the order given.
    pub periods: Vec<PeriodPair>,
}

/// Output of `agreements.run_period`: the documents of each period, both sides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunPeriodOutput {
    /// The agreement.
    pub agreement_id: String,
    /// The documents per period, licensor first.
    pub periods: Vec<PeriodPair>,
}

/// One period's two documents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PeriodPair {
    /// The period, `YYYY-MM`.
    pub period: String,
    /// The licensor's and the licensee's documents.
    pub docs: Vec<PeriodDocView>,
}

/// `agreements.run_period`: drafts both sides of each period. A period that
/// already has drafts keeps them, so a second run changes nothing.
pub struct AgreementsRunPeriod;

/// Input of `agreements.post_period` and `agreements.settle_period`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PeriodsInput {
    /// The agreement id, from the path.
    pub id: String,
    /// The periods, each `YYYY-MM`, posted or settled in order.
    pub periods: Vec<String>,
}

/// Input of `reports.intercompany_reconciliation`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReconciliationInput {
    /// The group to reconcile, from the path.
    pub id: String,
}

/// Input of `reports.group_view`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GroupViewInput {
    /// The group, from the path.
    pub id: String,
    /// The period of the P&L, `YYYY-MM`.
    pub period: String,
    /// The as-of date of the balance sheet, `YYYY-MM-DD`.
    pub as_of: String,
}

// ---------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------

/// Checks a group's name and member list. Every member must be a distinct
/// company that exists, with a kind and an ownership in range.
async fn check_members(db: &dyn Database, members: &[MemberInput]) -> Result<(), CommandError> {
    if members.len() < 2 {
        return Err(invalid("a group needs at least two member companies"));
    }
    let mut seen = BTreeSet::new();
    for member in members {
        if !seen.insert(member.company_id.as_str()) {
            return Err(invalid(format!(
                "company {} is listed twice",
                member.company_id
            )));
        }
        if !(0..=10_000).contains(&member.ownership_bp) {
            return Err(invalid("ownership is in basis points, from 0 to 10000"));
        }
        if store::company_name(db, &member.company_id).await?.is_none() {
            return Err(invalid(format!(
                "company {} does not exist",
                member.company_id
            )));
        }
    }
    Ok(())
}

fn check_name(name: &str) -> Result<String, CommandError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(invalid(format!(
            "a group name is 1 to {NAME_MAX} characters"
        )));
    }
    Ok(name.to_owned())
}

fn member_statements(group_id: &str, members: &[MemberInput], now: &str) -> Vec<Statement> {
    members
        .iter()
        .map(|member| {
            Statement::with_values(
                "INSERT INTO company_group_member (company_id, group_id, kind, ownership_bp, created_at) \
                 VALUES (?, ?, ?, ?, ?)"
                    .to_owned(),
                vec![
                    Value::from(member.company_id.as_str()),
                    Value::from(group_id),
                    Value::from(member.kind.as_str()),
                    Value::from(member.ownership_bp),
                    Value::from(now),
                ],
            )
        })
        .collect()
}

/// The view of one group from `members` (the group's rows), the names of the
/// companies (read once by the caller) and the companies the person may see.
fn group_view(
    group: &GroupRow,
    members: &[MemberRow],
    names: &BTreeMap<String, String>,
    accessible: &[String],
) -> Result<GroupView, CommandError> {
    let mut visible = Vec::new();
    let mut hidden = false;
    for member in members {
        if !accessible.contains(&member.company_id) {
            hidden = true;
            continue;
        }
        let kind = GroupKind::parse(&member.kind).ok_or_else(|| {
            CommandError::InvalidInput(format!("unknown member kind {}", member.kind))
        })?;
        visible.push(MemberView {
            company_id: member.company_id.clone(),
            company_name: names.get(&member.company_id).cloned().unwrap_or_default(),
            kind,
            ownership_bp: member.ownership_bp,
        });
    }
    Ok(GroupView {
        id: group.id.clone(),
        name: group.name.clone(),
        members: visible,
        has_hidden_members: hidden,
        created_by: group.created_by.clone(),
        created_at: group.created_at.clone(),
        updated_at: group.updated_at.clone(),
    })
}

#[async_trait::async_trait]
impl Command for GroupsCreate {
    const NAME: &'static str = "groups.create";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = GroupsCreateInput;
    type Output = GroupView;

    async fn preview(
        &self,
        db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        check_name(&input.name)?;
        check_members(db, &input.members).await?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let name = check_name(&input.name)?;
        check_members(db, &input.members).await?;
        if input.id.is_empty() {
            return Err(invalid("the group needs an id"));
        }
        let mut statements = vec![Statement::with_values(
            "INSERT INTO company_group (id, name, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?)"
                .to_owned(),
            vec![
                Value::from(input.id.as_str()),
                Value::from(name.as_str()),
                Value::from(input.created_by.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.now.as_str()),
            ],
        )];
        statements.extend(member_statements(&input.id, &input.members, &ctx.now));
        let group = GroupRow {
            id: input.id.clone(),
            name,
            created_by: input.created_by.clone(),
            created_at: ctx.now.clone(),
            updated_at: ctx.now.clone(),
        };
        let members: Vec<MemberRow> = input
            .members
            .iter()
            .map(|m| MemberRow {
                group_id: input.id.clone(),
                company_id: m.company_id.clone(),
                kind: m.kind.as_str().to_owned(),
                ownership_bp: m.ownership_bp,
            })
            .collect();
        let accessible: Vec<String> = input.members.iter().map(|m| m.company_id.clone()).collect();
        let ids: Vec<String> = accessible.clone();
        let names = store::company_names(db, &ids).await?;
        let view = group_view(&group, &members, &names, &accessible)?;
        Ok(Plan {
            statements,
            output: view,
            objects: vec![input.id.clone()],
        })
    }
}

#[async_trait::async_trait]
impl Command for GroupsList {
    const NAME: &'static str = "groups.list";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = GroupsListInput;
    type Output = GroupsListOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let groups = store::groups_of_company(db, &ctx.company_id).await?;
        let ids: Vec<String> = groups.iter().map(|g| g.id.clone()).collect();
        let members = store::members_of_groups(db, &ids).await?;
        let companies: Vec<String> = members
            .iter()
            .map(|m| m.company_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let names = store::company_names(db, &companies).await?;
        let mut views = Vec::new();
        for group in groups.iter().take(MAX_LIST) {
            let mine: Vec<MemberRow> = members
                .iter()
                .filter(|m| m.group_id == group.id)
                .cloned()
                .collect();
            views.push(group_view(group, &mine, &names, &self.accessible)?);
        }
        Ok(Plan::with_output(GroupsListOutput { groups: views }))
    }
}

#[async_trait::async_trait]
impl Command for GroupsUpdate {
    const NAME: &'static str = "groups.update";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = GroupsUpdateInput;
    type Output = GroupView;

    async fn preview(
        &self,
        db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        if let Some(name) = &input.name {
            check_name(name)?;
        }
        if let Some(members) = &input.members {
            check_members(db, members).await?;
        }
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let group = store::group_by_id(db, &input.id)
            .await?
            .ok_or_else(|| CommandError::Domain(cratefield_core::Problem::not_found()))?;
        let mut statements = Vec::new();
        let name = match &input.name {
            Some(name) => check_name(name)?,
            None => group.name.clone(),
        };
        if let Some(members) = &input.members {
            check_members(db, members).await?;
            statements.push(Statement::with_values(
                "DELETE FROM company_group_member WHERE group_id = ?".to_owned(),
                vec![Value::from(input.id.as_str())],
            ));
            statements.extend(member_statements(&input.id, members, &ctx.now));
        }
        statements.push(Statement::with_values(
            "UPDATE company_group SET name = ?, updated_at = ? WHERE id = ?".to_owned(),
            vec![
                Value::from(name.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(input.id.as_str()),
            ],
        ));
        let members = match &input.members {
            Some(list) => list
                .iter()
                .map(|m| MemberRow {
                    group_id: input.id.clone(),
                    company_id: m.company_id.clone(),
                    kind: m.kind.as_str().to_owned(),
                    ownership_bp: m.ownership_bp,
                })
                .collect(),
            None => store::members_of(db, &input.id).await?,
        };
        let updated = GroupRow {
            name,
            updated_at: ctx.now.clone(),
            ..group
        };
        let companies: Vec<String> = members.iter().map(|m| m.company_id.clone()).collect();
        let names = store::company_names(db, &companies).await?;
        let view = group_view(&updated, &members, &names, &self.accessible)?;
        Ok(Plan {
            statements,
            output: view,
            objects: vec![input.id.clone()],
        })
    }
}

// ---------------------------------------------------------------------------
// Agreements: terms, accounts and the withholding rate
// ---------------------------------------------------------------------------

/// The terms an agreement's two sides share.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Terms {
    kind: Kind,
    basis: Basis,
    rate_bp: Option<i64>,
    fixed_minor: Option<i64>,
    currency: String,
    treaty_override_bp: Option<i64>,
}

/// The checked terms and accounts of an agreement, ready to write.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Prepared {
    terms: Terms,
    wht_bp: i64,
    wht_basis: WhtBasis,
    licensor: Accounts,
    licensee: Accounts,
}

fn check_terms(terms: &Terms) -> Result<(), CommandError> {
    match terms.basis {
        Basis::PctOfRevenueBp | Basis::CostPlusBp => {
            let rate = terms
                .rate_bp
                .ok_or_else(|| invalid("this basis needs a rate in basis points"))?;
            if !(0..=100_000).contains(&rate) {
                return Err(invalid("a rate is from 0 to 100000 basis points"));
            }
            if terms.fixed_minor.is_some() {
                return Err(invalid("a percentage basis takes no fixed amount"));
            }
        }
        Basis::FixedMonthly => {
            let amount = terms
                .fixed_minor
                .ok_or_else(|| invalid("the fixed basis needs a monthly amount"))?;
            if amount < 0 {
                return Err(invalid("a monthly amount is not negative"));
            }
            if terms.rate_bp.is_some() {
                return Err(invalid("the fixed basis takes no rate"));
            }
        }
    }
    if let Some(bp) = terms.treaty_override_bp
        && !(0..=10_000).contains(&bp)
    {
        return Err(invalid("a treaty rate is from 0 to 10000 basis points"));
    }
    if terms.currency.len() != 3 || !terms.currency.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(invalid("the currency is a three-letter ISO code"));
    }
    Ok(())
}

/// One account of a side, checked against the chart: it exists, is live and
/// postable, and has the type the side needs.
async fn check_account(
    db: &dyn Database,
    company_id: &str,
    number: &str,
    expected: AccountType,
    role: &str,
) -> Result<(), CommandError> {
    let entries = list_accounts(db, company_id, true)
        .await
        .map_err(|err| invalid(err.to_string()))?;
    let entry = entries
        .iter()
        .find(|entry| entry.account.number == number)
        .ok_or_else(|| invalid(format!("account {number} does not exist for the {role}")))?;
    if entry.account.archived || !entry.is_postable() {
        return Err(invalid(format!(
            "account {number} is not a live, postable account for the {role}"
        )));
    }
    if entry.account.account_type != expected {
        return Err(invalid(format!(
            "account {number} for the {role} must be a {} account",
            expected_text(expected)
        )));
    }
    Ok(())
}

fn expected_text(kind: AccountType) -> &'static str {
    match kind {
        AccountType::Asset => "asset",
        AccountType::Liability => "liability",
        AccountType::Equity => "equity",
        AccountType::Income => "income",
        AccountType::Expense => "expense",
    }
}

/// The business unit a royalty line carries. A company with business units
/// names one of its active units, since its P&L lines must carry a unit.
async fn check_royalty_unit(
    db: &dyn Database,
    company_id: &str,
    role: &str,
    unit: Option<&str>,
) -> Result<(), CommandError> {
    let enabled = ledgers_ledger::units::units_enabled(db, company_id)
        .await
        .map_err(|err| invalid(err.to_string()))?;
    match unit {
        None if enabled => Err(invalid(format!(
            "the {role} has business units: name the unit its royalty line posts under"
        ))),
        None => Ok(()),
        Some(code) => {
            let found = ledgers_ledger::units::find_unit(db, company_id, code)
                .await
                .map_err(|err| invalid(err.to_string()))?;
            match found {
                Some(unit) if !unit.archived => Ok(()),
                _ => Err(invalid(format!(
                    "business unit {code} is not an active unit of the {role}"
                ))),
            }
        }
    }
}

async fn check_side_accounts(
    db: &dyn Database,
    company_id: &str,
    side: Side,
    accounts: &AccountsInput,
) -> Result<(), CommandError> {
    let (royalty, ic, bank, role) = match side {
        Side::Licensor => (
            AccountType::Income,
            AccountType::Asset,
            AccountType::Asset,
            "licensor",
        ),
        Side::Licensee => (
            AccountType::Expense,
            AccountType::Liability,
            AccountType::Asset,
            "licensee",
        ),
    };
    check_account(db, company_id, &accounts.royalty_account, royalty, role).await?;
    check_account(db, company_id, &accounts.ic_account, ic, role).await?;
    check_account(db, company_id, &accounts.bank_account, bank, role).await?;
    check_royalty_unit(db, company_id, role, accounts.unit.as_deref()).await?;
    if let Some(number) = &accounts.wht_account {
        let wht = match side {
            Side::Licensor => AccountType::Asset,
            Side::Licensee => AccountType::Liability,
        };
        check_account(db, company_id, number, wht, role).await?;
    }
    Ok(())
}

/// The base currency of both companies must be the agreement currency: FX
/// gain and loss for intercompany accruals is not built, so other currencies
/// are refused rather than converted.
async fn check_currency(
    db: &dyn Database,
    licensor: &str,
    licensee: &str,
    currency: &str,
) -> Result<(), CommandError> {
    for company in [licensor, licensee] {
        let base = store::base_currency_of(db, company)
            .await?
            .ok_or_else(|| invalid(format!("company {company} has no base currency")))?;
        if base != currency {
            return Err(invalid(format!(
                "the agreement is in {currency} but company {company} keeps its books in {base}: \
                 intercompany agreements need one currency, and FX gain and loss on intercompany \
                 balances is not built yet (a TODO)"
            )));
        }
    }
    Ok(())
}

/// Checks the whole agreement and works out its withholding.
async fn prepare(
    db: &dyn Database,
    licensor: &str,
    licensee: &str,
    group_id: &str,
    terms: Terms,
    accounts: (&AccountsInput, &AccountsInput),
) -> Result<Prepared, CommandError> {
    if licensor == licensee {
        return Err(invalid("an agreement needs two different companies"));
    }
    check_terms(&terms)?;
    let members = store::members_of(db, group_id).await?;
    for company in [licensor, licensee] {
        if !members.iter().any(|m| m.company_id == company) {
            return Err(invalid(format!(
                "company {company} is not a member of the group"
            )));
        }
    }
    check_currency(db, licensor, licensee, &terms.currency).await?;
    check_side_accounts(db, licensor, Side::Licensor, accounts.0).await?;
    check_side_accounts(db, licensee, Side::Licensee, accounts.1).await?;

    let licensee_pack = store::country_pack_of(db, licensee)
        .await?
        .and_then(|id| ledgers_tax::pack_by_id(&id).ok());
    let licensor_pack = store::country_pack_of(db, licensor)
        .await?
        .and_then(|id| ledgers_tax::pack_by_id(&id).ok());
    let (wht_bp, wht_basis) = withholding_for(
        terms.kind,
        licensee_pack.as_ref(),
        licensor_pack.as_ref(),
        terms.treaty_override_bp,
    );
    if wht_bp > 0 && (accounts.0.wht_account.is_none() || accounts.1.wht_account.is_none()) {
        return Err(invalid(
            "the agreement withholds tax, so both sides name a withholding account",
        ));
    }
    Ok(Prepared {
        terms,
        wht_bp,
        wht_basis,
        licensor: accounts_of(accounts.0),
        licensee: accounts_of(accounts.1),
    })
}

fn accounts_of(input: &AccountsInput) -> Accounts {
    Accounts {
        royalty: input.royalty_account.clone(),
        ic: input.ic_account.clone(),
        wht: input.wht_account.clone(),
        bank: input.bank_account.clone(),
        unit: input.unit.clone(),
    }
}

/// One side's row of an agreement, as the write and the reads both build it.
fn side_view(
    id: &str,
    group_id: &str,
    side: Side,
    counterparty: &str,
    prepared: &Prepared,
    created_by: &str,
    now: &str,
) -> AgreementView {
    AgreementView {
        id: id.to_owned(),
        group_id: group_id.to_owned(),
        side,
        counterparty_id: Some(counterparty.to_owned()),
        counterparty_hidden: false,
        kind: prepared.terms.kind,
        basis: prepared.terms.basis,
        rate_bp: prepared.terms.rate_bp,
        fixed_minor: prepared.terms.fixed_minor,
        currency: prepared.terms.currency.clone(),
        wht_bp: prepared.wht_bp,
        wht_basis: prepared.wht_basis,
        treaty_override_bp: prepared.terms.treaty_override_bp,
        accounts: match side {
            Side::Licensor => prepared.licensor.clone(),
            Side::Licensee => prepared.licensee.clone(),
        },
        notes: None,
        terminated_at: None,
        created_by: created_by.to_owned(),
        created_at: now.to_owned(),
        updated_at: now.to_owned(),
    }
}

fn insert_agreement(
    company: &str,
    id: &str,
    group_id: &str,
    side: Side,
    counterparty: &str,
    view: &AgreementView,
    notes: Option<&str>,
) -> Statement {
    let accounts = &view.accounts;
    Statement::with_values(
        "INSERT INTO intercompany_agreement (company_id, id, group_id, side, counterparty_id, kind, basis, rate_bp, \
         fixed_minor, currency, wht_bp, wht_basis, treaty_override_bp, account_royalty, account_ic, account_wht, \
         account_bank, account_unit, notes, terminated_at, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)"
            .to_owned(),
        vec![
            Value::from(company),
            Value::from(id),
            Value::from(group_id),
            Value::from(side.as_str()),
            Value::from(counterparty),
            Value::from(view.kind.as_str()),
            Value::from(view.basis.as_str()),
            view.rate_bp.map_or(Value::BigInt(None), Value::from),
            view.fixed_minor.map_or(Value::BigInt(None), Value::from),
            Value::from(view.currency.as_str()),
            Value::from(view.wht_bp),
            Value::from(WhtBasis::as_str(view.wht_basis)),
            view.treaty_override_bp.map_or(Value::BigInt(None), Value::from),
            Value::from(accounts.royalty.as_str()),
            Value::from(accounts.ic.as_str()),
            accounts.wht.as_deref().map_or(Value::String(None), Value::from),
            Value::from(accounts.bank.as_str()),
            accounts.unit.as_deref().map_or(Value::String(None), Value::from),
            notes.map_or(Value::String(None), Value::from),
            Value::from(view.created_by.as_str()),
            Value::from(view.created_at.as_str()),
            Value::from(view.updated_at.as_str()),
        ],
    )
}

/// A stored agreement row, as the company that holds it sees it. The other
/// company is named only when the person may see it.
fn visible(row: &AgreementRow, accessible: &[String]) -> AgreementView {
    let mut view = row.view.clone();
    if !accessible.contains(&row.counterparty_id) {
        view.counterparty_id = None;
        view.counterparty_hidden = true;
    }
    view
}

fn amounts_for(view: &AgreementView, inputs: BasisInputs) -> Result<PeriodAmounts, CommandError> {
    period_amounts(
        view.basis,
        view.rate_bp,
        view.fixed_minor,
        view.wht_bp,
        inputs,
    )
    .ok_or_else(|| invalid("a period amount does not fit the ledger"))
}

async fn agreement_pair(
    db: &dyn Database,
    agreement_id: &str,
) -> Result<(AgreementRow, AgreementRow), CommandError> {
    let rows = store::agreement_sides(db, agreement_id).await?;
    let mut licensor = None;
    let mut licensee = None;
    for row in rows {
        match row.view.side {
            Side::Licensor => licensor = Some(row),
            Side::Licensee => licensee = Some(row),
        }
    }
    licensor
        .zip(licensee)
        .ok_or_else(|| CommandError::Domain(cratefield_core::Problem::not_found()))
}

fn refuse_if_terminated(row: &AgreementRow) -> Result<(), CommandError> {
    if row.view.terminated_at.is_some() {
        return Err(invalid(
            "the agreement is terminated and runs no more periods",
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl Command for AgreementsCreate {
    const NAME: &'static str = "agreements.create";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = AgreementsCreateInput;
    type Output = AgreementsCreateOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        prepare_create(db, input).await?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        if input.id.is_empty() {
            return Err(invalid("the agreement needs an id"));
        }
        let prepared = prepare_create(db, input).await?;
        let licensor_view = side_view(
            &input.id,
            &input.group_id,
            Side::Licensor,
            &input.licensee_company_id,
            &prepared,
            &input.created_by,
            &ctx.now,
        );
        let licensee_view = side_view(
            &input.id,
            &input.group_id,
            Side::Licensee,
            &input.licensor_company_id,
            &prepared,
            &input.created_by,
            &ctx.now,
        );
        let statements = vec![
            insert_agreement(
                &input.licensor_company_id,
                &input.id,
                &input.group_id,
                Side::Licensor,
                &input.licensee_company_id,
                &licensor_view,
                input.notes.as_deref(),
            ),
            insert_agreement(
                &input.licensee_company_id,
                &input.id,
                &input.group_id,
                Side::Licensee,
                &input.licensor_company_id,
                &licensee_view,
                input.notes.as_deref(),
            ),
        ];
        Ok(Plan {
            statements,
            output: AgreementsCreateOutput {
                licensor: AgreementView {
                    notes: input.notes.clone(),
                    ..licensor_view
                },
                licensee: AgreementView {
                    notes: input.notes.clone(),
                    ..licensee_view
                },
            },
            objects: vec![input.id.clone()],
        })
    }
}

async fn prepare_create(
    db: &dyn Database,
    input: &AgreementsCreateInput,
) -> Result<Prepared, CommandError> {
    let terms = Terms {
        kind: input.kind,
        basis: input.basis,
        rate_bp: input.rate_bp,
        fixed_minor: input.fixed_minor,
        currency: input.currency.clone(),
        treaty_override_bp: input.treaty_override_bp,
    };
    prepare(
        db,
        &input.licensor_company_id,
        &input.licensee_company_id,
        &input.group_id,
        terms,
        (&input.licensor, &input.licensee),
    )
    .await
}

#[async_trait::async_trait]
impl Command for AgreementsList {
    const NAME: &'static str = "agreements.list";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = AgreementsListInput;
    type Output = AgreementsListOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let rows = store::agreements_of_company(db, &ctx.company_id).await?;
        let agreements = rows
            .iter()
            .take(MAX_LIST)
            .map(|row| visible(row, &self.accessible))
            .collect();
        Ok(Plan::with_output(AgreementsListOutput { agreements }))
    }
}

#[async_trait::async_trait]
impl Command for AgreementsGet {
    const NAME: &'static str = "agreements.get";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = AgreementIdInput;
    type Output = AgreementsGetOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let (licensor, licensee) = agreement_pair(db, &input.id).await?;
        let own = [&licensor, &licensee]
            .into_iter()
            .find(|row| row.company_id == ctx.company_id)
            .ok_or_else(|| CommandError::Domain(cratefield_core::Problem::not_found()))?;
        let periods = store::docs_of_agreement(db, &input.id)
            .await?
            .iter()
            .filter(|doc| doc.company_id == ctx.company_id)
            .map(|doc| doc.view.clone())
            .collect();
        Ok(Plan::with_output(AgreementsGetOutput {
            agreement: visible(own, &self.accessible),
            periods,
        }))
    }
}

#[async_trait::async_trait]
impl Command for AgreementsUpdate {
    const NAME: &'static str = "agreements.update";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = AgreementsUpdateInput;
    type Output = AgreementView;

    async fn preview(
        &self,
        db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        update_prepared(db, input).await?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let (prepared, licensor, licensee) = update_prepared(db, input).await?;
        let mut statements = Vec::new();
        for (row, accounts) in [
            (&licensor, &prepared.licensor),
            (&licensee, &prepared.licensee),
        ] {
            statements.push(Statement::with_values(
                "UPDATE intercompany_agreement SET kind = ?, basis = ?, rate_bp = ?, fixed_minor = ?, currency = ?, \
                 wht_bp = ?, wht_basis = ?, treaty_override_bp = ?, account_royalty = ?, account_ic = ?, \
                 account_wht = ?, account_bank = ?, account_unit = ?, notes = ?, updated_at = ? \
                 WHERE company_id = ? AND id = ? AND terminated_at IS NULL"
                    .to_owned(),
                vec![
                    Value::from(input.kind.as_str()),
                    Value::from(input.basis.as_str()),
                    input.rate_bp.map_or(Value::BigInt(None), Value::from),
                    input.fixed_minor.map_or(Value::BigInt(None), Value::from),
                    Value::from(input.currency.as_str()),
                    Value::from(prepared.wht_bp),
                    Value::from(WhtBasis::as_str(prepared.wht_basis)),
                    input.treaty_override_bp.map_or(Value::BigInt(None), Value::from),
                    Value::from(accounts.royalty.as_str()),
                    Value::from(accounts.ic.as_str()),
                    accounts.wht.as_deref().map_or(Value::String(None), Value::from),
                    Value::from(accounts.bank.as_str()),
                    accounts.unit.as_deref().map_or(Value::String(None), Value::from),
                    input.notes.as_deref().map_or(Value::String(None), Value::from),
                    Value::from(ctx.now.as_str()),
                    Value::from(row.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ));
        }
        let own = if licensor.company_id == ctx.company_id {
            &licensor
        } else {
            &licensee
        };
        let refreshed = AgreementRow {
            view: AgreementView {
                kind: input.kind,
                basis: input.basis,
                rate_bp: input.rate_bp,
                fixed_minor: input.fixed_minor,
                currency: input.currency.clone(),
                wht_bp: prepared.wht_bp,
                wht_basis: prepared.wht_basis,
                treaty_override_bp: input.treaty_override_bp,
                accounts: if own.view.side == Side::Licensor {
                    prepared.licensor.clone()
                } else {
                    prepared.licensee.clone()
                },
                notes: input.notes.clone(),
                updated_at: ctx.now.clone(),
                ..own.view.clone()
            },
            ..own.clone()
        };
        Ok(Plan {
            statements,
            output: visible(&refreshed, &self.accessible),
            objects: vec![input.id.clone()],
        })
    }
}

/// Checks an update: the agreement exists, is not terminated, and its
/// accounts do not change once a period is posted. Returns the checked terms
/// with both current rows.
async fn update_prepared(
    db: &dyn Database,
    input: &AgreementsUpdateInput,
) -> Result<(Prepared, AgreementRow, AgreementRow), CommandError> {
    let (licensor, licensee) = agreement_pair(db, &input.id).await?;
    refuse_if_terminated(&licensor)?;
    let terms = Terms {
        kind: input.kind,
        basis: input.basis,
        rate_bp: input.rate_bp,
        fixed_minor: input.fixed_minor,
        currency: input.currency.clone(),
        treaty_override_bp: input.treaty_override_bp,
    };
    let prepared = prepare(
        db,
        &licensor.company_id,
        &licensee.company_id,
        &licensor.group_id,
        terms,
        (&input.licensor, &input.licensee),
    )
    .await?;
    let posted = store::docs_of_agreement(db, &input.id)
        .await?
        .iter()
        .any(|doc| matches!(doc.view.status, DocStatus::Posted | DocStatus::Settled));
    let changed = accounts_of(&input.licensor) != licensor.view.accounts
        || accounts_of(&input.licensee) != licensee.view.accounts;
    if posted && changed {
        return Err(invalid(
            "a period is already posted: its accounts stay as they were, and settlement reads them",
        ));
    }
    Ok((prepared, licensor, licensee))
}

#[async_trait::async_trait]
impl Command for AgreementsTerminate {
    const NAME: &'static str = "agreements.terminate";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = AgreementIdInput;
    type Output = AgreementView;

    async fn preview(
        &self,
        db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        agreement_pair(db, &input.id).await?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let (licensor, licensee) = agreement_pair(db, &input.id).await?;
        let own = if licensor.company_id == ctx.company_id {
            licensor
        } else {
            licensee
        };
        let statements = vec![Statement::with_values(
            "UPDATE intercompany_agreement SET terminated_at = COALESCE(terminated_at, ?), updated_at = ? \
             WHERE id = ?"
                .to_owned(),
            vec![
                Value::from(ctx.now.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(input.id.as_str()),
            ],
        )];
        let mut view = visible(&own, &self.accessible);
        view.terminated_at = Some(view.terminated_at.unwrap_or_else(|| ctx.now.clone()));
        view.updated_at.clone_from(&ctx.now);
        Ok(Plan {
            statements,
            output: view,
            objects: vec![input.id.clone()],
        })
    }
}

/// Checks a list of periods: at least one, at most the limit, each `YYYY-MM`,
/// no repeats; returned in order.
///
/// # Errors
/// [`CommandError::InvalidInput`] for an empty or too long list, a period
/// that is not `YYYY-MM`, or a period given twice.
pub fn check_periods(periods: &[String]) -> Result<Vec<String>, CommandError> {
    if periods.is_empty() || periods.len() > MAX_PERIODS {
        return Err(invalid(format!("give from 1 to {MAX_PERIODS} periods")));
    }
    let mut seen = BTreeSet::new();
    for period in periods {
        if parse_period(period).is_none() {
            return Err(invalid(format!("{period:?} is not a YYYY-MM period")));
        }
        if !seen.insert(period.as_str()) {
            return Err(invalid(format!("period {period} is listed twice")));
        }
    }
    Ok(periods.to_vec())
}

#[async_trait::async_trait]
impl Command for AgreementsRunPeriod {
    const NAME: &'static str = "agreements.run_period";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = true;
    const SUPPORTS_DRY_RUN: bool = true;

    type Input = RunPeriodInput;
    type Output = RunPeriodOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        plan_run(db, ctx, input).await?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        plan_run(db, ctx, input).await
    }
}

/// Drafts both sides of each period from the licensee's revenue (basis
/// `pct_of_revenue_bp`) or the licensor's expenses (basis `cost_plus_bp`), in
/// the agreement currency. A period that already has drafts keeps them.
async fn plan_run(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &RunPeriodInput,
) -> Result<Plan<RunPeriodOutput>, CommandError> {
    let periods = check_periods(&input.periods)?;
    let (licensor, licensee) = agreement_pair(db, &input.id).await?;
    if ctx.company_id != licensor.company_id && ctx.company_id != licensee.company_id {
        return Err(CommandError::Domain(cratefield_core::Problem::not_found()));
    }
    refuse_if_terminated(&licensor)?;
    let mut statements = Vec::new();
    let mut out = Vec::new();
    for period in &periods {
        let existing = store::docs_for_period(db, &input.id, period).await?;
        if existing.len() == 2 {
            let mut docs: Vec<PeriodDocView> =
                existing.iter().map(|doc| doc.view.clone()).collect();
            docs.sort_by_key(|doc| doc.side != Side::Licensor);
            out.push(PeriodPair {
                period: period.clone(),
                docs,
            });
            continue;
        }
        let revenue = ledgers_ledger::report::profit_and_loss(db, &licensee.company_id, period)
            .await
            .map_err(|err| invalid(err.to_string()))?
            .revenue
            .minor();
        let expenses = ledgers_ledger::report::profit_and_loss(db, &licensor.company_id, period)
            .await
            .map_err(|err| invalid(err.to_string()))?
            .expenses
            .minor();
        let inputs = BasisInputs {
            revenue_minor: i64::try_from(revenue)
                .map_err(|_| invalid("the revenue does not fit"))?,
            expenses_minor: i64::try_from(expenses)
                .map_err(|_| invalid("the expenses do not fit"))?,
        };
        let amounts = amounts_for(&licensor.view, inputs)?;
        let intercompany = intercompany_id(&input.id, period);
        let mut docs = Vec::new();
        for row in [&licensor, &licensee] {
            statements.push(insert_doc(
                &row.company_id,
                &intercompany,
                row.view.side,
                &input.id,
                period,
                &row.view.currency,
                &amounts,
                &ctx.now,
            ));
            docs.push(PeriodDocView {
                id: intercompany.clone(),
                side: row.view.side,
                period: period.clone(),
                status: DocStatus::Draft,
                currency: row.view.currency.clone(),
                basis_minor: amounts.basis_minor,
                gross_minor: amounts.gross_minor,
                wht_minor: amounts.wht_minor,
                net_minor: amounts.net_minor,
                accrual_entry_id: None,
                settle_entry_id: None,
                last_error: None,
            });
        }
        out.push(PeriodPair {
            period: period.clone(),
            docs,
        });
    }
    Ok(Plan {
        statements,
        output: RunPeriodOutput {
            agreement_id: input.id.clone(),
            periods: out,
        },
        objects: vec![input.id.clone()],
    })
}

/// One draft, guarded so a second run with the same key adds nothing.
#[allow(clippy::too_many_arguments)]
fn insert_doc(
    company: &str,
    intercompany: &str,
    side: Side,
    agreement_id: &str,
    period: &str,
    currency: &str,
    amounts: &PeriodAmounts,
    now: &str,
) -> Statement {
    Statement::with_values(
        "INSERT INTO intercompany_document (company_id, id, side, agreement_id, period, status, currency, \
         basis_minor, gross_minor, wht_minor, net_minor, attempt, accrual_entry_id, settle_entry_id, last_error, \
         created_at, updated_at) SELECT ?, ?, ?, ?, ?, 'draft', ?, ?, ?, ?, ?, 0, NULL, NULL, NULL, ?, ? \
         WHERE NOT EXISTS (SELECT 1 FROM intercompany_document WHERE company_id = ? AND id = ? AND side = ?)"
            .to_owned(),
        vec![
            Value::from(company),
            Value::from(intercompany),
            Value::from(side.as_str()),
            Value::from(agreement_id),
            Value::from(period),
            Value::from(currency),
            Value::from(amounts.basis_minor),
            Value::from(amounts.gross_minor),
            Value::from(amounts.wht_minor),
            Value::from(amounts.net_minor),
            Value::from(now),
            Value::from(now),
            Value::from(company),
            Value::from(intercompany),
            Value::from(side.as_str()),
        ],
    )
}
