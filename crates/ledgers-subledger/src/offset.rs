//! The `subledger.offset` command: offset credit and debit items.

use crate::error::CONTROL_ACCOUNT_NOT_FOUND;
use crate::item::{ItemKind, Ledger, OpenItem, fx_difference, settle};
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

/// Input to the `subledger.offset` command.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OffsetInput {
    /// Ledger: ar or ap.
    pub ledger: String,
    /// Party ID (vendor in AP, customer in AR).
    pub party_id: String,
    /// Journal ID where the entry posts (if needed).
    pub journal_id: String,
    /// Posting date (ISO 8601).
    pub date: String,
    /// Credit item ID (negative, e.g., credit note or on-account).
    pub credit_item_id: String,
    /// Debit item ID (positive, e.g., invoice/bill).
    pub debit_item_id: String,
    /// Amount in transaction currency (minor units, positive).
    pub amount: i64,
    /// Optional description.
    pub description: Option<String>,
    /// Optional reference.
    pub reference: Option<String>,
}

/// Remaining balance after offset.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OffsetItemRemaining {
    /// Open item ID.
    pub open_item_id: String,
    /// Transaction remaining after offset.
    pub tx_remaining: i64,
    /// Base remaining after offset.
    pub base_remaining: i64,
}

/// Output of the `subledger.offset` command.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct OffsetOutput {
    /// The entry ID if an entry was posted (None if link-only).
    pub entry_id: Option<String>,
    /// Remaining balances for each item.
    pub items_remaining: Vec<OffsetItemRemaining>,
    /// Total FX difference (gain positive, loss negative).
    pub fx_difference_total: i64,
}

/// The `subledger.offset` command.
pub struct SubledgerOffset;

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Command for SubledgerOffset {
    const NAME: &'static str = "subledger.offset";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = true;
    const IDEMPOTENT: bool = false;
    const TARGET: Option<&'static str> = None;

    type Input = OffsetInput;
    type Output = OffsetOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Parse and validate input
        let _ledger = parse_ledger(&input.ledger)?;
        let _chart = load_chart(db, &ctx.company_id).await?;

        // Return an empty preview
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Parse and validate input
        let ledger = parse_ledger(&input.ledger)?;
        let chart = load_chart(db, &ctx.company_id).await?;

        // Resolve control account
        let control_account = chart
            .control_account(ledger)
            .ok_or_else(|| CommandError::Domain(Problem::new(&CONTROL_ACCOUNT_NOT_FOUND)))?
            .to_owned();

        // Load both items
        let credit_item = load_open_item(db, &input.credit_item_id).await?;
        let debit_item = load_open_item(db, &input.debit_item_id).await?;

        // Validate items belong to the same company, ledger, party, and currency
        if credit_item.company_id != ctx.company_id || debit_item.company_id != ctx.company_id {
            return Err(CommandError::InvalidInput(
                "items must be from the same company".to_string(),
            ));
        }

        if credit_item.ledger != ledger || debit_item.ledger != ledger {
            return Err(CommandError::InvalidInput(
                "items must be from the same ledger".to_string(),
            ));
        }

        if credit_item.party_id != input.party_id || debit_item.party_id != input.party_id {
            return Err(CommandError::InvalidInput(
                "items must be from the same party".to_string(),
            ));
        }

        if credit_item.currency != debit_item.currency {
            return Err(CommandError::InvalidInput(
                "items must have the same currency".to_string(),
            ));
        }

        // Settle both items
        let credit_base_cleared =
            settle(&credit_item, -input.amount).map_err(CommandError::from)?;
        let debit_base_cleared = settle(&debit_item, input.amount).map_err(CommandError::from)?;

        // Calculate FX difference
        let fx_diff = fx_difference(ledger, credit_base_cleared.abs(), debit_base_cleared);

        let mut statements = Vec::new();

        let entry_id = if credit_base_cleared.abs() == debit_base_cleared {
            // Link-only: no entry needed
            None
        } else {
            // Post an entry to handle the FX difference
            let fx_gain_account = chart
                .fx_gain_account()
                .ok_or_else(|| CommandError::InvalidInput("fx-gain account not found".to_string()))?
                .to_owned();
            let fx_loss_account = chart
                .fx_loss_account()
                .ok_or_else(|| CommandError::InvalidInput("fx-loss account not found".to_string()))?
                .to_owned();

            let mut entry_lines = Vec::new();
            let currency = Currency::from_code("EUR")
                .map_err(|e| CommandError::InvalidInput(format!("invalid currency: {e}")))?;

            // Credit item side
            let credit_amount = Amount::new(i128::from(credit_base_cleared.abs()), currency);
            match ledger {
                Ledger::Ar => {
                    entry_lines.push(SerializableLine {
                        account_number: control_account.clone(),
                        side: "debit".to_string(),
                        base: SerializableAmount {
                            minor: credit_amount.minor().to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    });
                }
                Ledger::Ap => {
                    entry_lines.push(SerializableLine {
                        account_number: control_account.clone(),
                        side: "credit".to_string(),
                        base: SerializableAmount {
                            minor: credit_amount.minor().to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    });
                }
            }

            // Debit item side
            let debit_amount = Amount::new(i128::from(debit_base_cleared), currency);
            match ledger {
                Ledger::Ar => {
                    entry_lines.push(SerializableLine {
                        account_number: control_account.clone(),
                        side: "credit".to_string(),
                        base: SerializableAmount {
                            minor: debit_amount.minor().to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    });
                }
                Ledger::Ap => {
                    entry_lines.push(SerializableLine {
                        account_number: control_account.clone(),
                        side: "debit".to_string(),
                        base: SerializableAmount {
                            minor: debit_amount.minor().to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    });
                }
            }

            // FX difference
            if fx_diff > 0 {
                let fx_amount = Amount::new(i128::from(fx_diff), currency);
                entry_lines.push(SerializableLine {
                    account_number: fx_gain_account,
                    side: "credit".to_string(),
                    base: SerializableAmount {
                        minor: fx_amount.minor().to_string(),
                        currency: "EUR".to_string(),
                    },
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });
            } else if fx_diff < 0 {
                let fx_amount = Amount::new(i128::from(-fx_diff), currency);
                entry_lines.push(SerializableLine {
                    account_number: fx_loss_account,
                    side: "debit".to_string(),
                    base: SerializableAmount {
                        minor: fx_amount.minor().to_string(),
                        currency: "EUR".to_string(),
                    },
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });
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
                        .unwrap_or_else(|| "Offset".to_owned()),
                    source_documents: Vec::new(),
                    external_id: Some(format!(
                        "offset-{}-{}-{}",
                        input.party_id, input.credit_item_id, input.debit_item_id
                    )),
                    source_app: Some("ledgers-subledger".to_owned()),
                    reverses: None,
                    lines: entry_lines,
                },
            };

            let posted = plan_post_entry_with_control(token, db, ctx, &entry_input).await?;
            statements = posted.statements;
            Some(posted.output.entry.entry_id.clone())
        };

        // Build allocation rows and open_item guards
        let allocation_id = format!("alloc_{}", ctx.event_id);

        // Credit item allocation
        {
            let alloc_id = format!("{allocation_id}_credit");
            let guard_key = guard::guard_key(&[
                "offset-credit",
                entry_id.as_deref().unwrap_or(""),
                &credit_item.id,
            ]);

            statements.push(guard::assert_statement(
                &guard_key,
                "(SELECT 1 FROM open_item WHERE id = ? AND company_id = ? AND ledger = ? \
                 AND party_id = ? AND tx_remaining_minor = ? AND base_remaining_minor = ?) IS NOT NULL",
                vec![
                    Value::from(credit_item.id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(ledger.to_string().as_str()),
                    Value::from(input.party_id.as_str()),
                    Value::from(credit_item.tx_remaining_minor),
                    Value::from(credit_item.base_remaining_minor),
                ],
            ));

            statements.push(Statement::with_values(
                "INSERT INTO allocation (id, company_id, open_item_id, entry_id, kind, counter_item_id, date, \
                 tx_minor, base_minor, fx_difference_minor, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                    .to_owned(),
                vec![
                    Value::from(alloc_id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(credit_item.id.as_str()),
                    entry_id.as_ref().map_or(Value::String(None), |e| Value::from(e.as_str())),
                    Value::from("offset"),
                    Value::from(debit_item.id.as_str()),
                    Value::from(input.date.as_str()),
                    Value::from(-input.amount),
                    Value::from(-credit_base_cleared),
                    Value::from(0i64), // No FX on the offset itself, FX is in the entry
                    Value::from(ctx.now.as_str()),
                ],
            ));

            statements.push(Statement::with_values(
                "UPDATE open_item SET tx_remaining_minor = tx_remaining_minor + ?, \
                 base_remaining_minor = base_remaining_minor + ?, updated_at = ? \
                 WHERE id = ? AND company_id = ? AND ledger = ? AND party_id = ?"
                    .to_owned(),
                vec![
                    Value::from(input.amount),
                    Value::from(credit_base_cleared),
                    Value::from(ctx.now.as_str()),
                    Value::from(credit_item.id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(ledger.to_string().as_str()),
                    Value::from(input.party_id.as_str()),
                ],
            ));

            statements.push(guard::release_statement(&guard_key));
        }

        // Debit item allocation
        {
            let alloc_id = format!("{allocation_id}_debit");
            let guard_key = guard::guard_key(&[
                "offset-debit",
                entry_id.as_deref().unwrap_or(""),
                &debit_item.id,
            ]);

            statements.push(guard::assert_statement(
                &guard_key,
                "(SELECT 1 FROM open_item WHERE id = ? AND company_id = ? AND ledger = ? \
                 AND party_id = ? AND tx_remaining_minor = ? AND base_remaining_minor = ?) IS NOT NULL",
                vec![
                    Value::from(debit_item.id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(ledger.to_string().as_str()),
                    Value::from(input.party_id.as_str()),
                    Value::from(debit_item.tx_remaining_minor),
                    Value::from(debit_item.base_remaining_minor),
                ],
            ));

            statements.push(Statement::with_values(
                "INSERT INTO allocation (id, company_id, open_item_id, entry_id, kind, counter_item_id, date, \
                 tx_minor, base_minor, fx_difference_minor, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                    .to_owned(),
                vec![
                    Value::from(alloc_id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(debit_item.id.as_str()),
                    entry_id.as_ref().map_or(Value::String(None), |e| Value::from(e.as_str())),
                    Value::from("offset"),
                    Value::from(credit_item.id.as_str()),
                    Value::from(input.date.as_str()),
                    Value::from(input.amount),
                    Value::from(debit_base_cleared),
                    Value::from(0i64), // No FX on the offset itself
                    Value::from(ctx.now.as_str()),
                ],
            ));

            statements.push(Statement::with_values(
                "UPDATE open_item SET tx_remaining_minor = tx_remaining_minor - ?, \
                 base_remaining_minor = base_remaining_minor - ?, updated_at = ? \
                 WHERE id = ? AND company_id = ? AND ledger = ? AND party_id = ?"
                    .to_owned(),
                vec![
                    Value::from(input.amount),
                    Value::from(debit_base_cleared),
                    Value::from(ctx.now.as_str()),
                    Value::from(debit_item.id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(ledger.to_string().as_str()),
                    Value::from(input.party_id.as_str()),
                ],
            ));

            statements.push(guard::release_statement(&guard_key));
        }

        // Build output
        let items_remaining = vec![
            OffsetItemRemaining {
                open_item_id: credit_item.id.clone(),
                tx_remaining: credit_item.tx_remaining_minor + input.amount,
                base_remaining: credit_item.base_remaining_minor + credit_base_cleared,
            },
            OffsetItemRemaining {
                open_item_id: debit_item.id.clone(),
                tx_remaining: debit_item.tx_remaining_minor - input.amount,
                base_remaining: debit_item.base_remaining_minor - debit_base_cleared,
            },
        ];

        let mut plan = Plan::with_output(OffsetOutput {
            entry_id,
            items_remaining,
            fx_difference_total: fx_diff,
        });
        plan.statements = statements;
        plan.objects = vec![input.party_id.clone()];

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
    fx_gain: Option<String>,
    fx_loss: Option<String>,
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
                if chart.fx_gain.is_none() && info.tags.iter().any(|t| t == "fx-gain") {
                    chart.fx_gain = Some(account.number.clone());
                }
                if chart.fx_loss.is_none() && info.tags.iter().any(|t| t == "fx-loss") {
                    chart.fx_loss = Some(account.number.clone());
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

    fn fx_gain_account(&self) -> Option<&str> {
        self.fx_gain.as_deref()
    }

    fn fx_loss_account(&self) -> Option<&str> {
        self.fx_loss.as_deref()
    }
}

async fn load_chart(db: &dyn Database, company_id: &str) -> Result<Chart, CommandError> {
    let entries = ledgers_ledger::commands::list_accounts(db, company_id, true)
        .await
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    Ok(Chart::from_entries(&entries))
}

/// Parse ledger string into Ledger enum.
fn parse_ledger(s: &str) -> Result<Ledger, CommandError> {
    match s.to_lowercase().as_str() {
        "ar" => Ok(Ledger::Ar),
        "ap" => Ok(Ledger::Ap),
        _ => Err(CommandError::InvalidInput(
            "ledger must be 'ar' or 'ap'".to_owned(),
        )),
    }
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
            "invoice" => ItemKind::Invoice,
            "credit-note" => ItemKind::CreditNote,
            "bill" => ItemKind::Bill,
            "on-account" => ItemKind::OnAccount,
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
