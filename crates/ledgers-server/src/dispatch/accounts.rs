//! The chart-of-accounts commands: `accounts.create`, `accounts.update`,
//! `accounts.archive` and `accounts.restore` (writes) and `accounts.get`
//! (one account with its balance and recent activity).
//!
//! The writes run through the ledgers-ledger chart functions, each of which
//! writes its change, its guard statements and its audit event in one batch
//! (docs/PLAN.md section 3, rule 6). This module authenticates the caller
//! first: a person is always allowed (their owner or admin role is checked
//! at sign-in); an agent key is allowed only when its policy sets
//! `chart_changes` (see `ledgers_policy::policy::CHART_WRITE_COMMANDS`).
//! A refused agent gets a 403 and one `denied` audit event, as any policy
//! refusal does.

use super::{
    AccountView, Deps, KeyStore, Presented, ServerDispatch, domain, load_key, rfc3339_utc,
};
use crate::people::SessionAuthenticator;
use async_trait::async_trait;
use cratefield_core::{Database, DbError, Problem};
use ledgers_audit::{AuditConfig, AuditContext, AuditRecord, Outcome};
use ledgers_commands::auth::{Authenticator, Principal, Tier};
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_ledger::account::{Account, AccountDraft, AccountEntry, AccountType};
use ledgers_ledger::commands::{self as chart, AccountChanges};
use ledgers_ledger::query::{self as ledger_query, AccountActivity};
use ledgers_ledger::surface::SerializableAmount as Amount;
use ledgers_money::Currency;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// The input of `accounts.create`. `type` is the account class.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountsCreateInput {
    /// The account number: 2 to 10 ASCII digits, unique in the chart.
    pub number: String,
    /// The name shown for the account (a custom name).
    pub name: String,
    /// The account class: asset, liability, equity, income or expense.
    #[serde(rename = "type")]
    pub account_type: String,
    /// The parent account number; absent means the root of the chart.
    #[serde(default)]
    pub parent: Option<String>,
    /// Kebab-case tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// The account's currency code; absent means the company base currency.
    #[serde(default)]
    pub currency: Option<String>,
}

/// The input of `accounts.update`. `number` comes from the path. Only the
/// fields present change; `clear_parent` moves the account to the root.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AccountsUpdateInput {
    /// The account number (from the path).
    #[serde(default)]
    pub number: String,
    /// A new name.
    #[serde(default)]
    pub name: Option<String>,
    /// A new class; refused once the account has postings or children.
    #[serde(rename = "type", default)]
    pub account_type: Option<String>,
    /// A new tag list, replacing the old one.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// A new parent account number.
    #[serde(default)]
    pub parent: Option<String>,
    /// Move the account to the root of the chart.
    #[serde(default)]
    pub clear_parent: bool,
}

/// The input of `accounts.archive`, `accounts.restore` and `accounts.get`:
/// the account number (from the path).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AccountsNumberInput {
    /// The account number.
    #[serde(default)]
    pub number: String,
}

/// The output of the four writes: the account after the change.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AccountsChangedOutput {
    /// The account, with its balance.
    pub account: AccountView,
}

/// One recent entry of an account, as the account page lists it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AccountActivityView {
    /// The entry's id: the journal entry to open.
    pub entry_id: String,
    /// The journal the entry posted into.
    pub journal_id: String,
    /// The fiscal year the entry number is scoped to.
    pub fiscal_year: i64,
    /// The gapless entry number.
    pub number: i64,
    /// The accounting date (YYYY-MM-DD).
    pub posting_date: String,
    /// The entry's description.
    pub description: String,
    /// The account's signed base amount in the entry (debit positive).
    pub minor: String,
    /// The base currency code of the amount.
    pub currency: String,
}

/// The output of `accounts.get`: the account and its recent activity.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AccountsGetOutput {
    /// The account, with its balance.
    pub account: AccountView,
    /// The most recent entries touching the account, newest first.
    pub activity: Vec<AccountActivityView>,
}

/// How many recent entries `accounts.get` returns.
const ACTIVITY_LIMIT: u32 = 20;

/// `accounts.get`: one account, its balance and its recent activity.
pub struct AccountsGet;

#[async_trait]
impl Command for AccountsGet {
    const NAME: &'static str = "accounts.get";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = AccountsNumberInput;
    type Output = AccountsGetOutput;

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
        let entry = chart::show_account(db, &ctx.company_id, &input.number)
            .await
            .map_err(|err| CommandError::Domain(err.to_problem()))?;
        let balances = balances_of(db, &ctx.company_id, &input.number).await?;
        let activity =
            ledger_query::account_activity(db, &ctx.company_id, &input.number, ACTIVITY_LIMIT)
                .await
                .map_err(|err| domain(&err))?;
        Ok(Plan::with_output(AccountsGetOutput {
            account: view(entry, balances),
            activity: activity.into_iter().map(activity_view).collect(),
        }))
    }
}

/// The chart view of one account: its entry with the balance attached.
pub(crate) fn view(entry: AccountEntry, balances: Vec<Amount>) -> AccountView {
    let is_postable = entry.is_postable();
    let account = entry.account;
    AccountView {
        number: account.number,
        name_key: account.name_key,
        display_name: account.display_name,
        account_type: account.account_type.to_string(),
        normal_balance: account.normal_balance.to_string(),
        currency: account.currency.map(|c| c.code().to_string()),
        parent: account.parent,
        tags: account.tags,
        is_postable,
        archived: account.archived,
        balances,
    }
}

/// The balances of one account, one row per base currency.
async fn balances_of(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<Vec<Amount>, CommandError> {
    let balances = ledger_query::account_balances(db, company_id)
        .await
        .map_err(|err| domain(&err))?;
    Ok(balances
        .into_iter()
        .filter(|balance| balance.account_number == number)
        .map(|balance| Amount::from(balance.balance))
        .collect())
}

fn activity_view(activity: AccountActivity) -> AccountActivityView {
    AccountActivityView {
        entry_id: activity.entry_id,
        journal_id: activity.journal_id,
        fiscal_year: activity.fiscal_year,
        number: activity.number,
        posting_date: activity.posting_date,
        description: activity.description,
        minor: activity.base.minor().to_string(),
        currency: activity.base.currency().code().to_string(),
    }
}

/// Maps a chart error to the command error the adapters render.
fn chart_error(err: &ledgers_ledger::AccountsError) -> CommandError {
    CommandError::Domain(err.to_problem())
}

/// Builds the new account's draft from the create input. A person-named
/// account gets the `account.custom.<number>` key and its name as the
/// display name, so the name shows in every language.
fn draft_of(input: AccountsCreateInput) -> Result<AccountDraft, CommandError> {
    let account_type: AccountType =
        input
            .account_type
            .parse()
            .map_err(|err: ledgers_ledger::AccountsError| {
                CommandError::InvalidInput(err.to_string())
            })?;
    let currency = input
        .currency
        .as_deref()
        .map(Currency::from_code)
        .transpose()
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    Ok(AccountDraft {
        name_key: Account::custom_name_key(&input.number),
        number: input.number,
        account_type,
        normal_balance: None,
        currency,
        parent: input.parent,
        tags: input.tags,
        display_name: Some(input.name),
    })
}

/// The changes of an update input. `None` fields keep their value.
fn changes_of(input: AccountsUpdateInput) -> Result<AccountChanges, CommandError> {
    let account_type = input
        .account_type
        .as_deref()
        .map(str::parse::<AccountType>)
        .transpose()
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    let parent = if input.clear_parent {
        Some(None)
    } else {
        input.parent.map(Some)
    };
    Ok(AccountChanges {
        display_name: input.name,
        account_type,
        tags: input.tags,
        parent,
    })
}

impl ServerDispatch {
    /// The four chart writes: authenticate, refuse an agent key without the
    /// chart permission (audited), then run the change.
    pub(super) async fn accounts_write(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        command: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let db = deps.db.as_ref();
        let company = self.company_id.as_str();
        let at = rfc3339_utc(deps.clock.now());
        let actor = self
            .chart_actor(deps, presented, request_id, command, &input, &at)
            .await?;
        let audit = AuditContext::new(actor, request_id.to_owned(), deps.id_gen.ulid());

        let output = match command {
            "accounts.create" => {
                let body: AccountsCreateInput = serde_json::from_value(input)
                    .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
                let draft = draft_of(body).map_err(fail)?;
                let entry = chart::create_account(db, company, draft, at, &audit)
                    .await
                    .map_err(|err| fail(chart_error(&err)))?;
                let number = entry.account.number.clone();
                changed(db, company, entry, &number).await.map_err(fail)?
            }
            "accounts.update" => {
                let body: AccountsUpdateInput = serde_json::from_value(input)
                    .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
                let number = body.number.clone();
                let changes = changes_of(body).map_err(fail)?;
                let entry = chart::update_account(db, company, &number, changes, at, &audit)
                    .await
                    .map_err(|err| fail(chart_error(&err)))?;
                changed(db, company, entry, &number).await.map_err(fail)?
            }
            "accounts.archive" | "accounts.restore" => {
                let body: AccountsNumberInput = serde_json::from_value(input)
                    .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
                let entry = if command == "accounts.archive" {
                    chart::archive_account(db, company, &body.number, at, &audit).await
                } else {
                    chart::restore_account(db, company, &body.number, at, &audit).await
                }
                .map_err(|err| fail(chart_error(&err)))?;
                changed(db, company, entry, &body.number)
                    .await
                    .map_err(fail)?
            }
            other => {
                return Err(
                    Problem::validation_failed(format!("unknown chart command {other:?}"))
                        .instance(request_id),
                );
            }
        };
        Ok(output)
    }

    /// Authenticates the caller for a chart write and returns the actor the
    /// change is audited under. A refused agent key is audited as `denied`
    /// and answered with the policy-denied problem (403).
    async fn chart_actor(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        command: &str,
        input: &Value,
        at: &str,
    ) -> Result<ledgers_audit::Actor, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let db = deps.db.as_ref();
        let company = self.company_id.as_str();
        let principal = match presented
            .ok_or(CommandError::Unauthenticated)
            .map_err(fail)?
        {
            Presented::Bearer(token) => KeyStore {
                db: Arc::clone(&deps.db),
                clock: Arc::clone(&deps.clock),
                company_id: self.company_id.clone(),
                preloaded: None,
            }
            .authenticate(&token)
            .await
            .map_err(fail)?,
            Presented::Session(session) => {
                let id = session.id.clone();
                SessionAuthenticator {
                    db: Arc::clone(&deps.db),
                    clock: Arc::clone(&deps.clock),
                    app_origins: self.app_origins.clone(),
                    company_id: self.company_id.clone(),
                    presented: session,
                }
                .authenticate(&id)
                .await
                .map_err(fail)?
            }
            Presented::Principal(principal) => principal,
        };
        match principal {
            Principal::Person { person_id, .. } => Ok(ledgers_audit::Actor::Person { person_id }),
            Principal::Agent { key_id, tier, .. } => {
                let allowed = load_key(db, company, &key_id)
                    .await
                    .map_err(fail)?
                    .is_some_and(|key| key.chart_changes);
                let actor = ledgers_audit::Actor::Agent {
                    key_id: key_id.clone(),
                    tier: tier.to_string(),
                };
                if allowed && tier >= Tier::Draft {
                    return Ok(actor);
                }
                let reason = format!(
                    "{command} changes the chart of accounts; an agent key may do that only when its policy allows chart changes"
                );
                let audit =
                    AuditContext::new(actor.clone(), request_id.to_owned(), deps.id_gen.ulid());
                let record = AuditRecord::from_input(
                    command,
                    input,
                    Outcome::Denied,
                    Vec::new(),
                    Some(reason),
                );
                ledgers_audit::record(
                    db,
                    &AuditConfig::default(),
                    company,
                    &audit,
                    &record,
                    at,
                    ledgers_db::RetryPolicy::default(),
                    |_| {},
                )
                .await
                .map_err(|err| fail(CommandError::Database(DbError::Query(err.to_string()))))?;
                Err(fail(CommandError::PolicyDenied))
            }
        }
    }
}

/// The response for a changed account: the entry with its balance.
async fn changed(
    db: &dyn Database,
    company: &str,
    entry: AccountEntry,
    number: &str,
) -> Result<Value, CommandError> {
    let balances = balances_of(db, company, number).await?;
    let output = AccountsChangedOutput {
        account: view(entry, balances),
    };
    serde_json::to_value(output).map_err(|err| CommandError::InvalidInput(err.to_string()))
}
