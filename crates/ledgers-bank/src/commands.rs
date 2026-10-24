//! The bank commands: accounts (a person), statement import, the line list,
//! matching (post, link, ignore), rules and the reconciliation view.
//!
//! Every write is a guard batch (docs/adr/0004): a line can be matched or
//! ignored only while it is `unmatched`, checked by a guard statement in the
//! same batch as the change. Postings are built as the ledger's own
//! `journal.post_entry` input and planned by it, so policy, approval and the
//! audit trail are the executor's: an agent's post into the bank's ledger
//! account is held for a person, exactly as a bill payment is.

use crate::fingerprint;
use crate::iban;
use crate::model::{
    AccountView, AccountsArchiveInput, AccountsCreateInput, AccountsGetInput, AccountsListInput,
    AccountsListOutput, AccountsUpdateInput, BalanceView, MAX_IMPORT_LINES, MAX_LIST, MatchedView,
    OpenItemView, PostedBankEntry, ReceivablesOpenItemsInput, ReceivablesOpenItemsOutput,
    ReconcileInput, ReconciliationView, RuleView, RulesCreateInput, RulesCreateRequest,
    RulesEnableInput, RulesEnableOutput, RulesListInput, RulesListOutput, StatementSummary,
    StatementsImportInput, StatementsImportOutput, TransactionStatusOutput, TransactionView,
    TransactionsIgnoreInput, TransactionsLinkInput, TransactionsListInput, TransactionsListOutput,
    TransactionsPostInput, TransactionsPostOutput,
};
use crate::money::{Decimal, parse_minor};
use crate::parse::CsvMapping;
use crate::rules::RuleConditions;
use crate::statement::{BalanceCheck, BalanceRow, ParsedStatement, check_balances};
use crate::store::{self, AccountRow, TransactionFilter, TransactionRow};
use crate::suggest::{self, BillCandidate, EntryCandidate, LineInput, RuleCandidate};
use cratefield_core::{Database, DbError, Problem, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_db::guard;
use ledgers_ledger::surface::{
    PostEntry, PostEntryInput, SerializableAmount, SerializableEntryDraft, SerializableLine,
};
use ledgers_money::Currency;
use ledgers_subledger::allocate::{AllocateInput, AllocateItem, plan_allocate};
use sea_query::Value;
use std::collections::{BTreeMap, HashMap};

/// The chart tag of a bank's ledger account.
pub const TAG_BANK: &str = "bank";
/// The chart tag of accounts payable.
const TAG_PAYABLE: &str = "ap";
/// The chart tag of accounts receivable.
const TAG_RECEIVABLE: &str = "ar";
/// The journal a bank entry posts into when none is named (as bills do).
const DEFAULT_JOURNAL_CODE: &str = "GN";
/// The longest description a posted entry keeps.
const MAX_DESCRIPTION: usize = 200;
/// The status a line carries when a suggestion is shown for it.
const STATUS_SUGGESTED: &str = "suggested";
/// Flag set on a line whose remittance reads like an instruction.
pub const FLAG_INSTRUCTION_TEXT: &str = "remittance-reads-like-instruction";

/// The problem of a missing bank record.
fn not_found() -> CommandError {
    CommandError::Domain(Problem::not_found())
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(message.into())
}

/// The day of the command's clock, as a `YYYY-MM-DD` string.
fn today_of(ctx: &CommandContext) -> String {
    ctx.now.get(..10).unwrap_or("").to_owned()
}

fn iso_date(text: &str, what: &str) -> Result<String, CommandError> {
    ledgers_extract::schema::parse_iso_date(text)
        .map(|_| text.to_owned())
        .ok_or_else(|| invalid(format!("{what} {text:?} is not a YYYY-MM-DD date")))
}

fn currency_of(code: &str) -> Result<Currency, CommandError> {
    let currency = Currency::from_code(&code.to_ascii_uppercase())
        .map_err(|_| invalid(format!("{code:?} is not an ISO 4217 currency")))?;
    if !currency.is_active() {
        return Err(invalid(format!("{code} is not an active currency")));
    }
    Ok(currency)
}

fn amount(minor: i64, currency: &str) -> SerializableAmount {
    SerializableAmount {
        minor: minor.to_string(),
        currency: currency.to_owned(),
    }
}

/// A chart account as the bank commands read it.
#[derive(Clone, Debug)]
struct ChartAccount {
    number: String,
    postable: bool,
    archived: bool,
    tags: Vec<String>,
    currency: Option<String>,
}

async fn chart(db: &dyn Database, company_id: &str) -> Result<Vec<ChartAccount>, CommandError> {
    let entries = ledgers_ledger::commands::list_accounts(db, company_id, true)
        .await
        .map_err(|err| invalid(err.to_string()))?;
    Ok(entries
        .iter()
        .map(|entry| ChartAccount {
            number: entry.account.number.clone(),
            postable: entry.is_postable(),
            archived: entry.account.archived,
            tags: entry.account.tags.clone(),
            currency: entry.account.currency.map(|c| c.code().to_owned()),
        })
        .collect())
}

fn find_account<'a>(chart: &'a [ChartAccount], number: &str) -> Option<&'a ChartAccount> {
    chart.iter().find(|account| account.number == number)
}

fn payable_account(chart: &[ChartAccount], currency: &str) -> Option<String> {
    chart
        .iter()
        .find(|a| {
            a.postable
                && !a.archived
                && a.tags.iter().any(|t| t == TAG_PAYABLE)
                && a.currency.as_deref().is_none_or(|c| c == currency)
        })
        .map(|a| a.number.clone())
}

fn validate_mapping(mapping: &CsvMapping) -> Result<(), CommandError> {
    mapping.validate().map_err(invalid)
}

fn account_row_values(row: &AccountRow) -> Vec<Value> {
    vec![
        Value::from(row.name.as_str()),
        Value::from(row.bic.as_deref()),
        Value::from(
            row.csv_mapping
                .as_ref()
                .map(|m| serde_json::to_string(m).unwrap_or_default()),
        ),
        Value::from(row.updated_at.as_str()),
    ]
}

/// The view of an account from its row and the summaries computed for it.
#[derive(Default)]
struct Summary {
    /// The latest statement closing on or before the cutoff.
    statement: Option<BalanceRow>,
    /// The ledger balance of the linked account on or before the cutoff.
    ledger_minor: i64,
    /// Line counts by stored status.
    counts: HashMap<String, u32>,
    /// The date the balances are compared at.
    cutoff: String,
}

fn account_view(row: &AccountRow, summary: &Summary) -> AccountView {
    let statement_balance = summary.statement.as_ref().map(|b| BalanceView {
        date: b.date.clone(),
        amount: amount(b.minor, &row.currency),
    });
    // Without a statement there is nothing to compare the books with yet.
    let difference = summary
        .statement
        .as_ref()
        .map_or(0, |b| b.minor - summary.ledger_minor);
    AccountView {
        id: row.id.clone(),
        name: row.name.clone(),
        iban: row.iban.clone(),
        bic: row.bic.clone(),
        currency: row.currency.clone(),
        ledger_account_number: row.ledger_account_number.clone(),
        opening: amount(row.opening_minor, &row.currency),
        opening_date: row.opening_date.clone(),
        csv_mapping: row.csv_mapping.clone(),
        archived: row.archived_at.is_some(),
        statement_balance,
        ledger_balance: amount(summary.ledger_minor, &row.currency),
        difference: amount(difference, &row.currency),
        unmatched_count: summary.counts.get("unmatched").copied().unwrap_or(0),
        matched_count: summary.counts.get("matched").copied().unwrap_or(0),
        ignored_count: summary.counts.get("ignored").copied().unwrap_or(0),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
    }
}

/// Summaries for many accounts at once: the statements, the ledger lines and
/// the counts, one query each. The cutoff of an account is the date asked for
/// (`as_of`), else its latest statement's closing date, else today.
async fn summaries(
    db: &dyn Database,
    company_id: &str,
    accounts: &[AccountRow],
    as_of: &HashMap<String, Option<String>>,
    today: &str,
) -> Result<HashMap<String, Summary>, DbError> {
    let ids: Vec<String> = accounts.iter().map(|a| a.id.clone()).collect();
    let numbers: Vec<String> = accounts
        .iter()
        .map(|a| a.ledger_account_number.clone())
        .collect();
    let statements = store::statements_of(db, company_id, &ids).await?;
    let lines = store::ledger_lines(db, company_id, &numbers).await?;
    let counts = store::status_counts(db, company_id).await?;
    let mut out: HashMap<String, Summary> = HashMap::new();
    for account in accounts {
        let closings: Vec<(String, i64)> = statements
            .iter()
            .filter(|s| s.account_id == account.id)
            .filter_map(|s| Some((s.closing_date.clone()?, s.closing_minor?)))
            .collect();
        let latest_date = closings.iter().map(|(date, _)| date.clone()).max();
        let cutoff = as_of
            .get(&account.id)
            .cloned()
            .flatten()
            .or(latest_date)
            .unwrap_or_else(|| today.to_owned());
        let latest = closings
            .into_iter()
            .filter(|(date, _)| *date <= cutoff)
            .max_by(|a, b| a.0.cmp(&b.0));
        let ledger_minor = lines
            .iter()
            .filter(|(number, date, _)| *number == account.ledger_account_number && *date <= cutoff)
            .map(|(_, _, minor)| *minor)
            .sum();
        let mut tally = HashMap::new();
        for ((account_id, status), n) in &counts {
            if *account_id == account.id {
                tally.insert(status.clone(), *n);
            }
        }
        out.insert(
            account.id.clone(),
            Summary {
                statement: latest.map(|(date, minor)| BalanceRow { date, minor }),
                ledger_minor,
                counts: tally,
                cutoff,
            },
        );
    }
    Ok(out)
}

/// The views of accounts, in the order given.
async fn account_views(
    db: &dyn Database,
    company_id: &str,
    accounts: &[AccountRow],
    today: &str,
) -> Result<Vec<AccountView>, DbError> {
    let summary = summaries(db, company_id, accounts, &HashMap::new(), today).await?;
    Ok(accounts
        .iter()
        .map(|row| account_view(row, summary.get(&row.id).unwrap_or(&Summary::default())))
        .collect())
}

fn rule_view(row: &store::RuleRow) -> RuleView {
    RuleView {
        id: row.id.clone(),
        name: row.name.clone(),
        conditions: row.conditions.clone(),
        account_number: row.account_number.clone(),
        tax_code: row.tax_code.clone(),
        unit: row.unit.clone(),
        enabled: row.enabled,
        proposed_by: row.proposed_by.clone(),
        enabled_by: row.enabled_by.clone(),
        created_at: row.created_at.clone(),
    }
}

// --- accounts -------------------------------------------------------------

/// `bank.accounts.list`.
pub struct AccountsList;

#[async_trait::async_trait]
impl Command for AccountsList {
    const NAME: &'static str = "bank.accounts.list";
    const TIER: Tier = Tier::Read;
    type Input = AccountsListInput;
    type Output = AccountsListOutput;

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
        let rows = store::accounts(db, &ctx.company_id, input.include_archived).await?;
        let accounts = account_views(db, &ctx.company_id, &rows, &today_of(ctx)).await?;
        Ok(Plan::with_output(AccountsListOutput { accounts }))
    }
}

/// `bank.accounts.get`.
pub struct AccountsGet;

#[async_trait::async_trait]
impl Command for AccountsGet {
    const NAME: &'static str = "bank.accounts.get";
    const TIER: Tier = Tier::Read;
    const TARGET: Option<&'static str> = Some("id");
    type Input = AccountsGetInput;
    type Output = AccountView;

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
        let row = store::account(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(not_found)?;
        let views = account_views(
            db,
            &ctx.company_id,
            std::slice::from_ref(&row),
            &today_of(ctx),
        )
        .await?;
        let view = views.into_iter().next().ok_or_else(not_found)?;
        Ok(Plan::with_output(view))
    }
}

/// `bank.accounts.create`: a person only.
pub struct AccountsCreate;

#[async_trait::async_trait]
impl Command for AccountsCreate {
    const NAME: &'static str = "bank.accounts.create";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    type Input = AccountsCreateInput;
    type Output = AccountView;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    #[allow(clippy::too_many_lines)]
    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(invalid("the account name must be 1 to 120 characters"));
        }
        let iban = iban::validate(&input.iban).map_err(|err| invalid(err.to_string()))?;
        let currency = currency_of(&input.currency)?;
        let code = currency.code().to_owned();
        let opening = parse_minor(
            input.opening_amount.trim(),
            Decimal::Point,
            currency.exponent(),
        )
        .map_err(invalid)?;
        let opening_date = iso_date(&input.opening_date, "the opening date")?;
        if let Some(mapping) = &input.csv_mapping {
            validate_mapping(mapping)?;
        }
        let chart = chart(db, &ctx.company_id).await?;
        let ledger = find_account(&chart, &input.ledger_account_number).ok_or_else(|| {
            invalid(format!(
                "account {} does not exist",
                input.ledger_account_number
            ))
        })?;
        if !ledger.postable || ledger.archived {
            return Err(invalid(format!(
                "account {} is not a live, postable account",
                ledger.number
            )));
        }
        if !ledger.tags.iter().any(|t| t == TAG_BANK) {
            return Err(invalid(format!(
                "account {} is not a bank account (tag bank)",
                ledger.number
            )));
        }
        if ledger.currency.as_deref().is_some_and(|c| c != code) {
            return Err(invalid(format!(
                "account {} is not in {code}",
                ledger.number
            )));
        }
        let existing = db
            .query(&Statement::with_values(
                "SELECT id FROM bank_account WHERE company_id = ? AND (iban = ? OR ledger_account_number = ?)"
                    .to_owned(),
                vec![Value::from(ctx.company_id.as_str()), Value::from(iban.as_str()), Value::from(ledger.number.as_str())],
            ))
            .await?;
        if !existing.is_empty() {
            return Err(invalid(
                "a bank account with this IBAN or ledger account already exists",
            ));
        }
        let id = ctx.event_id.clone();
        let mapping_json = input
            .csv_mapping
            .as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());
        let bic = input
            .bic
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_owned);
        let row = AccountRow {
            id: id.clone(),
            name: name.to_owned(),
            iban: iban.clone(),
            bic: bic.clone(),
            currency: code.clone(),
            ledger_account_number: ledger.number.clone(),
            opening_minor: opening,
            opening_date: opening_date.clone(),
            csv_mapping: input.csv_mapping.clone(),
            archived_at: None,
            created_by: input.actor.clone(),
            created_at: ctx.now.clone(),
            updated_at: ctx.now.clone(),
        };
        let statement = Statement::with_values(
            "INSERT INTO bank_account (id, company_id, name, iban, bic, currency, ledger_account_number, \
             opening_minor, opening_date, csv_mapping, archived_at, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)"
                .to_owned(),
            vec![
                Value::from(id.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(name),
                Value::from(iban.as_str()),
                Value::from(bic.as_deref()),
                Value::from(code.as_str()),
                Value::from(ledger.number.as_str()),
                Value::from(opening),
                Value::from(opening_date.as_str()),
                Value::from(mapping_json),
                Value::from(input.actor.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.now.as_str()),
            ],
        );
        let view = account_view(
            &row,
            &Summary {
                cutoff: today_of(ctx),
                ..Summary::default()
            },
        );
        let mut out = Plan::with_output(view);
        out.statements = vec![statement];
        out.objects = vec![id];
        Ok(out)
    }
}

/// `bank.accounts.update`: a person only.
pub struct AccountsUpdate;

#[async_trait::async_trait]
impl Command for AccountsUpdate {
    const NAME: &'static str = "bank.accounts.update";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const TARGET: Option<&'static str> = Some("id");
    type Input = AccountsUpdateInput;
    type Output = AccountView;

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
        let mut row = store::account(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(not_found)?;
        if let Some(name) = &input.name {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 120 {
                return Err(invalid("the account name must be 1 to 120 characters"));
            }
            row.name = name.to_owned();
        }
        if let Some(bic) = &input.bic {
            row.bic = Some(bic.trim().to_owned()).filter(|b| !b.is_empty());
        }
        if let Some(mapping) = &input.csv_mapping {
            validate_mapping(mapping)?;
            row.csv_mapping = Some(mapping.clone());
        }
        row.updated_at = ctx.now.clone();
        let mut values = account_row_values(&row);
        values.push(Value::from(ctx.company_id.as_str()));
        values.push(Value::from(row.id.as_str()));
        let statement = Statement::with_values(
            "UPDATE bank_account SET name = ?, bic = ?, csv_mapping = ?, updated_at = ? \
             WHERE company_id = ? AND id = ?"
                .to_owned(),
            values,
        );
        let views = account_views(
            db,
            &ctx.company_id,
            std::slice::from_ref(&row),
            &today_of(ctx),
        )
        .await?;
        let mut out = Plan::with_output(views.into_iter().next().ok_or_else(not_found)?);
        out.statements = vec![statement];
        out.objects = vec![row.id];
        Ok(out)
    }
}

/// `bank.accounts.archive`: a person only. An archived account takes no
/// imports or postings; its lines stay readable.
pub struct AccountsArchive;

#[async_trait::async_trait]
impl Command for AccountsArchive {
    const NAME: &'static str = "bank.accounts.archive";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const TARGET: Option<&'static str> = Some("id");
    type Input = AccountsArchiveInput;
    type Output = AccountView;

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
        let mut row = store::account(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(not_found)?;
        if row.archived_at.is_some() {
            return Err(invalid("the account is already archived"));
        }
        row.archived_at = Some(ctx.now.clone());
        row.updated_at = ctx.now.clone();
        let statement = Statement::with_values(
            "UPDATE bank_account SET archived_at = ?, updated_at = ? WHERE company_id = ? AND id = ? \
             AND archived_at IS NULL"
                .to_owned(),
            vec![
                Value::from(ctx.now.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(row.id.as_str()),
            ],
        );
        let views = account_views(
            db,
            &ctx.company_id,
            std::slice::from_ref(&row),
            &today_of(ctx),
        )
        .await?;
        let mut out = Plan::with_output(views.into_iter().next().ok_or_else(not_found)?);
        out.statements = vec![statement];
        out.objects = vec![row.id];
        Ok(out)
    }
}

/// `bank.accounts.reconcile`: the statement balance against the ledger, and
/// what explains the difference.
pub struct AccountsReconcile;

#[async_trait::async_trait]
impl Command for AccountsReconcile {
    const NAME: &'static str = "bank.accounts.reconcile";
    const TIER: Tier = Tier::Read;
    const TARGET: Option<&'static str> = Some("id");
    type Input = ReconcileInput;
    type Output = ReconciliationView;

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
        let row = store::account(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(not_found)?;
        let as_of_request = input
            .as_of
            .as_deref()
            .map(|d| iso_date(d, "as_of"))
            .transpose()?;
        let mut as_of = HashMap::new();
        as_of.insert(row.id.clone(), as_of_request);
        let summary = summaries(
            db,
            &ctx.company_id,
            std::slice::from_ref(&row),
            &as_of,
            &today_of(ctx),
        )
        .await?
        .remove(&row.id)
        .unwrap_or_default();
        let cutoff = summary.cutoff.clone();
        let (unmatched_total, unmatched_count, ignored_total, ignored_count) =
            balance_summary(db, &ctx.company_id, &row.id, &cutoff).await?;
        // With no statement at or before the cutoff, the opening balance stands in.
        let statement_minor = summary
            .statement
            .as_ref()
            .map_or(row.opening_minor, |b| b.minor);
        let difference = statement_minor - summary.ledger_minor;
        let residual = difference - unmatched_total - ignored_total;
        let cur = &row.currency;
        Ok(Plan::with_output(ReconciliationView {
            account_id: row.id.clone(),
            as_of: cutoff,
            statement_balance: amount(statement_minor, cur),
            ledger_balance: amount(summary.ledger_minor, cur),
            difference: amount(difference, cur),
            unmatched_total: amount(unmatched_total, cur),
            ignored_total: amount(ignored_total, cur),
            residual: amount(residual, cur),
            reconciled: residual == 0 && unmatched_count == 0,
            unmatched_count,
            ignored_count,
        }))
    }
}

/// The sums and counts of an account's lines up to a date, by status:
/// `(unmatched total, unmatched count, ignored total, ignored count)`.
async fn balance_summary(
    db: &dyn Database,
    company_id: &str,
    account_id: &str,
    as_of: &str,
) -> Result<(i64, u32, i64, u32), DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT status, COUNT(*) AS n, COALESCE(SUM(minor), 0) AS total FROM bank_transaction \
             WHERE company_id = ? AND account_id = ? AND booking_date <= ? GROUP BY status"
                .to_owned(),
            vec![
                Value::from(company_id),
                Value::from(account_id),
                Value::from(as_of),
            ],
        ))
        .await?;
    let (mut unmatched, mut unmatched_n, mut ignored, mut ignored_n) = (0i64, 0u32, 0i64, 0u32);
    for row in &rows.rows {
        let status = row.get::<String>("status").unwrap_or_default();
        let n = u32::try_from(row.get::<i64>("n").unwrap_or(0)).unwrap_or(u32::MAX);
        let total = row.get::<i64>("total").unwrap_or(0);
        match status.as_str() {
            "unmatched" => {
                unmatched = total;
                unmatched_n = n;
            }
            "ignored" => {
                ignored = total;
                ignored_n = n;
            }
            _ => {}
        }
    }
    Ok((unmatched, unmatched_n, ignored, ignored_n))
}

// --- statements -----------------------------------------------------------

/// `bank.statements.import`: the statements of an uploaded document, each
/// line stored once.
pub struct StatementsImport;

/// Builds the import's statement and line rows, and the summary.
#[async_trait::async_trait]
impl Command for StatementsImport {
    const NAME: &'static str = "bank.statements.import";
    const TIER: Tier = Tier::Draft;
    type Input = StatementsImportInput;
    type Output = StatementsImportOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    #[allow(clippy::too_many_lines)]
    #[allow(clippy::too_many_lines)]
    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let company = ctx.company_id.as_str();
        let account = store::account(db, company, &input.account_id)
            .await?
            .ok_or_else(not_found)?;
        if account.archived_at.is_some() {
            return Err(invalid("the account is archived: it takes no imports"));
        }
        if ledgers_documents::store::get(db, company, &input.document_id)
            .await?
            .is_none()
        {
            return Err(invalid(format!(
                "document {} is not stored",
                input.document_id
            )));
        }
        let total_lines: usize = input.statements.iter().map(|s| s.lines.len()).sum();
        if total_lines > MAX_IMPORT_LINES {
            return Err(invalid(format!(
                "the file has {total_lines} lines; an import takes at most {MAX_IMPORT_LINES}: split the file"
            )));
        }
        let mut checks = Vec::with_capacity(input.statements.len());
        for statement in &input.statements {
            checks.push(check_balances(statement).map_err(|err| invalid(err.to_string()))?);
            if statement.currency != account.currency {
                return Err(invalid(format!(
                    "the statement is in {} but the account is in {}",
                    statement.currency, account.currency
                )));
            }
            if let Some(iban) = &statement.iban
                && *iban != account.iban
            {
                return Err(invalid(format!(
                    "the statement is for {iban}, not for this account"
                )));
            }
        }
        let prior_statements =
            store::statements_of(db, company, std::slice::from_ref(&account.id)).await?;
        if prior_statements.is_empty()
            && let Some(first) = input.statements.first().and_then(|s| s.opening.as_ref())
            && first.date == account.opening_date
            && first.minor != account.opening_minor
        {
            return Err(invalid(format!(
                "the statement opens at {} on {}, but the account's opening balance is {} on {}",
                first.minor, first.date, account.opening_minor, account.opening_date
            )));
        }
        let imported =
            store::imported_statements(db, company, &account.id, &input.document_id).await?;
        let existing = store::group_counts(db, company, &account.id).await?;

        let mut groups = Vec::with_capacity(total_lines);
        for statement in &input.statements {
            for line in &statement.lines {
                iso_date(&line.booking_date, "the booking date")?;
                if let Some(value) = &line.value_date {
                    iso_date(value, "the value date")?;
                }
                groups.push(fingerprint::group_key(
                    &account.id,
                    &line.booking_date,
                    line.minor,
                    &line.currency,
                    line.bank_reference.as_deref(),
                    line.counterparty_iban.as_deref(),
                    &line.remittance,
                ));
            }
        }
        let occurrences = fingerprint::assign_occurrences(&groups, &existing);

        let mut statements = Vec::new();
        let mut summaries = Vec::with_capacity(input.statements.len());
        let mut index = 0usize;
        let mut new_total = 0u32;
        let mut duplicate_total = 0u32;
        for (ordinal, statement) in input.statements.iter().enumerate() {
            let ordinal_u32 = u32::try_from(ordinal).unwrap_or(u32::MAX);
            // A statement imported before keeps its row; its new lines hang off it.
            let existing = imported
                .iter()
                .find(|(n, _)| *n == ordinal_u32)
                .map(|(_, id)| id.clone());
            let already = existing.is_some();
            let statement_id = existing.unwrap_or_else(|| format!("{}-s{ordinal}", ctx.event_id));
            let mut new_here = 0u32;
            let mut dup_here = 0u32;
            for line in &statement.lines {
                let occurrence = occurrences[index];
                let group = &groups[index];
                index += 1;
                match occurrence {
                    Some(occurrence) => {
                        new_here += 1;
                        statements.push(transaction_insert(
                            ctx,
                            &account.id,
                            &statement_id,
                            group,
                            occurrence,
                            index - 1,
                            line,
                        ));
                    }
                    None => dup_here += 1,
                }
            }
            new_total += new_here;
            duplicate_total += dup_here;
            if !already {
                statements.insert(
                    0,
                    statement_insert(
                        ctx,
                        &account.id,
                        &statement_id,
                        &input.document_id,
                        ordinal_u32,
                        statement,
                        &input.actor,
                        check_is_checked(checks[ordinal]),
                        statement.lines.len(),
                    ),
                );
            }
            summaries.push(StatementSummary {
                ordinal: ordinal_u32,
                format: statement.format,
                reference: statement.reference.clone(),
                opening: statement.opening.clone(),
                closing: statement.closing.clone(),
                balance_checked: check_is_checked(checks[ordinal]),
                lines: u32::try_from(statement.lines.len()).unwrap_or(u32::MAX),
                new_transactions: new_here,
                duplicate_transactions: dup_here,
                already_imported: already,
            });
        }
        let mut out = Plan::with_output(StatementsImportOutput {
            account_id: account.id.clone(),
            document_id: input.document_id.clone(),
            statements: summaries,
            new_transactions: new_total,
            duplicate_transactions: duplicate_total,
        });
        out.statements = statements;
        out.objects = vec![account.id];
        Ok(out)
    }
}

fn check_is_checked(check: BalanceCheck) -> bool {
    matches!(check, BalanceCheck::Checked)
}

#[allow(clippy::too_many_arguments)]
fn statement_insert(
    ctx: &CommandContext,
    account_id: &str,
    statement_id: &str,
    document_id: &str,
    ordinal: u32,
    statement: &ParsedStatement,
    actor: &str,
    balance_checked: bool,
    lines: usize,
) -> Statement {
    Statement::with_values(
        "INSERT INTO bank_statement (id, company_id, account_id, document_id, ordinal, format, reference, \
         currency, opening_date, opening_minor, closing_date, closing_minor, balance_checked, line_count, \
         created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            Value::from(statement_id),
            Value::from(ctx.company_id.as_str()),
            Value::from(account_id),
            Value::from(document_id),
            Value::from(i64::from(ordinal)),
            Value::from(statement.format.as_str()),
            Value::from(statement.reference.as_deref()),
            Value::from(statement.currency.as_str()),
            Value::from(statement.opening.as_ref().map(|b| b.date.clone())),
            Value::from(statement.opening.as_ref().map(|b| b.minor)),
            Value::from(statement.closing.as_ref().map(|b| b.date.clone())),
            Value::from(statement.closing.as_ref().map(|b| b.minor)),
            Value::from(i64::from(balance_checked)),
            Value::from(i64::try_from(lines).unwrap_or(i64::MAX)),
            Value::from(actor),
            Value::from(ctx.now.as_str()),
        ],
    )
}

fn transaction_insert(
    ctx: &CommandContext,
    account_id: &str,
    statement_id: &str,
    group: &str,
    occurrence: u32,
    position: usize,
    line: &crate::statement::ParsedLine,
) -> Statement {
    let id = format!("{}-t{position}", ctx.event_id);
    Statement::with_values(
        "INSERT INTO bank_transaction (id, company_id, account_id, statement_id, fingerprint, group_key, \
         booking_date, value_date, minor, currency, counterparty_name, counterparty_iban, remittance, \
         bank_reference, status, matched_entry_id, matched_bill_id, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'unmatched', NULL, NULL, ?, ?)"
            .to_owned(),
        vec![
            Value::from(id.as_str()),
            Value::from(ctx.company_id.as_str()),
            Value::from(account_id),
            Value::from(statement_id),
            Value::from(fingerprint::fingerprint(group, occurrence)),
            Value::from(group),
            Value::from(line.booking_date.as_str()),
            Value::from(line.value_date.as_deref()),
            Value::from(line.minor),
            Value::from(line.currency.as_str()),
            Value::from(line.counterparty_name.as_deref()),
            Value::from(line.counterparty_iban.as_deref()),
            Value::from(line.remittance.as_str()),
            Value::from(line.bank_reference.as_deref()),
            Value::from(ctx.now.as_str()),
            Value::from(ctx.now.as_str()),
        ],
    )
}

// --- transactions ---------------------------------------------------------

/// The suggestion inputs of a set of accounts: their open bills, their
/// posted lines and the enabled rules. Each is read once.
struct Candidates {
    rules: Vec<RuleCandidate>,
    bills: HashMap<String, Vec<BillCandidate>>,
    entries: HashMap<String, Vec<EntryCandidate>>,
}

async fn candidates(
    db: &dyn Database,
    company_id: &str,
    accounts: &[AccountRow],
) -> Result<Candidates, DbError> {
    let rules = store::rules(db, company_id, true)
        .await?
        .into_iter()
        .map(|r| RuleCandidate {
            id: r.id,
            conditions: r.conditions,
            account_number: r.account_number,
        })
        .collect();
    let mut currencies: Vec<String> = accounts.iter().map(|a| a.currency.clone()).collect();
    currencies.sort();
    currencies.dedup();
    let mut bills: HashMap<String, Vec<BillCandidate>> = HashMap::new();
    for currency in &currencies {
        let open = store::open_bills(db, company_id, currency).await?;
        bills.insert(
            currency.clone(),
            open.into_iter()
                .map(|b| BillCandidate {
                    id: b.id,
                    gross_minor: b.gross_minor,
                    vendor_name: b.vendor_name,
                    vendor_iban: b.vendor_iban,
                    invoice_number: b.invoice_number,
                    invoice_date: b.invoice_date,
                })
                .collect(),
        );
    }
    let numbers: Vec<String> = accounts
        .iter()
        .map(|a| a.ledger_account_number.clone())
        .collect();
    let lines = store::account_lines(db, company_id, &numbers, 500).await?;
    let mut entries: HashMap<String, Vec<EntryCandidate>> = HashMap::new();
    for line in lines {
        entries
            .entry(line.account_number)
            .or_default()
            .push(EntryCandidate {
                id: line.entry_id,
                minor: line.minor,
                posting_date: line.posting_date,
                description: line.description,
            });
    }
    Ok(Candidates {
        rules,
        bills,
        entries,
    })
}

/// The view of a line: its status (with suggestions), its match and its flags.
fn transaction_view(
    row: &TransactionRow,
    suggestions: Vec<crate::suggest::Suggestion>,
) -> TransactionView {
    let matched = match (&row.matched_entry_id, &row.matched_bill_id) {
        (Some(entry), _) => Some(MatchedView {
            kind: "entry".to_owned(),
            id: entry.clone(),
        }),
        (None, Some(bill)) => Some(MatchedView {
            kind: "bill".to_owned(),
            id: bill.clone(),
        }),
        (None, None) => None,
    };
    let status = if row.status == "unmatched" && !suggestions.is_empty() {
        STATUS_SUGGESTED.to_owned()
    } else {
        row.status.clone()
    };
    let flags = if ledgers_extract::guard::screen(&row.remittance).is_empty() {
        Vec::new()
    } else {
        vec![FLAG_INSTRUCTION_TEXT.to_owned()]
    };
    TransactionView {
        id: row.id.clone(),
        account_id: row.account_id.clone(),
        statement_id: row.statement_id.clone(),
        booking_date: row.booking_date.clone(),
        value_date: row.value_date.clone(),
        amount: amount(row.minor, &row.currency),
        counterparty_name: row.counterparty_name.clone(),
        counterparty_iban: row.counterparty_iban.clone(),
        remittance: row.remittance.clone(),
        bank_reference: row.bank_reference.clone(),
        status,
        matched,
        suggestions,
        flags,
        party: None,
    }
}

fn line_input(row: &TransactionRow) -> LineInput<'_> {
    LineInput {
        booking_date: &row.booking_date,
        minor: row.minor,
        counterparty_name: row.counterparty_name.as_deref(),
        counterparty_iban: row.counterparty_iban.as_deref(),
        remittance: &row.remittance,
    }
}

/// `bank.transactions.list`: lines newest first, with derived status and
/// suggestions. The page holds at most `limit` stored lines; a status filter
/// applies within that page.
pub struct TransactionsList;

#[async_trait::async_trait]
impl Command for TransactionsList {
    const NAME: &'static str = "bank.transactions.list";
    const TIER: Tier = Tier::Read;
    type Input = TransactionsListInput;
    type Output = TransactionsListOutput;

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
        let company = ctx.company_id.as_str();
        let want = match input.status.as_deref() {
            None => None,
            Some(s @ ("unmatched" | "suggested" | "matched" | "ignored")) => Some(s.to_owned()),
            Some(other) => {
                return Err(invalid(format!(
                    "status {other:?} is not one of the four states"
                )));
            }
        };
        let limit = input.limit.unwrap_or(50).clamp(1, MAX_LIST);
        let from = input
            .from
            .as_deref()
            .map(|d| iso_date(d, "from"))
            .transpose()?;
        let to = input.to.as_deref().map(|d| iso_date(d, "to")).transpose()?;
        let after = match input.after.as_deref() {
            None => None,
            Some(cursor) => {
                let (date, id) = cursor
                    .split_once('|')
                    .ok_or_else(|| invalid("the cursor is malformed"))?;
                Some((iso_date(date, "the cursor date")?, id.to_owned()))
            }
        };
        let accounts = store::accounts(db, company, true).await?;
        if let Some(account) = &input.account_id
            && !accounts.iter().any(|a| &a.id == account)
        {
            return Err(not_found());
        }
        let sql_status = match want.as_deref() {
            Some("matched") => Some("matched".to_owned()),
            Some("ignored") => Some("ignored".to_owned()),
            Some("unmatched" | "suggested") => Some("unmatched".to_owned()),
            _ => None,
        };
        let filter = TransactionFilter {
            account_id: input.account_id.clone(),
            status: sql_status,
            search: input
                .search
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_lowercase),
            from,
            to,
            after,
            take: limit + 1,
        };
        let mut rows = store::transactions(db, company, &filter).await?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let next = if more {
            rows.last()
                .map(|row| format!("{}|{}", row.booking_date, row.id))
        } else {
            None
        };
        let page_accounts: Vec<AccountRow> = accounts
            .iter()
            .filter(|a| rows.iter().any(|r| r.account_id == a.id))
            .cloned()
            .collect();
        let candidates = candidates(db, company, &page_accounts).await?;
        let parties = store::parties_by_iban(db, company).await?;
        let mut transactions = Vec::with_capacity(rows.len());
        for row in &rows {
            let suggestions = if row.status == "unmatched" {
                let account = accounts.iter().find(|a| a.id == row.account_id);
                let bills = account
                    .and_then(|a| candidates.bills.get(&a.currency))
                    .cloned()
                    .unwrap_or_default();
                let entries = account
                    .and_then(|a| candidates.entries.get(&a.ledger_account_number))
                    .cloned()
                    .unwrap_or_default();
                suggest::rank(&line_input(row), &candidates.rules, &bills, &entries)
            } else {
                Vec::new()
            };
            let mut view = transaction_view(row, suggestions);
            view.party = row
                .counterparty_iban
                .as_deref()
                .and_then(|iban| parties.get(&ledgers_bills::iban::normalise(iban)).cloned());
            if want.as_deref().is_none_or(|w| w == view.status) {
                transactions.push(view);
            }
        }
        Ok(Plan::with_output(TransactionsListOutput {
            transactions,
            next,
        }))
    }
}

/// Builds the ledger entry a line posts: the bank's ledger account against
/// the chosen counterparty, on the side the line's sign gives.
struct PostPlan {
    line: TransactionRow,
    entry: PostEntryInput,
    bill_id: Option<String>,
}

#[allow(clippy::too_many_lines)]
async fn post_plan(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &TransactionsPostInput,
) -> Result<PostPlan, CommandError> {
    let company = ctx.company_id.as_str();
    let line = store::transaction(db, company, &input.id)
        .await?
        .ok_or_else(not_found)?;
    if line.status != "unmatched" {
        return Err(invalid(format!(
            "the line is {} and cannot be posted",
            line.status
        )));
    }
    let account = store::account(db, company, &line.account_id)
        .await?
        .ok_or_else(not_found)?;
    if account.archived_at.is_some() {
        return Err(invalid(
            "the account is archived: its lines cannot be posted",
        ));
    }
    let chart = chart(db, company).await?;
    let (counterparty, bill_id) = match (&input.counterparty_account, &input.bill_id) {
        (Some(_), Some(_)) | (None, None) => {
            return Err(invalid(
                "give exactly one of counterparty_account or bill_id",
            ));
        }
        (None, Some(bill)) => {
            let open = store::open_bills(db, company, &line.currency).await?;
            let found = open
                .iter()
                .find(|b| &b.id == bill)
                .ok_or_else(|| invalid(format!("bill {bill} is not open in {}", line.currency)))?;
            if found.gross_minor != -line.minor {
                return Err(invalid(format!(
                    "bill {bill} is for {} and the line is {}",
                    found.gross_minor, line.minor
                )));
            }
            let payable = payable_account(&chart, &line.currency)
                .ok_or_else(|| invalid("the chart has no payable account (tag ap)"))?;
            (payable, Some(bill.clone()))
        }
        (Some(number), None) => {
            let target = find_account(&chart, number)
                .ok_or_else(|| invalid(format!("account {number} does not exist")))?;
            if !target.postable || target.archived {
                return Err(invalid(format!(
                    "account {number} is not a live, postable account"
                )));
            }
            if target.tags.iter().any(|t| t == TAG_BANK) {
                return Err(invalid(
                    "a bank account cannot be the other side of a bank line",
                ));
            }
            if target
                .currency
                .as_deref()
                .is_some_and(|c| c != line.currency)
            {
                return Err(invalid(format!(
                    "account {number} is not in {}",
                    line.currency
                )));
            }
            (number.clone(), None)
        }
    };
    let journal_id = match &input.journal_id {
        Some(id) => id.clone(),
        None => ledgers_ledger::journal::find_journal(db, company, DEFAULT_JOURNAL_CODE)
            .await
            .map_err(|err| invalid(err.to_string()))?
            .map(|journal| journal.id)
            .ok_or_else(|| invalid("no general journal GN exists"))?,
    };
    let fiscal_year = match input.fiscal_year {
        Some(year) => year,
        None => line
            .booking_date
            .get(..4)
            .and_then(|y| y.parse().ok())
            .ok_or_else(|| invalid("the booking date has no year"))?,
    };
    let statement_document = store::statements_of(db, company, std::slice::from_ref(&account.id))
        .await?
        .into_iter()
        .find(|s| s.id == line.statement_id)
        .map(|s| s.document_id)
        .ok_or_else(not_found)?;
    let abs = line.minor.unsigned_abs();
    let (bank_side, other_side) = if line.minor > 0 {
        ("debit", "credit")
    } else {
        ("credit", "debit")
    };
    let base = |minor: u128| SerializableAmount {
        minor: minor.to_string(),
        currency: line.currency.clone(),
    };
    let mut dimensions = BTreeMap::new();
    if let Some(unit) = input
        .unit
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        dimensions.insert("unit".to_owned(), unit.to_owned());
    }
    let description = describe(&line);
    let entry = SerializableEntryDraft {
        journal_id: journal_id.clone(),
        document_date: line.booking_date.clone(),
        posting_date: line.booking_date.clone(),
        tax_point_date: None,
        base_currency: line.currency.clone(),
        description,
        source_documents: vec![statement_document],
        external_id: Some(format!("bank-{}", line.id)),
        source_app: Some("ledgers-bank".to_owned()),
        reverses: None,
        lines: vec![
            SerializableLine {
                account_number: account.ledger_account_number.clone(),
                side: bank_side.to_owned(),
                base: base(u128::from(abs)),
                fx: None,
                tax_code: None,
                dimensions: BTreeMap::new(),
                memo: None,
            },
            SerializableLine {
                account_number: counterparty,
                side: other_side.to_owned(),
                base: base(u128::from(abs)),
                fx: None,
                tax_code: input.tax_code.clone().filter(|t| !t.trim().is_empty()),
                dimensions,
                memo: None,
            },
        ],
    };
    Ok(PostPlan {
        line,
        entry: PostEntryInput {
            journal_id,
            fiscal_year,
            entry,
        },
        bill_id,
    })
}

/// The entry's description: the counterparty and the remittance, bounded.
fn describe(line: &TransactionRow) -> String {
    let text = format!(
        "{} {}",
        line.counterparty_name.as_deref().unwrap_or(""),
        line.remittance
    );
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = collapsed.chars().take(MAX_DESCRIPTION).collect();
    if trimmed.is_empty() {
        format!("Bank line {}", line.booking_date)
    } else {
        trimmed
    }
}

/// `bank.transactions.post`: posts the line as a journal entry against an
/// account or pays a bill. Agents go through approval when policy says so.
pub struct TransactionsPost;

#[async_trait::async_trait]
impl Command for TransactionsPost {
    const NAME: &'static str = "bank.transactions.post";
    const TIER: Tier = Tier::PostWithinLimits;
    const SUPPORTS_DRY_RUN: bool = true;
    const TARGET: Option<&'static str> = Some("id");
    type Input = TransactionsPostInput;
    type Output = TransactionsPostOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let plan = post_plan(db, ctx, input).await?;
        PostEntry.preview(db, ctx, &plan.entry).await
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // A line to a receivables account is a customer's receipt: the subledger
        // settles the customer's open items with it.
        if let Some(number) = &input.counterparty_account
            && is_receivable(db, &ctx.company_id, number).await?
        {
            return receipt_plan(token, db, ctx, input).await;
        }
        let plan = post_plan(db, ctx, input).await?;
        // A line matched to a bill is a payment of that bill: the bills module
        // posts it and allocates the bill's open item, as any bill payment is.
        // Any other line posts its own entry through the journal.
        let (posted_statements, entry) = if let Some(bill_id) = &plan.bill_id {
            {
                if plan.line.minor >= 0 {
                    return Err(invalid("a bill is paid by an outgoing (negative) line"));
                }
                let account = store::account(db, &ctx.company_id, &plan.line.account_id)
                    .await?
                    .ok_or_else(not_found)?;
                let payment = ledgers_bills::commands::RecordPaymentInput {
                    id: bill_id.clone(),
                    amount: -plan.line.minor,
                    date: plan.line.booking_date.clone(),
                    from_account: account.ledger_account_number,
                    reference: Some(plan.line.id.clone()),
                };
                let (payment_plan, paid) =
                    ledgers_bills::commands::plan_record_payment(token, db, ctx, &payment).await?;
                (payment_plan.statements, paid.entry)
            }
        } else {
            {
                let posted = PostEntry.plan(token, db, ctx, &plan.entry).await?;
                (posted.statements, posted.output.entry)
            }
        };
        let key = guard::guard_key(&["bank-unmatched", &plan.line.id]);
        let mut statements = vec![guard::assert_statement(
            key.clone(),
            "(SELECT status FROM bank_transaction WHERE company_id = ? AND id = ?) = 'unmatched'",
            vec![
                Value::from(ctx.company_id.as_str()),
                Value::from(plan.line.id.as_str()),
            ],
        )];
        statements.extend(posted_statements);
        statements.push(Statement::with_values(
            "UPDATE bank_transaction SET status = 'matched', matched_entry_id = ?, matched_bill_id = ?, \
             updated_at = ? WHERE company_id = ? AND id = ? AND status = 'unmatched'"
                .to_owned(),
            vec![
                Value::from(entry.entry_id.as_str()),
                Value::from(plan.bill_id.as_deref()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(plan.line.id.as_str()),
            ],
        ));
        statements.push(guard::release_statement(key));
        let mut out = Plan::with_output(TransactionsPostOutput {
            transaction_id: plan.line.id.clone(),
            status: "matched".to_owned(),
            entry: PostedBankEntry {
                entry_id: entry.entry_id,
                journal_id: entry.journal_id,
                fiscal_year: entry.fiscal_year,
                number: entry.number,
                recorded_at: entry.recorded_at,
            },
        });
        out.statements = statements;
        out.objects = vec![plan.line.id];
        Ok(out)
    }
}

/// Whether an account of the chart is tagged as receivable.
async fn is_receivable(
    db: &dyn Database,
    company: &str,
    number: &str,
) -> Result<bool, CommandError> {
    let chart = chart(db, company).await?;
    Ok(find_account(&chart, number)
        .is_some_and(|account| account.tags.iter().any(|t| t == TAG_RECEIVABLE)))
}

/// A customer's open receivables in the order a receipt settles them: the named
/// items in the order named, or else every open invoice, oldest first.
async fn settle_order(
    db: &dyn Database,
    company: &str,
    party: &str,
    named: Option<&[String]>,
) -> Result<Vec<(String, i64)>, CommandError> {
    let Some(named) = named else {
        let rows = db
            .query(&Statement::with_values(
                "SELECT id, tx_remaining_minor FROM open_item \
                 WHERE company_id = ? AND ledger = 'ar' AND party_id = ? AND tx_remaining_minor > 0 \
                 ORDER BY posting_date, id"
                    .to_owned(),
                vec![Value::from(company), Value::from(party)],
            ))
            .await?;
        return Ok(rows
            .rows
            .iter()
            .filter_map(|row| {
                Some((
                    row.get::<String>("id")?,
                    row.get::<i64>("tx_remaining_minor")?,
                ))
            })
            .collect());
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for id in named {
        if !seen.insert(id.as_str()) {
            return Err(invalid(format!("open item {id} is named twice")));
        }
        let rows = db
            .query(&Statement::with_values(
                "SELECT id, tx_remaining_minor FROM open_item \
                 WHERE company_id = ? AND ledger = 'ar' AND party_id = ? AND id = ? AND tx_remaining_minor > 0"
                    .to_owned(),
                vec![Value::from(company), Value::from(party), Value::from(id.as_str())],
            ))
            .await?;
        let row = rows.rows.first().ok_or_else(|| {
            invalid(format!(
                "open item {id} is not an open receivable of this customer"
            ))
        })?;
        let remaining: i64 = row.get("tx_remaining_minor").unwrap_or(0);
        out.push((id.clone(), remaining));
    }
    Ok(out)
}

/// The allocations a payment of `paid` makes against the items in order: each
/// item takes what it owes until the payment runs out. A remainder is on account.
fn allocate_in_order(paid: i64, items: &[(String, i64)]) -> Vec<AllocateItem> {
    let mut left = paid;
    let mut out = Vec::new();
    for (id, remaining) in items {
        if left <= 0 {
            break;
        }
        let amount = (*remaining).min(left);
        if amount > 0 {
            out.push(AllocateItem {
                open_item_id: id.clone(),
                amount,
                paid_base: None,
            });
            left -= amount;
        }
    }
    out
}

/// A receipt: a line that settles a customer's open receivables. The subledger
/// allocates the payment; the excess becomes the customer's on-account item.
async fn receipt_plan(
    token: &ExecToken,
    db: &dyn Database,
    ctx: &CommandContext,
    input: &TransactionsPostInput,
) -> Result<Plan<TransactionsPostOutput>, CommandError> {
    let company = ctx.company_id.as_str();
    let line = store::transaction(db, company, &input.id)
        .await?
        .ok_or_else(not_found)?;
    if line.status != "unmatched" {
        return Err(invalid(format!(
            "the line is {} and cannot be posted",
            line.status
        )));
    }
    if line.minor <= 0 {
        return Err(invalid("a receipt is an incoming (positive) bank line"));
    }
    if line.currency != "EUR" {
        return Err(invalid(
            "receipts are booked in EUR, the company's base currency",
        ));
    }
    let party = input
        .party_id
        .as_deref()
        .filter(|party| !party.is_empty())
        .ok_or_else(|| invalid("a receivables line needs the customer: give party_id"))?;
    let account = store::account(db, company, &line.account_id)
        .await?
        .ok_or_else(not_found)?;
    if account.archived_at.is_some() {
        return Err(invalid(
            "the account is archived: its lines cannot be posted",
        ));
    }
    let journal_id = ledgers_ledger::journal::find_journal(db, company, DEFAULT_JOURNAL_CODE)
        .await
        .map_err(|err| invalid(err.to_string()))?
        .map(|journal| journal.id)
        .ok_or_else(|| invalid("no general journal GN exists"))?;

    let items = settle_order(db, company, party, input.open_item_ids.as_deref()).await?;
    let allocate = AllocateInput {
        ledger: "ar".to_owned(),
        party_id: party.to_owned(),
        journal_id,
        date: line.booking_date.clone(),
        bank_account: account.ledger_account_number.clone(),
        paid: line.minor,
        items: allocate_in_order(line.minor, &items),
        description: Some(line.remittance.clone()).filter(|text| !text.is_empty()),
        reference: Some(line.id.clone()),
    };
    let (allocated, paid) =
        plan_allocate(token, db, ctx, &allocate)
            .await
            .map_err(|err| match err {
                CommandError::InvalidInput(message) => invalid(message),
                other => other,
            })?;

    let key = guard::guard_key(&["bank-unmatched", &line.id]);
    let mut statements = vec![guard::assert_statement(
        key.clone(),
        "(SELECT status FROM bank_transaction WHERE company_id = ? AND id = ?) = 'unmatched'",
        vec![Value::from(company), Value::from(line.id.as_str())],
    )];
    statements.extend(allocated.statements);
    statements.push(Statement::with_values(
        "UPDATE bank_transaction SET status = 'matched', matched_entry_id = ?, matched_bill_id = NULL, \
         updated_at = ? WHERE company_id = ? AND id = ? AND status = 'unmatched'"
            .to_owned(),
        vec![
            Value::from(paid.entry.entry_id.as_str()),
            Value::from(ctx.now.as_str()),
            Value::from(company),
            Value::from(line.id.as_str()),
        ],
    ));
    statements.push(guard::release_statement(key));
    let entry = paid.entry;
    let mut out = Plan::with_output(TransactionsPostOutput {
        transaction_id: line.id.clone(),
        status: "matched".to_owned(),
        entry: PostedBankEntry {
            entry_id: entry.entry_id,
            journal_id: entry.journal_id,
            fiscal_year: entry.fiscal_year,
            number: entry.number,
            recorded_at: entry.recorded_at,
        },
    });
    out.statements = statements;
    out.objects = vec![line.id];
    Ok(out)
}

/// `bank.receivables.open_items`: a customer's open invoices, oldest first, for
/// a receipt to settle.
pub struct ReceivablesOpenItems;

#[async_trait::async_trait]
impl Command for ReceivablesOpenItems {
    const NAME: &'static str = "bank.receivables.open_items";
    const TIER: Tier = Tier::Read;
    type Input = ReceivablesOpenItemsInput;
    type Output = ReceivablesOpenItemsOutput;

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
        let rows = db
            .query(&Statement::with_values(
                "SELECT id, kind, source_id, posting_date, due_date, currency, tx_remaining_minor \
                 FROM open_item \
                 WHERE company_id = ? AND ledger = 'ar' AND party_id = ? AND tx_remaining_minor > 0 \
                 ORDER BY posting_date, id"
                    .to_owned(),
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.party_id.as_str()),
                ],
            ))
            .await?;
        let items = rows
            .rows
            .iter()
            .filter_map(|row| {
                let currency: String = row.get("currency")?;
                Some(OpenItemView {
                    id: row.get("id")?,
                    kind: row.get("kind")?,
                    document: row.get("source_id"),
                    posting_date: row.get("posting_date")?,
                    due_date: row.get("due_date"),
                    remaining: amount(row.get::<i64>("tx_remaining_minor")?, &currency),
                })
            })
            .collect();
        Ok(Plan::with_output(ReceivablesOpenItemsOutput { items }))
    }
}

/// `bank.transactions.link`: matches a line to an entry already posted on the
/// bank's ledger account, with the line's amount. Posts nothing.
pub struct TransactionsLink;

#[async_trait::async_trait]
impl Command for TransactionsLink {
    const NAME: &'static str = "bank.transactions.link";
    const TIER: Tier = Tier::Draft;
    const TARGET: Option<&'static str> = Some("id");
    type Input = TransactionsLinkInput;
    type Output = TransactionStatusOutput;

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
        let company = ctx.company_id.as_str();
        let line = store::transaction(db, company, &input.id)
            .await?
            .ok_or_else(not_found)?;
        if line.status != "unmatched" {
            return Err(invalid(format!(
                "the line is {} and cannot be linked",
                line.status
            )));
        }
        let account = store::account(db, company, &line.account_id)
            .await?
            .ok_or_else(not_found)?;
        let rows = db
            .query(&Statement::with_values(
                "SELECT l.base_minor FROM journal_line l JOIN journal_entry e ON e.id = l.entry_id \
                 WHERE e.company_id = ? AND e.id = ? AND l.account_number = ?"
                    .to_owned(),
                vec![
                    Value::from(company),
                    Value::from(input.entry_id.as_str()),
                    Value::from(account.ledger_account_number.as_str()),
                ],
            ))
            .await?;
        let amounts: Vec<i64> = rows
            .rows
            .iter()
            .filter_map(|r| r.get::<i64>("base_minor"))
            .collect();
        if amounts.len() != 1 || amounts[0] != line.minor {
            return Err(invalid(format!(
                "entry {} has no single line of {} on the bank account",
                input.entry_id, line.minor
            )));
        }
        let key = guard::guard_key(&["bank-unmatched", &line.id]);
        let mut out = Plan::with_output(TransactionStatusOutput {
            transaction_id: line.id.clone(),
            status: "matched".to_owned(),
            matched_entry_id: Some(input.entry_id.clone()),
        });
        out.statements = vec![
            guard::assert_statement(
                key.clone(),
                "(SELECT status FROM bank_transaction WHERE company_id = ? AND id = ?) = 'unmatched'",
                vec![Value::from(company), Value::from(line.id.as_str())],
            ),
            Statement::with_values(
                "UPDATE bank_transaction SET status = 'matched', matched_entry_id = ?, updated_at = ? \
                 WHERE company_id = ? AND id = ? AND status = 'unmatched'"
                    .to_owned(),
                vec![
                    Value::from(input.entry_id.as_str()),
                    Value::from(ctx.now.as_str()),
                    Value::from(company),
                    Value::from(line.id.as_str()),
                ],
            ),
            guard::release_statement(key),
        ];
        out.objects = vec![line.id];
        Ok(out)
    }
}

/// `bank.transactions.ignore`: leaves a line out of the books on purpose.
pub struct TransactionsIgnore;

#[async_trait::async_trait]
impl Command for TransactionsIgnore {
    const NAME: &'static str = "bank.transactions.ignore";
    const TIER: Tier = Tier::Draft;
    const TARGET: Option<&'static str> = Some("id");
    type Input = TransactionsIgnoreInput;
    type Output = TransactionStatusOutput;

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
        let company = ctx.company_id.as_str();
        let line = store::transaction(db, company, &input.id)
            .await?
            .ok_or_else(not_found)?;
        if line.status != "unmatched" {
            return Err(invalid(format!(
                "the line is {} and cannot be ignored",
                line.status
            )));
        }
        let key = guard::guard_key(&["bank-unmatched", &line.id]);
        let mut out = Plan::with_output(TransactionStatusOutput {
            transaction_id: line.id.clone(),
            status: "ignored".to_owned(),
            matched_entry_id: None,
        });
        out.statements = vec![
            guard::assert_statement(
                key.clone(),
                "(SELECT status FROM bank_transaction WHERE company_id = ? AND id = ?) = 'unmatched'",
                vec![Value::from(company), Value::from(line.id.as_str())],
            ),
            Statement::with_values(
                "UPDATE bank_transaction SET status = 'ignored', updated_at = ? \
                 WHERE company_id = ? AND id = ? AND status = 'unmatched'"
                    .to_owned(),
                vec![
                    Value::from(ctx.now.as_str()),
                    Value::from(company),
                    Value::from(line.id.as_str()),
                ],
            ),
            guard::release_statement(key),
        ];
        out.objects = vec![line.id];
        Ok(out)
    }
}

// --- rules ----------------------------------------------------------------

/// `bank.rules.list`.
pub struct RulesList;

#[async_trait::async_trait]
impl Command for RulesList {
    const NAME: &'static str = "bank.rules.list";
    const TIER: Tier = Tier::Read;
    type Input = RulesListInput;
    type Output = RulesListOutput;

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
        let rules = store::rules(db, &ctx.company_id, false).await?;
        Ok(Plan::with_output(RulesListOutput {
            rules: rules.iter().map(rule_view).collect(),
        }))
    }
}

/// Validates a rule's conditions and its account, shared by create.
async fn rule_account_ok(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<(), CommandError> {
    let chart = chart(db, company_id).await?;
    let account = find_account(&chart, number)
        .ok_or_else(|| invalid(format!("account {number} does not exist")))?;
    if !account.postable || account.archived {
        return Err(invalid(format!(
            "account {number} is not a live, postable account"
        )));
    }
    if account.tags.iter().any(|t| t == TAG_BANK) {
        return Err(invalid("a rule cannot post to a bank account"));
    }
    Ok(())
}

/// `bank.rules.create`. The server sets whether the rule applies: a person's
/// rule does, an agent's waits for a person to enable it.
pub struct RulesCreate;

#[async_trait::async_trait]
impl Command for RulesCreate {
    const NAME: &'static str = "bank.rules.create";
    const TIER: Tier = Tier::Draft;
    type Input = RulesCreateInput;
    type Output = RuleView;

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
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(invalid("the rule name must be 1 to 120 characters"));
        }
        if input.id.is_empty() {
            return Err(invalid("the rule has no id"));
        }
        input.conditions.validate().map_err(invalid)?;
        let conditions = RuleConditions {
            contains: input
                .conditions
                .contains
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned),
            counterparty_iban: input
                .conditions
                .counterparty_iban
                .as_deref()
                .map(iban::normalise),
            amount_min: input.conditions.amount_min,
            amount_max: input.conditions.amount_max,
        };
        rule_account_ok(db, &ctx.company_id, &input.account_number).await?;
        let tax_code = input.tax_code.clone().filter(|t| !t.trim().is_empty());
        let unit = input
            .unit
            .clone()
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty());
        let row = store::RuleRow {
            id: input.id.clone(),
            name: name.to_owned(),
            conditions: conditions.clone(),
            account_number: input.account_number.clone(),
            tax_code: tax_code.clone(),
            unit: unit.clone(),
            enabled: input.enabled,
            proposed_by: input.proposed_by.clone(),
            enabled_by: input.enabled.then(|| input.proposed_by.clone()),
            created_at: ctx.now.clone(),
        };
        let statement = Statement::with_values(
            "INSERT INTO bank_rule (id, company_id, name, contains_text, counterparty_iban, amount_min, \
             amount_max, account_number, tax_code, unit, enabled, proposed_by, enabled_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                .to_owned(),
            vec![
                Value::from(input.id.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(name),
                Value::from(conditions.contains.clone()),
                Value::from(conditions.counterparty_iban.clone()),
                Value::from(conditions.amount_min),
                Value::from(conditions.amount_max),
                Value::from(input.account_number.as_str()),
                Value::from(tax_code),
                Value::from(unit),
                Value::from(i64::from(input.enabled)),
                Value::from(input.proposed_by.as_str()),
                Value::from(row.enabled_by.clone()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.now.as_str()),
            ],
        );
        let mut out = Plan::with_output(rule_view(&row));
        out.statements = vec![statement];
        out.objects = vec![input.id.clone()];
        Ok(out)
    }
}

/// `bank.rules.enable`: a person only. An agent's proposed rule applies only
/// once this runs.
pub struct RulesEnable;

#[async_trait::async_trait]
impl Command for RulesEnable {
    const NAME: &'static str = "bank.rules.enable";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const TARGET: Option<&'static str> = Some("id");
    type Input = RulesEnableInput;
    type Output = RulesEnableOutput;

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
        let mut rule = store::rule(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(not_found)?;
        if rule.enabled {
            return Err(invalid("the rule is already enabled"));
        }
        rule.enabled = true;
        rule.enabled_by = Some(input.actor.clone());
        let statement = Statement::with_values(
            "UPDATE bank_rule SET enabled = 1, enabled_by = ?, updated_at = ? WHERE company_id = ? AND id = ? \
             AND enabled = 0"
                .to_owned(),
            vec![
                Value::from(input.actor.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        );
        let mut out = Plan::with_output(RulesEnableOutput {
            rule: rule_view(&rule),
        });
        out.statements = vec![statement];
        out.objects = vec![input.id.clone()];
        Ok(out)
    }
}

/// The command input of `bank.rules.create`, built from the public request
/// with the server-set fields (the id, who proposed the rule, and whether it applies).
#[must_use]
pub fn rule_request_to_input(
    request: RulesCreateRequest,
    id: String,
    proposed_by: String,
    enabled: bool,
) -> RulesCreateInput {
    RulesCreateInput {
        id,
        name: request.name,
        conditions: request.conditions,
        account_number: request.account_number,
        tax_code: request.tax_code,
        unit: request.unit,
        enabled,
        proposed_by,
    }
}
