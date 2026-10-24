//! The `subledger.reconcile` read command: reconciliation report.

use async_trait::async_trait;
use cratefield_core::Database;
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::reconcile::{LedgerReconciliation, reconcile};

/// Input to the `subledger.reconcile` command.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReconcileInput {
    /// The as-of date (ISO 8601).
    pub as_of: String,
}

/// Output of the `subledger.reconcile` command.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReconcileOutput {
    /// Reconciliation results per ledger (ar, ap).
    pub ledgers: Vec<LedgerReconciliation>,
}

/// The `subledger.reconcile` read command.
pub struct SubledgerReconcile;

#[async_trait]
impl Command for SubledgerReconcile {
    const NAME: &'static str = "subledger.reconcile";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = false;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = None;

    type Input = ReconcileInput;
    type Output = ReconcileOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Read command, no journal entry
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let ledgers = reconcile(db, &ctx.company_id, &input.as_of)
            .await
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;

        Ok(Plan::with_output(ReconcileOutput { ledgers }))
    }
}
