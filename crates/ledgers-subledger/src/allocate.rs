//! The `subledger.allocate` command: payment/receipt allocation to open items.

use crate::builders;
use crate::error::CONTROL_ACCOUNT_NOT_FOUND;
use crate::item::{ItemKind, Ledger, OpenItem, SubledgerError, fx_difference, settle};
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

impl From<SubledgerError> for CommandError {
    fn from(err: SubledgerError) -> Self {
        match err {
            SubledgerError::ExceedsRemaining => {
                CommandError::Domain(Problem::new(&crate::error::ALLOCATION_EXCEEDS_OPEN_ITEM))
            }
            other => CommandError::InvalidInput(other.to_string()),
        }
    }
}

/// Input to the `subledger.allocate` command.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AllocateInput {
    /// Ledger: ar or ap.
    pub ledger: String,
    /// Party ID (vendor in AP, customer in AR).
    pub party_id: String,
    /// Journal ID where the entry posts.
    pub journal_id: String,
    /// Posting date (ISO 8601).
    pub date: String,
    /// Bank account (must be tagged 'bank').
    pub bank_account: String,
    /// Amount paid in base currency (minor units, positive).
    pub paid: i64,
    /// Items to allocate.
    pub items: Vec<AllocateItem>,
    /// Optional description.
    pub description: Option<String>,
    /// Optional reference.
    pub reference: Option<String>,
}

/// One item in an allocation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AllocateItem {
    /// Open item ID.
    pub open_item_id: String,
    /// Amount applied in transaction currency (minor units, positive).
    pub amount: i64,
    /// Amount paid in base currency (minor units). Required when item currency ≠ base.
    /// Defaults to amount when equal.
    pub paid_base: Option<i64>,
}

/// Remaining balance after allocation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AllocateItemRemaining {
    /// Open item ID.
    pub open_item_id: String,
    /// Transaction remaining after allocation.
    pub tx_remaining: i64,
    /// Base remaining after allocation.
    pub base_remaining: i64,
}

/// Output of the `subledger.allocate` command.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AllocateOutput {
    /// The entry ID.
    pub entry_id: String,
    /// Remaining balances for each item.
    pub items_remaining: Vec<AllocateItemRemaining>,
    /// The on-account item ID if prepayment or overpayment.
    pub on_account_item_id: Option<String>,
    /// Total FX difference (gain positive, loss negative).
    pub fx_difference_total: i64,
}

/// The `subledger.allocate` command.
pub struct SubledgerAllocate;

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Command for SubledgerAllocate {
    const NAME: &'static str = "subledger.allocate";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = true;
    const IDEMPOTENT: bool = false;
    const TARGET: Option<&'static str> = None;

    type Input = AllocateInput;
    type Output = AllocateOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Parse and validate input
        let _ledger = parse_ledger(&input.ledger)?;
        let _chart = load_chart(db, &ctx.company_id).await?;

        // Return an empty preview - the actual posting happens in plan()
        // (preview is mostly a validation step)
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

        // Resolve FX accounts
        let fx_gain_account = chart
            .fx_gain_account()
            .ok_or_else(|| CommandError::InvalidInput("fx-gain account not found".to_string()))?
            .to_owned();
        let fx_loss_account = chart
            .fx_loss_account()
            .ok_or_else(|| CommandError::InvalidInput("fx-loss account not found".to_string()))?
            .to_owned();

        // Load items
        let mut items_to_allocate: Vec<(OpenItem, i64, Option<i64>)> = Vec::new();
        let mut total_paid_base = 0i64;
        let mut fx_difference_total = 0i64;

        for item_input in &input.items {
            // Load item from DB
            let item = load_open_item(db, &item_input.open_item_id).await?;

            // Verify item belongs to this company/ledger/party
            if item.company_id != ctx.company_id
                || item.ledger != ledger
                || item.party_id != input.party_id
            {
                return Err(CommandError::InvalidInput(format!(
                    "item {} not found for this party/ledger",
                    item_input.open_item_id
                )));
            }

            // Settle and compute FX
            let base_cleared = settle(&item, item_input.amount).map_err(CommandError::from)?;

            // Determine paid_base: use provided value if item is non-base, otherwise use amount
            let paid_base = if let Some(pb) = item_input.paid_base {
                pb
            } else {
                item_input.amount
            };

            let fx = fx_difference(ledger, paid_base, base_cleared);
            fx_difference_total += fx;
            total_paid_base += paid_base;

            items_to_allocate.push((item, item_input.amount, Some(paid_base)));
        }

        // Compute excess
        let excess = input.paid - total_paid_base;
        if excess < 0 {
            return Err(CommandError::Domain(Problem::new(
                &crate::error::ALLOCATION_EXCEEDS_OPEN_ITEM,
            )));
        }

        // Build entry
        let mut entry_lines = Vec::new();

        let currency = Currency::from_code("EUR")
            .map_err(|e| CommandError::InvalidInput(format!("invalid currency: {e}")))?;
        let paid_amount = Amount::new(i128::from(input.paid), currency);

        match ledger {
            Ledger::Ar => {
                // AR receipt: Dr bank paid, Cr control base_cleared per item, Cr control excess, Cr/Dr FX
                entry_lines.push(SerializableLine {
                    account_number: input.bank_account.clone(),
                    side: "debit".to_string(),
                    base: SerializableAmount {
                        minor: paid_amount.minor().to_string(),
                        currency: "EUR".to_string(),
                    },
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });

                // Cr control for each settled item
                for (item, tx_amount, _paid_base) in &items_to_allocate {
                    let base_cleared = settle(item, *tx_amount)?;
                    let base_cleared_amount = Amount::new(i128::from(base_cleared), currency);
                    entry_lines.push(SerializableLine {
                        account_number: control_account.clone(),
                        side: "credit".to_string(),
                        base: SerializableAmount {
                            minor: base_cleared_amount.minor().to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    });
                }

                // Cr control for excess (on-account)
                if excess > 0 {
                    let excess_amount = Amount::new(i128::from(excess), currency);
                    entry_lines.push(SerializableLine {
                        account_number: control_account.clone(),
                        side: "credit".to_string(),
                        base: SerializableAmount {
                            minor: excess_amount.minor().to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    });
                }

                // FX gain/loss
                if fx_difference_total > 0 {
                    // Cr fx-gain
                    let fx_amount = Amount::new(i128::from(fx_difference_total), currency);
                    entry_lines.push(SerializableLine {
                        account_number: fx_gain_account.clone(),
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
                } else if fx_difference_total < 0 {
                    // Dr fx-loss
                    let fx_amount = Amount::new(i128::from(-fx_difference_total), currency);
                    entry_lines.push(SerializableLine {
                        account_number: fx_loss_account.clone(),
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
            }
            Ledger::Ap => {
                // AP payment: Cr bank paid, Dr control base_cleared per item, Dr control excess, Cr/Dr FX
                entry_lines.push(SerializableLine {
                    account_number: input.bank_account.clone(),
                    side: "credit".to_string(),
                    base: SerializableAmount {
                        minor: paid_amount.minor().to_string(),
                        currency: "EUR".to_string(),
                    },
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });

                // Dr control for each settled item
                for (item, tx_amount, _paid_base) in &items_to_allocate {
                    let base_cleared = settle(item, *tx_amount)?;
                    let base_cleared_amount = Amount::new(i128::from(base_cleared), currency);
                    entry_lines.push(SerializableLine {
                        account_number: control_account.clone(),
                        side: "debit".to_string(),
                        base: SerializableAmount {
                            minor: base_cleared_amount.minor().to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    });
                }

                // Dr control for excess (on-account)
                if excess > 0 {
                    let excess_amount = Amount::new(i128::from(excess), currency);
                    entry_lines.push(SerializableLine {
                        account_number: control_account.clone(),
                        side: "debit".to_string(),
                        base: SerializableAmount {
                            minor: excess_amount.minor().to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    });
                }

                // FX gain/loss
                if fx_difference_total > 0 {
                    // Cr fx-gain
                    let fx_amount = Amount::new(i128::from(fx_difference_total), currency);
                    entry_lines.push(SerializableLine {
                        account_number: fx_gain_account.clone(),
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
                } else if fx_difference_total < 0 {
                    // Dr fx-loss
                    let fx_amount = Amount::new(i128::from(-fx_difference_total), currency);
                    entry_lines.push(SerializableLine {
                        account_number: fx_loss_account.clone(),
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
                description: input.description.clone().unwrap_or_default(),
                source_documents: Vec::new(),
                external_id: Some(format!("alloc-{}-{}", input.party_id, ctx.event_id)),
                source_app: Some("ledgers-subledger".to_owned()),
                reverses: None,
                lines: entry_lines,
            },
        };

        let mut posted = plan_post_entry_with_control(token, db, ctx, &entry_input).await?;
        let entry_id = posted.output.entry.entry_id.clone();

        // Generate allocation ID
        let allocation_id = format!("alloc_{}", ctx.event_id);

        // Build allocation rows and open_item update guard statements
        for (i, (item, tx_amount, paid_base)) in items_to_allocate.iter().enumerate() {
            let base_cleared = settle(item, *tx_amount)?;

            // Insert allocation row
            let alloc_id = format!("{allocation_id}_{i}");
            let fx = fx_difference(ledger, paid_base.unwrap_or(*tx_amount), base_cleared);

            posted.statements.push(builders::build_allocation_insert(
                &alloc_id,
                &ctx.company_id,
                &item.id,
                &entry_id,
                "payment",
                *tx_amount,
                base_cleared,
                fx,
                &input.date,
                &ctx.now,
            ));

            // Guard: verify that the item still has the expected remaining values
            // (no concurrent allocation has changed them since plan time).
            let guard_key = guard::guard_key(&["alloc-item", entry_id.as_str(), item.id.as_str()]);
            posted.statements.push(guard::assert_statement(
                &guard_key,
                "(SELECT 1 FROM open_item WHERE id = ? AND company_id = ? AND ledger = ? \n\
                 AND party_id = ? AND tx_remaining_minor = ? AND base_remaining_minor = ?) IS NOT NULL",
                vec![
                    Value::from(item.id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(ledger.to_string().as_str()),
                    Value::from(input.party_id.as_str()),
                    Value::from(item.tx_remaining_minor),
                    Value::from(item.base_remaining_minor),
                ],
            ));

            // Update open_item with relative decrements
            posted.statements.push(Statement::with_values(
                "UPDATE open_item SET tx_remaining_minor = tx_remaining_minor - ?, \
                 base_remaining_minor = base_remaining_minor - ?, updated_at = ? \
                 WHERE id = ? AND company_id = ? AND ledger = ? AND party_id = ?"
                    .to_owned(),
                vec![
                    Value::from(*tx_amount),
                    Value::from(base_cleared),
                    Value::from(ctx.now.as_str()),
                    Value::from(item.id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(ledger.to_string().as_str()),
                    Value::from(input.party_id.as_str()),
                ],
            ));

            // Release the guard
            posted.statements.push(guard::release_statement(&guard_key));
        }

        // Handle on-account item if excess > 0 or items empty
        let on_account_item_id = if excess > 0 || input.items.is_empty() {
            let oa_id = format!("oa_{}", ctx.event_id);
            posted.statements.push(Statement::with_values(
                "INSERT INTO open_item (id, company_id, ledger, kind, party_id, source_id, entry_id, \
                 control_account, currency, rate_scaled, document_date, posting_date, due_date, \
                 tx_original_minor, base_original_minor, tx_remaining_minor, base_remaining_minor, \
                 created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                    .to_owned(),
                vec![
                    Value::from(oa_id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(ledger.to_string().as_str()),
                    Value::from("on-account"),
                    Value::from(input.party_id.as_str()),
                    Value::from(None::<String>),
                    Value::from(entry_id.as_str()),
                    Value::from(control_account.as_str()),
                    Value::from("EUR"),
                    Value::from(None::<String>),
                    Value::from(input.date.as_str()),
                    Value::from(input.date.as_str()),
                    Value::from(None::<String>),
                    Value::from(-excess),
                    Value::from(-excess),
                    Value::from(-excess),
                    Value::from(-excess),
                    Value::from(ctx.now.as_str()),
                    Value::from(ctx.now.as_str()),
                ],
            ));
            Some(oa_id)
        } else {
            None
        };

        // Build output
        let items_remaining = items_to_allocate
            .iter()
            .map(|(item, tx_amount, _)| {
                let base_cleared = settle(item, *tx_amount).unwrap_or(0);
                AllocateItemRemaining {
                    open_item_id: item.id.clone(),
                    tx_remaining: item.tx_remaining_minor - tx_amount,
                    base_remaining: item.base_remaining_minor - base_cleared,
                }
            })
            .collect();

        let mut plan = Plan::with_output(AllocateOutput {
            entry_id,
            items_remaining,
            on_account_item_id,
            fx_difference_total,
        });
        plan.statements = posted.statements;
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
        .query(&Statement::with_values(
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
