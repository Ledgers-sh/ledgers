//! `parties.open_items`: what a party still owes or is owed, from the
//! subledger. A receivable (`ar`) is what a customer owes the company; a
//! payable (`ap`) is what the company owes a supplier. The same party id names
//! both (see `ledgers-bills`' parties), so one read serves the detail page.

use cratefield_core::{Database, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};

/// The input of `parties.open_items`: the party id.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenItemsInput {
    /// The party id (from the path).
    pub id: String,
}

/// One open item of a party, still partly or wholly unpaid.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OpenItemView {
    /// The open item id.
    pub id: String,
    /// `ar` for a receivable, `ap` for a payable.
    pub ledger: String,
    /// `invoice`, `credit-note`, `bill` or `on-account`.
    pub kind: String,
    /// The document or invoice number, when the item has one.
    pub document: Option<String>,
    /// The posting date (YYYY-MM-DD).
    pub posting_date: String,
    /// The due date, when there is one.
    pub due_date: Option<String>,
    /// The currency code.
    pub currency: String,
    /// The original amount, in minor units.
    pub original_minor: i64,
    /// The amount still open, in minor units.
    pub remaining_minor: i64,
}

/// The total still open for a party, per ledger and currency.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PartyBalance {
    /// `ar` or `ap`.
    pub ledger: String,
    /// The currency code.
    pub currency: String,
    /// The amount still open, in minor units.
    pub remaining_minor: i64,
}

/// The output of `parties.open_items`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OpenItemsOutput {
    /// The open items, oldest first.
    pub open_items: Vec<OpenItemView>,
    /// The totals still open, per ledger and currency.
    pub balances: Vec<PartyBalance>,
}

/// `parties.open_items`: a party's open receivables and payables.
pub struct PartiesOpenItems;

#[async_trait::async_trait]
impl Command for PartiesOpenItems {
    const NAME: &'static str = "parties.open_items";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = OpenItemsInput;
    type Output = OpenItemsOutput;

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
                "SELECT id, ledger, kind, source_id, posting_date, due_date, currency, tx_original_minor, tx_remaining_minor FROM open_item WHERE company_id = ? AND party_id = ? AND tx_remaining_minor != 0 ORDER BY posting_date, id".to_string(),
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ))
            .await?;
        let mut open_items = Vec::new();
        for row in &rows.rows {
            let text = |column: &str| -> Result<String, CommandError> {
                row.get::<String>(column)
                    .ok_or_else(|| CommandError::InvalidInput(format!("open item lacks {column}")))
            };
            open_items.push(OpenItemView {
                id: text("id")?,
                ledger: text("ledger")?,
                kind: text("kind")?,
                document: row.get::<Option<String>>("source_id").flatten(),
                posting_date: text("posting_date")?,
                due_date: row.get::<Option<String>>("due_date").flatten(),
                currency: text("currency")?,
                original_minor: row.get::<i64>("tx_original_minor").unwrap_or(0),
                remaining_minor: row.get::<i64>("tx_remaining_minor").unwrap_or(0),
            });
        }
        let mut balances: Vec<PartyBalance> = Vec::new();
        for item in &open_items {
            match balances
                .iter_mut()
                .find(|b| b.ledger == item.ledger && b.currency == item.currency)
            {
                Some(total) => total.remaining_minor += item.remaining_minor,
                None => balances.push(PartyBalance {
                    ledger: item.ledger.clone(),
                    currency: item.currency.clone(),
                    remaining_minor: item.remaining_minor,
                }),
            }
        }
        Ok(Plan::with_output(OpenItemsOutput {
            open_items,
            balances,
        }))
    }
}
