//! The `subledger.write-off` command: write off an open item with person/agent controls.

use crate::builders;
use crate::error::CONTROL_ACCOUNT_NOT_FOUND;
use crate::item::{Ledger, OpenItem, settle};
use async_trait::async_trait;
use cratefield_core::{Database, Problem, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_db::guard;
use ledgers_ledger::surface::{
    PostEntryInput, SerializableAmount, SerializableEntryDraft, SerializableLine,
    plan_post_entry_with_control,
};
use ledgers_money::{Amount, Currency};
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum amount an agent can write off (in base minor units).
const AGENT_WRITE_OFF_LIMIT_MINOR: i64 = 5_000; // 50.00 in base currency

/// Input to the `subledger.write-off` command.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct WriteOffInput {
    /// Open item ID.
    pub open_item_id: String,
    /// Journal ID where the entry posts.
    pub journal_id: String,
    /// Posting date (ISO 8601).
    pub date: String,
    /// Amount in transaction currency (minor units, positive).
    pub amount: i64,
    /// Write-off account (e.g., 6950 for AR, expense for AP).
    pub account: String,
    /// Reason for write-off.
    pub reason: String,
    /// Optional description.
    pub description: Option<String>,
    /// Optional reference.
    pub reference: Option<String>,
}

/// Output of the `subledger.write-off` command.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct WriteOffOutput {
    /// The entry ID.
    pub entry_id: String,
    /// Transaction remaining after write-off.
    pub tx_remaining: i64,
    /// Base remaining after write-off.
    pub base_remaining: i64,
}

/// The `subledger.write-off` command.
pub struct SubledgerWriteOff;

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Command for SubledgerWriteOff {
    const NAME: &'static str = "subledger.write-off";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = true;
    const IDEMPOTENT: bool = false;
    const TARGET: Option<&'static str> = None;

    type Input = WriteOffInput;
    type Output = WriteOffOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Load item and validate
        let _item = load_open_item(db, &input.open_item_id).await?;
        let _chart = load_chart(db, &ctx.company_id).await?;

        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Load item
        let item = load_open_item(db, &input.open_item_id).await?;

        // Verify the item belongs to this company
        if item.company_id != ctx.company_id {
            return Err(CommandError::InvalidInput(
                "open_item does not belong to this company".to_string(),
            ));
        }

        // Settle the item to compute base amount
        let base_cleared = settle(&item, input.amount).map_err(CommandError::from)?;

        // Agent ceiling check: agents cannot write off more than AGENT_WRITE_OFF_LIMIT_MINOR
        if !token.acts_as_person() && base_cleared > AGENT_WRITE_OFF_LIMIT_MINOR {
            return Err(CommandError::Domain(
                Problem::new(&crate::error::WRITE_OFF_REQUIRES_PERSON).with_detail(format!(
                    "agents can write off at most {AGENT_WRITE_OFF_LIMIT_MINOR} in base currency"
                )),
            ));
        }

        // Load chart
        let chart = load_chart(db, &ctx.company_id).await?;
        let control_account = chart
            .control_account(item.ledger)
            .ok_or_else(|| CommandError::Domain(Problem::new(&CONTROL_ACCOUNT_NOT_FOUND)))?
            .to_owned();

        // Build entry
        let mut entry_lines = Vec::new();
        let currency = Currency::from_code("EUR")
            .map_err(|e| CommandError::InvalidInput(format!("invalid currency: {e}")))?;
        let base_amount = Amount::new(i128::from(base_cleared), currency);

        match item.ledger {
            Ledger::Ar => {
                // AR write-off: Dr 6950 (expense) / Cr 1400 (AR control)
                entry_lines.push(SerializableLine {
                    account_number: input.account.clone(),
                    side: "debit".to_string(),
                    base: SerializableAmount {
                        minor: base_amount.minor().to_string(),
                        currency: "EUR".to_string(),
                    },
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: Some(format!("Write-off: {}", input.reason)),
                });
                entry_lines.push(SerializableLine {
                    account_number: control_account,
                    side: "credit".to_string(),
                    base: SerializableAmount {
                        minor: base_amount.minor().to_string(),
                        currency: "EUR".to_string(),
                    },
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });
            }
            Ledger::Ap => {
                // AP write-off: Dr 1600 (AP control) / Cr 6950 (income)
                entry_lines.push(SerializableLine {
                    account_number: control_account,
                    side: "debit".to_string(),
                    base: SerializableAmount {
                        minor: base_amount.minor().to_string(),
                        currency: "EUR".to_string(),
                    },
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });
                entry_lines.push(SerializableLine {
                    account_number: input.account.clone(),
                    side: "credit".to_string(),
                    base: SerializableAmount {
                        minor: base_amount.minor().to_string(),
                        currency: "EUR".to_string(),
                    },
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: Some(format!("Write-off: {}", input.reason)),
                });
            }
        }

        let fiscal_year = input
            .date
            .get(..4)
            .and_then(|y| y.parse::<i64>().ok())
            .ok_or_else(|| CommandError::InvalidInput("invalid date year".to_string()))?;

        let entry_input = PostEntryInput {
            journal_id: input.journal_id.clone(),
            fiscal_year,
            entry: SerializableEntryDraft {
                journal_id: input.journal_id.clone(),
                document_date: input.date.clone(),
                posting_date: input.date.clone(),
                tax_point_date: None,
                base_currency: "EUR".to_owned(),
                description: input
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("Write-off: {}", input.reason)),
                source_documents: Vec::new(),
                external_id: Some(format!("writeoff-{}", input.open_item_id)),
                source_app: Some("ledgers-subledger".to_owned()),
                reverses: None,
                lines: entry_lines,
            },
        };

        let mut posted = plan_post_entry_with_control(token, db, ctx, &entry_input).await?;
        let entry_id = posted.output.entry.entry_id.clone();

        // Build allocation row
        let allocation_id = format!("alloc_writeoff_{}", ctx.event_id);
        posted.statements.push(builders::build_allocation_insert(
            &allocation_id,
            &ctx.company_id,
            &item.id,
            &entry_id,
            "write-off",
            input.amount,
            base_cleared,
            0i64, // No FX difference for write-off
            &input.date,
            &ctx.now,
        ));

        // Guard: verify item still has expected remaining values
        let guard_key = guard::guard_key(&["writeoff-item", entry_id.as_str(), item.id.as_str()]);
        posted.statements.push(guard::assert_statement(
            &guard_key,
            "(SELECT 1 FROM open_item WHERE id = ? AND company_id = ? AND tx_remaining_minor = ? \
             AND base_remaining_minor = ?) IS NOT NULL",
            vec![
                Value::from(item.id.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(item.tx_remaining_minor),
                Value::from(item.base_remaining_minor),
            ],
        ));

        // Update open_item with relative decrement
        posted.statements.push(Statement::with_values(
            "UPDATE open_item SET tx_remaining_minor = tx_remaining_minor - ?, \
             base_remaining_minor = base_remaining_minor - ?, updated_at = ? \
             WHERE id = ? AND company_id = ?"
                .to_owned(),
            vec![
                Value::from(input.amount),
                Value::from(base_cleared),
                Value::from(ctx.now.as_str()),
                Value::from(item.id.as_str()),
                Value::from(ctx.company_id.as_str()),
            ],
        ));

        posted.statements.push(guard::release_statement(&guard_key));

        let mut plan = Plan::with_output(WriteOffOutput {
            entry_id,
            tx_remaining: item.tx_remaining_minor - input.amount,
            base_remaining: item.base_remaining_minor - base_cleared,
        });
        plan.statements = posted.statements;
        plan.objects = vec![item.party_id.clone()];

        Ok(plan)
    }
}

/// Information about a chart account.
#[derive(Clone, Debug)]
struct AccountInfo {
    #[allow(dead_code)]
    postable: bool,
    tags: Vec<String>,
}

/// The chart, indexed by number.
#[derive(Clone, Debug, Default)]
struct Chart {
    by_number: BTreeMap<String, AccountInfo>,
    ar_control: Option<String>,
    ap_control: Option<String>,
}

impl Chart {
    fn from_entries(entries: &[ledgers_ledger::account::AccountEntry]) -> Self {
        let mut chart = Chart::default();
        for entry in entries {
            let account = &entry.account;
            let info = AccountInfo {
                postable: entry.is_postable(),
                tags: account.tags.clone(),
            };
            if !account.archived {
                if chart.ar_control.is_none() && info.tags.iter().any(|t| t == "ar") {
                    chart.ar_control = Some(account.number.clone());
                }
                if chart.ap_control.is_none() && info.tags.iter().any(|t| t == "ap") {
                    chart.ap_control = Some(account.number.clone());
                }
            }
            chart.by_number.insert(account.number.clone(), info);
        }
        chart
    }

    fn control_account(&self, ledger: Ledger) -> Option<&str> {
        match ledger {
            Ledger::Ar => self.ar_control.as_deref(),
            Ledger::Ap => self.ap_control.as_deref(),
        }
    }
}

async fn load_chart(db: &dyn Database, company_id: &str) -> Result<Chart, CommandError> {
    let entries = ledgers_ledger::commands::list_accounts(db, company_id, true)
        .await
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    Ok(Chart::from_entries(&entries))
}

/// Load an open item by ID.
async fn load_open_item(db: &dyn Database, item_id: &str) -> Result<OpenItem, CommandError> {
    let rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT id, company_id, ledger, kind, party_id, source_id, entry_id, control_account, \
             currency, rate_scaled, posting_date, due_date, \
             tx_original_minor, base_original_minor, tx_remaining_minor, base_remaining_minor \
             FROM open_item WHERE id = ? LIMIT 1"
                .to_owned(),
            vec![Value::from(item_id)],
        ))
        .await
        .map_err(|e| CommandError::InvalidInput(format!("failed to load item: {e}")))?;

    let row = rows
        .first()
        .ok_or_else(|| CommandError::InvalidInput(format!("open_item {item_id} not found")))?;

    let ledger_str: String = row
        .get("ledger")
        .ok_or_else(|| CommandError::InvalidInput("invalid ledger".to_string()))?;
    let ledger = match ledger_str.as_str() {
        "ar" => Ledger::Ar,
        "ap" => Ledger::Ap,
        _ => return Err(CommandError::InvalidInput("invalid ledger".to_string())),
    };

    Ok(OpenItem {
        id: row
            .get("id")
            .ok_or_else(|| CommandError::InvalidInput("missing id".to_string()))?,
        company_id: row
            .get("company_id")
            .ok_or_else(|| CommandError::InvalidInput("missing company_id".to_string()))?,
        ledger,
        kind: match row
            .get::<String>("kind")
            .ok_or_else(|| CommandError::InvalidInput("missing kind".to_string()))?
            .as_str()
        {
            "invoice" => crate::item::ItemKind::Invoice,
            "credit-note" => crate::item::ItemKind::CreditNote,
            "bill" => crate::item::ItemKind::Bill,
            "on-account" => crate::item::ItemKind::OnAccount,
            _ => return Err(CommandError::InvalidInput("invalid kind".to_string())),
        },
        party_id: row
            .get("party_id")
            .ok_or_else(|| CommandError::InvalidInput("missing party_id".to_string()))?,
        source_id: row.get("source_id"),
        entry_id: row
            .get("entry_id")
            .ok_or_else(|| CommandError::InvalidInput("missing entry_id".to_string()))?,
        control_account: row
            .get("control_account")
            .ok_or_else(|| CommandError::InvalidInput("missing control_account".to_string()))?,
        currency: row
            .get("currency")
            .ok_or_else(|| CommandError::InvalidInput("missing currency".to_string()))?,
        rate_scaled: row.get("rate_scaled"),
        posting_date: row
            .get("posting_date")
            .ok_or_else(|| CommandError::InvalidInput("missing posting_date".to_string()))?,
        due_date: row.get("due_date"),
        tx_original_minor: row
            .get("tx_original_minor")
            .ok_or_else(|| CommandError::InvalidInput("missing tx_original_minor".to_string()))?,
        base_original_minor: row
            .get("base_original_minor")
            .ok_or_else(|| CommandError::InvalidInput("missing base_original_minor".to_string()))?,
        tx_remaining_minor: row
            .get("tx_remaining_minor")
            .ok_or_else(|| CommandError::InvalidInput("missing tx_remaining_minor".to_string()))?,
        base_remaining_minor: row.get("base_remaining_minor").ok_or_else(|| {
            CommandError::InvalidInput("missing base_remaining_minor".to_string())
        })?,
    })
}
