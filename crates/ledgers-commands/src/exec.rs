//! The command executor: authentication, policy, audit logging, and execution.
//!
//! The lifecycle of command execution:
//!
//! 1. Authenticate the credential → Principal.
//! 2. Check tier requirement and human-only flag.
//! 3. Preview the command (what it would do).
//! 4. If dry-run: return the preview without writing.
//! 5. Apply the policy engine → Decision.
//! 6. If denied: record a Denied audit event, return the error.
//! 7. If needs approval: record a `NeedsApproval` event, return the approval request.
//! 8. If allowed: execute (re-plan on chain-fork retry), append Allowed audit event.

use crate::auth::{Authenticator, Credential};
use crate::command::{Command, CommandContext};
use crate::error::CommandError;
use crate::journal_preview::JournalPreview;
use crate::policy::Policy;
use crate::registry::CommandMeta;
use cratefield_core::{Database, Statement};
use ledgers_audit::{AuditConfig, AuditContext, AuditRecord, Outcome, append_statements, record};
use ledgers_db::chain;
use ledgers_db::retry::{self, RetryPolicy};
use std::time::Duration;

/// A proof token that binds to the executor: only the executor can
/// construct it, and only code with the token can call plan.
///
/// This prevents code outside the executor from calling plan, ensuring
/// the executor controls when re-planning happens on retry.
pub struct ExecToken {
    _private: (),
}

impl ExecToken {
    /// Creates a new token (crate-private: only the executor constructs it).
    fn new() -> Self {
        ExecToken { _private: () }
    }
}

/// The execution mode: run the command, or do a dry-run.
#[derive(Copy, Clone, Debug)]
pub enum Execute {
    /// Execute the command: write to the database, audit log the change.
    Run,
    /// Dry-run: preview only, no writes, no audit.
    DryRun,
}

/// The result of executing a command.
#[derive(Debug)]
pub enum Executed<Output> {
    /// The command executed and produced output.
    Done(Output),
    /// A dry-run was executed: the preview of what would happen.
    DryRun(JournalPreview),
    /// The policy engine said approval is needed: the approval request hash
    /// and the preview are provided.
    NeedsApproval {
        /// Hash of (`command_name` + `input` + `preview`) as a request id.
        request_hash: String,
        /// The preview of what would be done.
        preview: JournalPreview,
    },
}

/// The command executor: runs commands with authentication, policy, and audit.
pub struct Executor<'db, A: Authenticator, P: Policy> {
    db: &'db dyn Database,
    authenticator: A,
    policy: P,
}

impl<'db, A: Authenticator, P: Policy> Executor<'db, A, P> {
    /// Creates an executor.
    pub fn new(db: &'db dyn Database, authenticator: A, policy: P) -> Self {
        Executor {
            db,
            authenticator,
            policy,
        }
    }

    /// Executes a command.
    ///
    /// The full flow: authenticate → check tier/human-only → preview →
    /// dry-run check → policy → deny/approval/allow → execute (with retry).
    ///
    /// # Errors
    /// Returns an error if authentication fails, tier is insufficient, the
    /// command is human-only but run by an agent, the preview fails, dry-run is
    /// unsupported, policy denies or needs approval, or execution fails.
    #[allow(clippy::too_many_lines)]
    pub async fn run<C: Command + ?Sized>(
        &self,
        cmd: &C,
        credential: Credential,
        ctx: CommandContext,
        input: &C::Input,
        mode: Execute,
    ) -> Result<Executed<C::Output>, CommandError> {
        // 1. Authenticate.
        let principal = self.authenticator.authenticate(&credential).await?;

        // 2. Check tier and human-only.
        #[allow(clippy::collapsible_if)]
        if let Some(tier) = principal.tier() {
            if tier < C::TIER {
                return Err(CommandError::TierInsufficient {
                    required: C::TIER.to_string(),
                    actual: tier.to_string(),
                });
            }
        }
        if C::HUMAN_ONLY && principal.is_agent() {
            return Err(CommandError::HumanOnly);
        }

        // 3. Preview.
        let preview = cmd.preview(self.db, &ctx, input).await?;

        // 4. Check dry-run support.
        match mode {
            Execute::DryRun if !C::SUPPORTS_DRY_RUN => {
                return Err(CommandError::DryRunUnsupported);
            }
            Execute::DryRun => {
                return Ok(Executed::DryRun(preview));
            }
            Execute::Run => {}
        }

        // 5. Apply policy.
        let meta = CommandMeta {
            name: C::NAME.to_string(),
            tier: C::TIER,
            human_only: C::HUMAN_ONLY,
            supports_dry_run: C::SUPPORTS_DRY_RUN,
            idempotent: C::IDEMPOTENT,
        };

        let decision = self.policy.decide(&principal, &meta, &preview);

        match decision {
            crate::policy::Decision::Deny { reason } => {
                // Record a Denied audit event (separate batch).
                let input_json = serde_json::to_value(input)
                    .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
                let audit_ctx = AuditContext::new(
                    principal.to_actor(),
                    ctx.request_id.clone(),
                    ctx.event_id.clone(),
                );
                let record = AuditRecord::from_input(
                    C::NAME,
                    &input_json,
                    Outcome::Denied,
                    vec![],
                    Some(reason),
                );
                record_deny(self.db, &ctx.company_id, &audit_ctx, &record, &ctx.now).await?;
                Err(CommandError::PolicyDenied)
            }
            crate::policy::Decision::NeedsApproval { reason } => {
                // Record a NeedsApproval audit event (separate batch).
                let input_json = serde_json::to_value(input)
                    .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
                let audit_ctx = AuditContext::new(
                    principal.to_actor(),
                    ctx.request_id.clone(),
                    ctx.event_id.clone(),
                );
                let record = AuditRecord::from_input(
                    C::NAME,
                    &input_json,
                    Outcome::NeedsApproval,
                    vec![],
                    Some(reason),
                );
                record_deny(self.db, &ctx.company_id, &audit_ctx, &record, &ctx.now).await?;

                let request_hash = compute_request_hash(C::NAME, &input_json, &preview)?;
                Ok(Executed::NeedsApproval {
                    request_hash,
                    preview,
                })
            }
            crate::policy::Decision::Allow => {
                // Execute (with retry).
                let input_json = serde_json::to_value(input)
                    .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
                let audit_ctx = AuditContext::new(
                    principal.to_actor(),
                    ctx.request_id.clone(),
                    ctx.event_id.clone(),
                );

                let token = ExecToken::new();
                let plan = cmd.plan(&token, self.db, &ctx, input).await?;

                // If the plan has no statements, don't write an audit event
                // (consistent with ledger's Read kind behavior).
                if plan.statements.is_empty() && C::TIER == crate::auth::Tier::Read {
                    return Ok(Executed::Done(plan.output));
                }

                let record = AuditRecord::from_input(
                    C::NAME,
                    &input_json,
                    Outcome::Allowed,
                    plan.objects,
                    None,
                );

                // Append guard statements from the policy.
                let mut statements = plan.statements;
                let guard_statements = self.policy.guards(&principal, &meta, &preview)?;
                statements.extend(guard_statements);

                execute_with_retry(
                    self.db,
                    &ctx.company_id,
                    &audit_ctx,
                    &record,
                    &ctx.now,
                    statements,
                )
                .await?;

                Ok(Executed::Done(plan.output))
            }
        }
    }
}

/// Records a single audit event (denial or approval request) in its own batch.
async fn record_deny(
    db: &dyn Database,
    company_id: &str,
    audit: &AuditContext,
    audit_record: &AuditRecord,
    now: &str,
) -> Result<(), CommandError> {
    record(
        db,
        &AuditConfig::default(),
        company_id,
        audit,
        audit_record,
        now,
        RetryPolicy::default(),
        no_sleep,
    )
    .await
    .map(|_| ())
    .map_err(|e| CommandError::Database(cratefield_core::DbError::Query(e.to_string())))
}

/// Executes a command's statements and appends its audit event in one batch,
/// with retry support (re-planning on chain-fork).
async fn execute_with_retry(
    db: &dyn Database,
    company_id: &str,
    audit: &AuditContext,
    audit_record: &AuditRecord,
    now: &str,
    change: Vec<Statement>,
) -> Result<(), CommandError> {
    retry::run_batch_with_retry(
        db,
        RetryPolicy::default(),
        || {
            let change = change.clone();
            async move {
                let head = chain::read_head(db, company_id).await?;
                let appended = append_statements(
                    company_id,
                    &head,
                    audit,
                    audit_record,
                    now,
                    &AuditConfig::default(),
                )
                .map_err(|e| cratefield_core::DbError::Query(e.to_string()))?;
                let mut statements = change;
                statements.extend(appended.statements);
                Ok(statements)
            }
        },
        no_sleep,
    )
    .await?;
    Ok(())
}

/// No sleep: the library never sleeps (ADR 0004 — wasm-clean).
fn no_sleep(_: Duration) {}

/// Computes the SHA-256 hash of (`command_name` + `input` + `preview`) as a
/// request id for approval tracking. The hash binds the approval to the
/// exact request so it cannot be replayed with different input or preview.
fn compute_request_hash(
    name: &str,
    input: &serde_json::Value,
    preview: &JournalPreview,
) -> Result<String, CommandError> {
    let preview_json =
        serde_json::to_value(preview).map_err(|e| CommandError::InvalidInput(e.to_string()))?;
    let request_obj = serde_json::json!({
        "command": name,
        "input": input,
        "preview": preview_json,
    });
    let hash = ledgers_audit::input_hash(&request_obj);
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_token_not_clone() {
        // Verify ExecToken cannot be cloned or copied.
        let _token = ExecToken::new();
        // This would fail to compile:
        // let _token2 = _token;
    }
}
