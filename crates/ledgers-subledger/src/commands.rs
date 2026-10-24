//! Subledger commands: posting items, allocations, reconciliation.

pub use crate::allocate::{
    AllocateInput, AllocateItem, AllocateItemRemaining, AllocateOutput, SubledgerAllocate,
};
pub use crate::offset::{OffsetInput, OffsetItemRemaining, OffsetOutput, SubledgerOffset};
pub use crate::reconcile_command::{ReconcileInput, ReconcileOutput, SubledgerReconcile};
pub use crate::write_off::{SubledgerWriteOff, WriteOffInput, WriteOffOutput};

use crate::error::CONTROL_ACCOUNT_NOT_FOUND;
use crate::item::{ItemKind, Ledger};
use async_trait::async_trait;
use cratefield_core::{Database, Problem, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_ledger::account::AccountEntry;
use ledgers_ledger::surface::{
    PostEntry, PostEntryInput, SerializableAmount, SerializableEntryDraft, SerializableLine,
    plan_post_entry_with_control,
};
use ledgers_money::{Amount, Currency, Rate, Rounding};
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The chart tag for receivables (AR control account).
const TAG_AR: &str = "ar";
/// The chart tag for payables (AP control account).
const TAG_AP: &str = "ap";

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
    fn from_entries(entries: &[AccountEntry]) -> Self {
        let mut chart = Chart::default();
        for entry in entries {
            let account = &entry.account;
            let info = AccountInfo {
                postable: entry.is_postable(),
                tags: account.tags.clone(),
            };
            if !account.archived {
                if chart.ar_control.is_none() && info.tags.iter().any(|t| t == TAG_AR) {
                    chart.ar_control = Some(account.number.clone());
                }
                if chart.ap_control.is_none() && info.tags.iter().any(|t| t == TAG_AP) {
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

/// Input to the `subledger.post-item` command.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostItemInput {
    /// Ledger: ar or ap.
    pub ledger: String,
    /// Kind: invoice, credit-note, bill, or on-account.
    pub kind: String,
    /// Party ID (vendor in AP, customer in AR).
    pub party_id: String,
    /// Journal ID where the entry posts.
    pub journal_id: String,
    /// Document date (ISO 8601).
    pub document_date: String,
    /// Posting date (ISO 8601).
    pub posting_date: String,
    /// Due date (ISO 8601), optional.
    pub due_date: Option<String>,
    /// Transaction currency.
    pub currency: String,
    /// Amount in transaction currency (positive minor units).
    pub amount: i64,
    /// Exchange rate (decimal string), required when currency != base.
    pub rate: Option<String>,
    /// Counter-account to post against (e.g., 6000 for expense).
    pub counter_account: String,
    /// Dimension values on the counter-account line (e.g. a cost centre on an
    /// expense), in the entry's dimension kinds.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dimensions: BTreeMap<String, String>,
    /// Description for the entry.
    pub description: String,
    /// Optional source reference (e.g., invoice number).
    pub source_id: Option<String>,
}

/// Output of the `subledger.post-item` command.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct PostItemOutput {
    /// The ID of the open item created.
    pub item_id: String,
    /// The entry that was posted.
    pub entry_id: String,
}

/// The `subledger.post-item` command.
pub struct SubledgerPostItem;

#[async_trait]
impl Command for SubledgerPostItem {
    const NAME: &'static str = "subledger.post-item";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = true;
    const IDEMPOTENT: bool = false;
    const TARGET: Option<&'static str> = None;

    type Input = PostItemInput;
    type Output = PostItemOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Parse and validate input
        let ledger = parse_ledger(&input.ledger)?;
        let kind = parse_kind(&input.kind)?;
        validate_ledger_kind(ledger, kind)?;
        let chart = load_chart(db, &ctx.company_id).await?;

        // Resolve control account
        let control_account = chart
            .control_account(ledger)
            .ok_or_else(|| CommandError::Domain(Problem::new(&CONTROL_ACCOUNT_NOT_FOUND)))?;

        // Build entry draft
        let entry_input = build_entry_input(
            input,
            ledger,
            kind,
            control_account,
            &ctx.company_id,
            &ctx.event_id,
        )?;

        // Preview through the ledger
        PostEntry.preview(db, ctx, &entry_input).await
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
        let kind = parse_kind(&input.kind)?;

        // Validate ledger/kind combination
        validate_ledger_kind(ledger, kind)?;
        let chart = load_chart(db, &ctx.company_id).await?;

        // Resolve control account
        let control_account = chart
            .control_account(ledger)
            .ok_or_else(|| CommandError::Domain(Problem::new(&CONTROL_ACCOUNT_NOT_FOUND)))?
            .to_owned();

        // Check for duplicate source_id if provided
        if let Some(ref source_id) = input.source_id {
            let exists = check_duplicate_source_id(db, &ctx.company_id, &kind, source_id).await?;
            if exists {
                return Err(CommandError::Domain(
                    Problem::new(&crate::error::DUPLICATE_SOURCE_ID)
                        .with_detail(format!("source_id {source_id} already used")),
                ));
            }
        }

        // Build entry and post it
        let entry_input = build_entry_input(
            input,
            ledger,
            kind,
            &control_account,
            &ctx.company_id,
            &ctx.event_id,
        )?;

        let mut posted = plan_post_entry_with_control(token, db, ctx, &entry_input).await?;

        // Generate item ID and build the open_item INSERT
        let item_id = format!("oi_{}", ctx.event_id);
        let entry_id = posted.output.entry.entry_id.clone();
        let (tx_original, base_original, rate_scaled) = compute_amounts(input, &entry_input)?;

        let item_insert = build_item_insert(
            &item_id,
            &ctx.company_id,
            ledger,
            kind,
            input,
            &entry_id,
            &control_account,
            tx_original,
            base_original,
            rate_scaled,
            &ctx.now,
        );

        // Append the insert to statements
        posted.statements.push(item_insert);

        let mut plan = Plan::with_output(PostItemOutput { item_id, entry_id });
        plan.statements = posted.statements;
        plan.objects = vec![input.party_id.clone()];

        Ok(plan)
    }
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

/// Parse kind string into `ItemKind` enum.
fn parse_kind(s: &str) -> Result<ItemKind, CommandError> {
    match s.to_lowercase().as_str() {
        "invoice" => Ok(ItemKind::Invoice),
        "credit-note" | "creditnote" => Ok(ItemKind::CreditNote),
        "bill" => Ok(ItemKind::Bill),
        "on-account" | "onaccount" => Ok(ItemKind::OnAccount),
        _ => Err(CommandError::InvalidInput(
            "kind must be 'invoice', 'credit-note', 'bill', or 'on-account'".to_owned(),
        )),
    }
}

/// Validates that ledger and kind are compatible.
fn validate_ledger_kind(ledger: Ledger, kind: ItemKind) -> Result<(), CommandError> {
    match (ledger, kind) {
        (Ledger::Ar, ItemKind::Invoice | ItemKind::CreditNote | ItemKind::OnAccount)
        | (Ledger::Ap, ItemKind::Bill | ItemKind::CreditNote | ItemKind::OnAccount) => Ok(()),
        (Ledger::Ap, ItemKind::Invoice) => {
            // In AP, we use "bill" not "invoice"
            Err(CommandError::InvalidInput(
                "AP uses 'bill' not 'invoice'".to_owned(),
            ))
        }
        (Ledger::Ar, ItemKind::Bill) => {
            // In AR, we use "invoice" not "bill"
            Err(CommandError::InvalidInput(
                "AR uses 'invoice' not 'bill'".to_owned(),
            ))
        }
    }
}

/// Builds the entry input for posting.
#[allow(clippy::too_many_lines)]
fn build_entry_input(
    input: &PostItemInput,
    ledger: Ledger,
    kind: ItemKind,
    control_account: &str,
    _company_id: &str,
    event_id: &str,
) -> Result<PostEntryInput, CommandError> {
    // Parse currency
    let currency = Currency::from_code(&input.currency)
        .map_err(|e| CommandError::InvalidInput(format!("invalid currency: {e}")))?;

    // The sign below is kept for documentation: it shows the intended sign for the amount
    // in the ledger (positive or negative), which is applied via side selection in the
    // entry lines (debit vs. credit).
    let _sign = match (ledger, kind) {
        (Ledger::Ar, ItemKind::Invoice)
        | (Ledger::Ap, ItemKind::Invoice | ItemKind::Bill | ItemKind::OnAccount) => 1,
        (Ledger::Ar, ItemKind::CreditNote | ItemKind::OnAccount)
        | (Ledger::Ap, ItemKind::CreditNote) => -1,
        (Ledger::Ar, ItemKind::Bill) => unreachable!("Bill should only be in AP"),
    };

    let tx_minor = i128::from(input.amount);
    let tx_amount = Amount::new(tx_minor, currency);

    // Parse rate if provided
    let rate = if input.currency.to_uppercase() == "EUR" {
        // Assume EUR is base currency
        Rate::ONE
    } else {
        let rate_str = input.rate.as_deref().ok_or_else(|| {
            CommandError::InvalidInput("rate required for non-base currency".to_owned())
        })?;
        Rate::parse(rate_str)
            .map_err(|e| CommandError::InvalidInput(format!("invalid rate: {e}")))?
    };

    let base_amount = if rate == Rate::ONE {
        tx_amount
    } else {
        tx_amount
            .mul_rate(rate, Rounding::HalfEven)
            .map_err(|e| CommandError::InvalidInput(format!("rate calculation failed: {e}")))?
    };

    // Build lines based on ledger and kind
    let mut lines = Vec::new();

    match (ledger, kind) {
        (Ledger::Ar, ItemKind::Invoice) => {
            // Dr control (receivables) / Cr counter (revenue)
            lines.push(build_line(
                control_account,
                "debit",
                tx_amount,
                base_amount,
                rate,
                currency,
            ));
            lines.push(build_line(
                &input.counter_account,
                "credit",
                tx_amount,
                base_amount,
                rate,
                currency,
            ));
        }
        (Ledger::Ar, ItemKind::CreditNote) => {
            // Cr control (receivables) / Dr counter (revenue)
            lines.push(build_line(
                control_account,
                "credit",
                tx_amount,
                base_amount,
                rate,
                currency,
            ));
            lines.push(build_line(
                &input.counter_account,
                "debit",
                tx_amount,
                base_amount,
                rate,
                currency,
            ));
        }
        (Ledger::Ap, ItemKind::Invoice | ItemKind::Bill) => {
            // Dr counter (expense) / Cr control (payables)
            lines.push(build_line(
                &input.counter_account,
                "debit",
                tx_amount,
                base_amount,
                rate,
                currency,
            ));
            lines.push(build_line(
                control_account,
                "credit",
                tx_amount,
                base_amount,
                rate,
                currency,
            ));
        }
        (Ledger::Ap, ItemKind::CreditNote) => {
            // Reversed: Cr counter (expense) / Dr control (payables)
            lines.push(build_line(
                &input.counter_account,
                "credit",
                tx_amount,
                base_amount,
                rate,
                currency,
            ));
            lines.push(build_line(
                control_account,
                "debit",
                tx_amount,
                base_amount,
                rate,
                currency,
            ));
        }
        (Ledger::Ar | Ledger::Ap, ItemKind::OnAccount) => {
            // On-account: just a control account entry
            // Dr for AP, Cr for AR
            let side = match ledger {
                Ledger::Ar => "credit",
                Ledger::Ap => "debit",
            };
            lines.push(build_line(
                control_account,
                side,
                tx_amount,
                base_amount,
                rate,
                currency,
            ));
        }
        (Ledger::Ar, ItemKind::Bill) => unreachable!("Bill should only be in AP"),
    }

    // The requested dimensions belong on the counter-account line only; the
    // control line stays untagged, as the open item is per party.
    for line in &mut lines {
        if line.account_number == input.counter_account {
            line.dimensions = input.dimensions.clone();
        }
    }

    let fiscal_year = input
        .posting_date
        .get(..4)
        .and_then(|y| y.parse::<i64>().ok())
        .ok_or_else(|| CommandError::InvalidInput("invalid posting_date year".to_owned()))?;

    Ok(PostEntryInput {
        journal_id: input.journal_id.clone(),
        fiscal_year,
        entry: SerializableEntryDraft {
            journal_id: input.journal_id.clone(),
            document_date: input.document_date.clone(),
            posting_date: input.posting_date.clone(),
            tax_point_date: None,
            base_currency: "EUR".to_owned(), // TODO: derive from company config
            description: input.description.clone(),
            source_documents: Vec::new(),
            external_id: Some(format!("item-{event_id}")),
            source_app: Some("ledgers-subledger".to_owned()),
            reverses: None,
            lines,
        },
    })
}

/// Builds a journal entry line.
fn build_line(
    account: &str,
    side: &str,
    tx_amount: Amount,
    base_amount: Amount,
    rate: Rate,
    currency: Currency,
) -> SerializableLine {
    use ledgers_ledger::surface::SerializableFx;

    // If the transaction is in base currency (rate = 1), no FX
    let fx = if rate == Rate::ONE {
        None
    } else {
        Some(SerializableFx {
            transaction: SerializableAmount {
                minor: tx_amount.minor().to_string(),
                currency: currency.code().to_string(),
            },
            rate: rate.to_string(),
        })
    };

    SerializableLine {
        account_number: account.to_owned(),
        side: side.to_owned(),
        base: SerializableAmount {
            minor: base_amount.minor().to_string(),
            currency: base_amount.currency().code().to_string(),
        },
        fx,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    }
}

/// Checks if a `source_id` already exists for this company+kind.
async fn check_duplicate_source_id(
    db: &dyn Database,
    company_id: &str,
    kind: &ItemKind,
    source_id: &str,
) -> Result<bool, CommandError> {
    let stmt = Statement::with_values(
        "SELECT 1 FROM open_item WHERE company_id = ? AND kind = ? AND source_id = ? LIMIT 1"
            .to_owned(),
        vec![
            Value::from(company_id),
            Value::from(kind.to_string().as_str()),
            Value::from(source_id),
        ],
    );
    let rows = db
        .query(&stmt)
        .await
        .map_err(|e| CommandError::InvalidInput(e.to_string()))?;

    Ok(!rows.rows.is_empty())
}

/// Computes the original amounts (transaction and base).
fn compute_amounts<'a>(
    input: &'a PostItemInput,
    _entry_input: &'a PostEntryInput,
) -> Result<(i64, i64, Option<&'a str>), CommandError> {
    let currency = Currency::from_code(&input.currency)
        .map_err(|e| CommandError::InvalidInput(format!("invalid currency: {e}")))?;

    let tx_minor = input.amount;

    // Compute base amount
    let base_minor = if input.currency.to_uppercase() == "EUR" {
        tx_minor
    } else {
        let rate_str = input.rate.as_deref().ok_or_else(|| {
            CommandError::InvalidInput("rate required for non-base currency".to_owned())
        })?;
        let rate = Rate::parse(rate_str)
            .map_err(|e| CommandError::InvalidInput(format!("invalid rate: {e}")))?;
        let tx_amount = Amount::new(i128::from(tx_minor), currency);
        let base = tx_amount
            .mul_rate(rate, Rounding::HalfEven)
            .map_err(|e| CommandError::InvalidInput(format!("rate calculation failed: {e}")))?;
        #[allow(clippy::cast_possible_truncation)]
        {
            base.minor() as i64
        }
    };

    let rate_scaled = input.rate.as_deref();
    Ok((tx_minor, base_minor, rate_scaled))
}

/// Builds the INSERT statement for the `open_item` row.
#[allow(clippy::too_many_arguments)]
fn build_item_insert(
    item_id: &str,
    company_id: &str,
    ledger: Ledger,
    kind: ItemKind,
    input: &PostItemInput,
    entry_id: &str,
    control_account: &str,
    tx_original: i64,
    base_original: i64,
    rate_scaled: Option<&str>,
    now: &str,
) -> Statement {
    // Determine sign for storage based on ledger and kind
    let (tx_minor, base_minor) = match (ledger, kind) {
        (Ledger::Ar, ItemKind::CreditNote | ItemKind::OnAccount)
        | (Ledger::Ap, ItemKind::CreditNote) => (-tx_original, -base_original),
        (Ledger::Ar, ItemKind::Invoice)
        | (Ledger::Ap, ItemKind::Invoice | ItemKind::Bill | ItemKind::OnAccount) => {
            (tx_original, base_original)
        }
        (Ledger::Ar, ItemKind::Bill) => unreachable!("Bill should only be in AP"),
    };

    Statement::with_values(
        "INSERT INTO open_item (id, company_id, ledger, kind, party_id, source_id, entry_id, \
         control_account, currency, rate_scaled, document_date, posting_date, due_date, \
         tx_original_minor, base_original_minor, tx_remaining_minor, base_remaining_minor, \
         created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            Value::from(item_id),
            Value::from(company_id),
            Value::from(ledger.to_string().as_str()),
            Value::from(kind.to_string().as_str()),
            Value::from(input.party_id.as_str()),
            input
                .source_id
                .as_ref()
                .map_or_else(|| Value::String(None), |s| Value::from(s.as_str())),
            Value::from(entry_id),
            Value::from(control_account),
            Value::from(input.currency.as_str()),
            rate_scaled.map_or_else(|| Value::String(None), Value::from),
            Value::from(input.document_date.as_str()),
            Value::from(input.posting_date.as_str()),
            input
                .due_date
                .as_ref()
                .map_or_else(|| Value::String(None), |d| Value::from(d.as_str())),
            Value::from(tx_minor),
            Value::from(base_minor),
            Value::from(tx_minor),
            Value::from(base_minor),
            Value::from(now),
            Value::from(now),
        ],
    )
}
