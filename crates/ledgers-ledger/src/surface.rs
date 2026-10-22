//! Command trait implementations for `account.create` and `journal.post_entry`,
//! wiring ledgers-ledger domain operations through the executor.
//!
//! These commands convert the domain's existing functions into the unified
//! ledgers-commands Command interface, enabling CLI, MCP, HTTP, and app routing
//! from a single definition.

use crate::account::{AccountDraft, AccountEntry, AccountType, NormalBalance};
use crate::entry::{Entry, EntryDraft, Line};
use crate::posting::{PostContext, PostedEntry};
use cratefield_core::Database;
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_money::Currency;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::Date;
use time::Month;

/// Serializable version of [`AccountDraft`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SerializableAccountDraft {
    /// The account number (2..=10 ASCII digits).
    pub number: String,
    /// The translatable name key (must start with `"account."`).
    pub name_key: String,
    /// The account class.
    pub account_type: String,
    /// The balance side; `None` means the default for the type.
    pub normal_balance: Option<String>,
    /// The account's currency code; `None` = company base currency.
    pub currency: Option<String>,
    /// The parent account number; `None` = root.
    pub parent: Option<String>,
    /// Kebab-case tags.
    pub tags: Vec<String>,
}

impl SerializableAccountDraft {
    /// Converts to domain [`AccountDraft`].
    ///
    /// # Errors
    /// Returns an error if the account type, normal balance, or currency code is invalid.
    pub fn to_domain(self) -> Result<AccountDraft, String> {
        let account_type = self
            .account_type
            .parse::<AccountType>()
            .map_err(|e| e.to_string())?;
        let normal_balance = self
            .normal_balance
            .map(|nb| nb.parse::<NormalBalance>())
            .transpose()
            .map_err(|e| e.to_string())?;
        let currency = self
            .currency
            .map(|code| Currency::from_code(&code))
            .transpose()
            .map_err(|e| e.to_string())?;

        Ok(AccountDraft {
            number: self.number,
            name_key: self.name_key,
            account_type,
            normal_balance,
            currency,
            parent: self.parent,
            tags: self.tags,
        })
    }
}

impl From<AccountDraft> for SerializableAccountDraft {
    fn from(draft: AccountDraft) -> Self {
        Self {
            number: draft.number,
            name_key: draft.name_key,
            account_type: draft.account_type.to_string(),
            normal_balance: draft.normal_balance.map(|nb| nb.to_string()),
            currency: draft.currency.map(|c| c.code().to_string()),
            parent: draft.parent,
            tags: draft.tags,
        }
    }
}

/// Serializable version of [`crate::AccountEntry`].
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct SerializableAccountEntry {
    /// The account number.
    pub number: String,
    /// The translatable name key.
    pub name_key: String,
    /// The account class.
    pub account_type: String,
    /// The balance side.
    pub normal_balance: String,
    /// The currency code.
    pub currency: Option<String>,
    /// The parent account number.
    pub parent: Option<String>,
    /// The tags.
    pub tags: Vec<String>,
    /// Whether this account can hold postings.
    pub is_postable: bool,
}

impl From<AccountEntry> for SerializableAccountEntry {
    fn from(entry: AccountEntry) -> Self {
        let is_postable = entry.is_postable();
        let account = entry.account;
        Self {
            number: account.number,
            name_key: account.name_key,
            account_type: account.account_type.to_string(),
            normal_balance: account.normal_balance.to_string(),
            currency: account.currency.map(|c| c.code().to_string()),
            parent: account.parent,
            tags: account.tags,
            is_postable,
        }
    }
}

/// Serializable amount with currency.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SerializableAmount {
    /// The minor units as a string.
    pub minor: String,
    /// The currency code.
    pub currency: String,
}

impl From<ledgers_money::Amount> for SerializableAmount {
    fn from(amount: ledgers_money::Amount) -> Self {
        Self {
            minor: amount.minor().to_string(),
            currency: amount.currency().code().to_string(),
        }
    }
}

/// Serializable version of [`Line`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SerializableLine {
    /// The account number.
    pub account_number: String,
    /// The side (debit/credit).
    pub side: String,
    /// The base amount.
    pub base: SerializableAmount,
    /// Optional FX transaction.
    pub fx: Option<SerializableFx>,
    /// Optional tax code.
    pub tax_code: Option<String>,
    /// Optional dimensions.
    pub dimensions: BTreeMap<String, String>,
    /// Optional memo.
    pub memo: Option<String>,
}

/// Serializable FX data.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SerializableFx {
    /// The transaction amount.
    pub transaction: SerializableAmount,
    /// The exchange rate.
    pub rate: String,
}

/// Serializable version of [`EntryDraft`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SerializableEntryDraft {
    /// The journal ID.
    pub journal_id: String,
    /// The document date (YYYY-MM-DD).
    pub document_date: String,
    /// The posting date (YYYY-MM-DD).
    pub posting_date: String,
    /// The optional tax point date (YYYY-MM-DD).
    pub tax_point_date: Option<String>,
    /// The base currency code.
    pub base_currency: String,
    /// The description.
    pub description: String,
    /// Source document links.
    pub source_documents: Vec<String>,
    /// External ID from the source application (for idempotency).
    pub external_id: Option<String>,
    /// The application that posted this entry.
    pub source_app: Option<String>,
    /// Entry this reverses, if any.
    pub reverses: Option<String>,
    /// The entry lines.
    pub lines: Vec<SerializableLine>,
}

/// Helper to parse a date from YYYY-MM-DD format.
fn parse_date(date_str: &str) -> Result<Date, String> {
    let parts: Vec<&str> = date_str.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("invalid date format: {date_str}"));
    }
    let year = parts[0]
        .parse::<i32>()
        .map_err(|_| format!("invalid year: {}", parts[0]))?;
    let month = parts[1]
        .parse::<u8>()
        .map_err(|_| format!("invalid month: {}", parts[1]))?;
    let day = parts[2]
        .parse::<u8>()
        .map_err(|_| format!("invalid day: {}", parts[2]))?;

    let month_enum = Month::try_from(month).map_err(|_| format!("invalid month value: {month}"))?;

    Date::from_calendar_date(year, month_enum, day).map_err(|_| format!("invalid date: {date_str}"))
}

impl SerializableEntryDraft {
    /// Converts to domain [`EntryDraft`].
    ///
    /// # Errors
    /// Returns an error if dates are malformed, currency codes are invalid, or amounts don't parse.
    pub fn to_domain(self) -> Result<EntryDraft, String> {
        let document_date = parse_date(&self.document_date)?;
        let posting_date = parse_date(&self.posting_date)?;
        let tax_point_date = self.tax_point_date.map(|d| parse_date(&d)).transpose()?;

        let base_currency = Currency::from_code(&self.base_currency)
            .map_err(|e| format!("invalid base_currency: {e}"))?;

        let lines =
            self.lines
                .into_iter()
                .map(|line| {
                    let side = match line.side.as_str() {
                        "debit" => crate::entry::Side::Debit,
                        "credit" => crate::entry::Side::Credit,
                        _ => return Err(format!("invalid side: {}", line.side)),
                    };

                    let base_amount = ledgers_money::Amount::new(
                        line.base
                            .minor
                            .parse::<i128>()
                            .map_err(|e| format!("invalid base minor: {e}"))?,
                        base_currency,
                    );

                    let fx =
                        line.fx
                            .map(|fx_data| -> Result<crate::entry::Fx, String> {
                                let tx_amount = ledgers_money::Amount::new(
                                    fx_data.transaction.minor.parse::<i128>().map_err(|e| {
                                        format!("invalid fx transaction minor: {e}")
                                    })?,
                                    Currency::from_code(&fx_data.transaction.currency)
                                        .map_err(|e| format!("invalid fx currency: {e}"))?,
                                );
                                let rate = ledgers_money::Rate::parse(&fx_data.rate)
                                    .map_err(|e| format!("invalid fx rate: {e}"))?;
                                Ok(crate::entry::Fx {
                                    transaction: tx_amount,
                                    rate,
                                })
                            })
                            .transpose()?;

                    Ok(Line {
                        account_number: line.account_number,
                        side,
                        base: base_amount,
                        fx,
                        tax_code: line.tax_code,
                        dimensions: line.dimensions,
                        memo: line.memo,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;

        Ok(EntryDraft {
            journal_id: self.journal_id,
            document_date,
            posting_date,
            tax_point_date,
            base_currency,
            description: self.description,
            source_documents: self.source_documents,
            external_id: self.external_id,
            source_app: self.source_app,
            reverses: self.reverses,
            lines,
        })
    }
}

/// Input for `account.create` command.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateAccountInput {
    /// The account draft (number, `name_key`, type, normal balance, currency, parent, tags).
    pub draft: SerializableAccountDraft,
}

/// Output for `account.create` command.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct CreateAccountOutput {
    /// The created account entry with its assigned metadata.
    pub account: SerializableAccountEntry,
}

/// The `account.create` command.
pub struct CreateAccount;

#[async_trait::async_trait]
impl Command for CreateAccount {
    const NAME: &'static str = "account.create";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = false;
    const IDEMPOTENT: bool = false;

    type Input = CreateAccountInput;
    type Output = CreateAccountOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // account.create moves no money, so preview is empty
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Convert serializable DTO to domain type
        let domain_draft = input
            .draft
            .clone()
            .to_domain()
            .map_err(CommandError::InvalidInput)?;

        let (statements, account) = crate::commands::plan_create_account(
            token,
            db,
            &ctx.company_id,
            &domain_draft,
            &ctx.now,
        )
        .await
        .map_err(|err| err.to_problem())?;

        let mut plan = Plan::with_output(CreateAccountOutput {
            account: SerializableAccountEntry::from(account),
        });
        plan.statements = statements;
        Ok(plan)
    }
}

/// Serializable version of [`PostedEntry`].
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct SerializablePostedEntry {
    /// The entry's id.
    pub entry_id: String,
    /// The journal it posted into.
    pub journal_id: String,
    /// The fiscal year the number is scoped to.
    pub fiscal_year: i64,
    /// The gapless number assigned.
    pub number: i64,
    /// The recorded-at timestamp.
    pub recorded_at: String,
}

impl From<PostedEntry> for SerializablePostedEntry {
    fn from(entry: PostedEntry) -> Self {
        Self {
            entry_id: entry.entry_id,
            journal_id: entry.journal_id,
            fiscal_year: entry.fiscal_year,
            number: entry.number,
            recorded_at: entry.recorded_at,
        }
    }
}

/// Input for `journal.post_entry` command.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostEntryInput {
    /// The journal ID.
    pub journal_id: String,
    /// The fiscal year for the entry number sequence.
    pub fiscal_year: i64,
    /// The entry draft (must balance, valid accounts, etc.).
    pub entry: SerializableEntryDraft,
}

/// Output for `journal.post_entry` command.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct PostEntryOutput {
    /// The posted entry with its assigned number.
    pub entry: SerializablePostedEntry,
}

/// The `journal.post_entry` command.
pub struct PostEntry;

#[async_trait::async_trait]
impl Command for PostEntry {
    const NAME: &'static str = "journal.post_entry";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = true;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = Some("journal_id");

    type Input = PostEntryInput;
    type Output = PostEntryOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Convert serializable draft to domain Entry
        let domain_draft = input
            .entry
            .clone()
            .to_domain()
            .map_err(CommandError::InvalidInput)?;
        let entry = Entry::from_draft(domain_draft, &ctx.company_id)
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;

        let (entry, _provisional_number) = crate::posting::dry_run_entry(
            db,
            &entry,
            &PostContext {
                fiscal_year: input.fiscal_year,
                entry_id: "preview".to_string(),
                recorded_at: String::new(),
                outbox_id: String::new(),
            },
        )
        .await
        .map_err(|je| CommandError::InvalidInput(je.to_string()))?;

        let mut preview = JournalPreview::empty();
        for line in &entry.lines {
            let side = line.side.to_string().to_lowercase();
            let txn = line.fx.as_ref().map(|fx| fx.transaction);
            preview
                .add_line(line.account_number.clone(), &side, line.base, txn)
                .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
        }
        Ok(preview)
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Convert serializable draft to domain Entry
        let domain_draft = input
            .entry
            .clone()
            .to_domain()
            .map_err(CommandError::InvalidInput)?;
        let entry = Entry::from_draft(domain_draft, &ctx.company_id)
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;

        let (statements, posted_entry) = crate::posting::plan_post_entry(
            token,
            db,
            &ctx.company_id,
            &entry,
            input.fiscal_year,
            &ctx.now,
            &ctx.request_id,
        )
        .await
        .map_err(|err| err.to_problem())?;

        let mut plan = Plan::with_output(PostEntryOutput {
            entry: SerializablePostedEntry::from(posted_entry),
        });
        plan.statements = statements;
        Ok(plan)
    }
}

/// Registry function: returns a ledgers-commands Registry with surface commands.
#[must_use]
pub fn registry() -> ledgers_commands::Registry {
    let mut reg = ledgers_commands::Registry::new();

    // Register CreateAccount command
    let input_schema = serde_json::to_value(schemars::schema_for!(CreateAccountInput))
        .unwrap_or(serde_json::json!({}));
    let output_schema = serde_json::to_value(schemars::schema_for!(CreateAccountOutput))
        .unwrap_or(serde_json::json!({}));
    if let Ok(descriptor) = ledgers_commands::CommandDescriptor::from_command::<CreateAccount>(
        input_schema,
        output_schema,
    ) {
        let _ = reg.register(descriptor);
    }

    // Register PostEntry command
    let input_schema = serde_json::to_value(schemars::schema_for!(PostEntryInput))
        .unwrap_or(serde_json::json!({}));
    let output_schema = serde_json::to_value(schemars::schema_for!(PostEntryOutput))
        .unwrap_or(serde_json::json!({}));
    if let Ok(descriptor) =
        ledgers_commands::CommandDescriptor::from_command::<PostEntry>(input_schema, output_schema)
    {
        let _ = reg.register(descriptor);
    }

    reg
}
