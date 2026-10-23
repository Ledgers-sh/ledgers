//! `parties.balances`: what the company owes each party (issue #44).
//!
//! Receivables are not tracked yet (there is no invoicing module), so the
//! read carries payables only: posted bills that no bank line has settled.
//! Settlement is the bank's `matched_bill_id`, the same rule `open_bills`
//! uses. Merged parties are left out; their bills already point at the survivor.

use cratefield_core::{Database, DbError, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};

/// The input of `parties.balances`: none.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PartyBalancesInput {}

/// One party's open payable in one currency.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PartyBalanceView {
    /// The party id.
    pub party_id: String,
    /// The ISO 4217 currency of the amount.
    pub currency: String,
    /// The sum of the open bills' gross amounts, in minor units.
    pub payable_minor: i64,
    /// How many bills make up the amount.
    pub open_bills: u32,
}

/// The output of `parties.balances`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PartyBalancesOutput {
    /// One row per party and currency with an open payable; parties with
    /// nothing open are absent.
    pub balances: Vec<PartyBalanceView>,
}

/// The open payables of the company, grouped by party and currency.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn open_payables(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<PartyBalanceView>, DbError> {
    let stmt = Statement::with_values(
        "SELECT b.vendor_id AS party_id, b.currency AS currency, CAST(SUM(b.gross_minor) AS BIGINT) AS payable_minor, CAST(COUNT(*) AS BIGINT) AS open_bills \
         FROM bill b JOIN vendor v ON v.id = b.vendor_id AND v.company_id = b.company_id \
         WHERE b.company_id = ? AND b.posted_entry_id IS NOT NULL AND b.discarded_at IS NULL \
         AND b.gross_minor IS NOT NULL AND v.merged_into IS NULL \
         AND NOT EXISTS (SELECT 1 FROM bank_transaction t WHERE t.company_id = b.company_id \
         AND t.matched_bill_id = b.id) \
         GROUP BY b.vendor_id, b.currency ORDER BY b.vendor_id, b.currency"
            .to_owned(),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            let text = |column: &str| {
                row.get::<String>(column)
                    .ok_or_else(|| DbError::Query(format!("bank row lacks {column}")))
            };
            Ok(PartyBalanceView {
                party_id: text("party_id")?,
                currency: text("currency")?,
                payable_minor: row
                    .get::<i64>("payable_minor")
                    .ok_or_else(|| DbError::Query("bank row lacks payable_minor".to_string()))?,
                open_bills: u32::try_from(
                    row.get::<i64>("open_bills")
                        .ok_or_else(|| DbError::Query("bank row lacks open_bills".to_string()))?,
                )
                .unwrap_or(u32::MAX),
            })
        })
        .collect()
}

/// `parties.balances`: open payables per party and currency.
pub struct PartiesBalances;

#[async_trait::async_trait]
impl Command for PartiesBalances {
    const NAME: &'static str = "parties.balances";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = PartyBalancesInput;
    type Output = PartyBalancesOutput;

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
        let balances = open_payables(db, &ctx.company_id)
            .await
            .map_err(|_| CommandError::InvalidInput("database error".to_string()))?;
        Ok(Plan::with_output(PartyBalancesOutput { balances }))
    }
}
