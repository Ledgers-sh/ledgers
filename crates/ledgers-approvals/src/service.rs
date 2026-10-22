//! Approval service: raise requests, issue nonces, approve and reject.

use crate::approval::{ApprovalRequest, Risk, Status};
use crate::config::ApprovalConfig;
use crate::error::ApprovalError;
use crate::rules::{ApprovalRules, StepUp};
use crate::stepup::{AssertionVerifier, CredentialKeys, PasskeyProof};
use cratefield_core::{Database, Statement};
use ledgers_audit::{AuditContext, AuditRecord, Outcome, event::Actor};
use ledgers_commands::{
    Approval, Authenticator, Command, CommandContext, CommandError, Credential, Executor, Policy,
    Principal, mcp::escape_document_text,
};
use ledgers_db::guard;
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// A card summarizing an approval request for chat display.
#[derive(Clone, Debug)]
pub struct ApprovalCard {
    /// The approval request id.
    pub approval_id: String,
    /// The URL to view/approve this request.
    pub url: String,
    /// Human-readable card text (markdown with escaped user content).
    pub card: String,
}

/// A raised approval request ready for the approver to review.
#[derive(Clone, Debug)]
pub struct RaisedApproval {
    /// The approval request.
    pub request: ApprovalRequest,
    /// A card for chat.
    pub card: ApprovalCard,
}

/// The approvals service.
pub struct Approvals<'db, R: ApprovalRules, V: AssertionVerifier, K: CredentialKeys> {
    db: &'db dyn Database,
    rules: R,
    verifier: V,
    credential_keys: K,
    config: ApprovalConfig,
}

impl<'db, R: ApprovalRules, V: AssertionVerifier, K: CredentialKeys> Approvals<'db, R, V, K> {
    /// Creates a new approvals service.
    #[must_use]
    pub fn new(
        db: &'db dyn Database,
        rules: R,
        verifier: V,
        credential_keys: K,
        config: ApprovalConfig,
    ) -> Self {
        Approvals {
            db,
            rules,
            verifier,
            credential_keys,
            config,
        }
    }

    /// Raises an approval request from a command that needs approval.
    ///
    /// The request is frozen (input and preview immutable), and a card
    /// is returned for display in chat. A counterparty can be supplied
    /// for context (e.g., "Pay `GmbH`").
    ///
    /// # Errors
    /// Returns an error if the request cannot be stored.
    #[allow(clippy::too_many_arguments)]
    pub async fn raise(
        &self,
        command: &str,
        input_json: &serde_json::Value,
        preview_json: &serde_json::Value,
        reads: &BTreeMap<String, String>,
        request_hash: String,
        reason: String,
        requested_by: String,
        company_id: &str,
        approval_id: String,
        counterparty: Option<&str>,
        now: &str,
    ) -> Result<RaisedApproval, ApprovalError> {
        let (statement, request) = request_statement(&NewRequest {
            id: &approval_id,
            company_id,
            command,
            input: input_json,
            preview: preview_json,
            reads,
            request_hash: &request_hash,
            reason: &reason,
            requested_by: &requested_by,
            counterparty,
            now,
            ttl: self.config.ttl,
            low_risk_limits: &BTreeMap::new(),
        })?;
        let card = build_card(&request, counterparty, None, false);
        self.db
            .batch_atomic(&[statement])
            .await
            .map_err(|e| ApprovalError::Database(e.to_string()))?;
        Ok(RaisedApproval { request, card })
    }

    /// Approves a request by a fresh sign-in: the person signed in within
    /// [`FRESH_SIGN_IN_SECS`] (`signed_in_at`, epoch seconds), or the request is
    /// low risk. The command runs once: the status guard and the command's
    /// writes share one batch, so a second approval of the same request fails
    /// its guard and posts nothing. The result is recorded on the request.
    ///
    /// # Errors
    /// `ReauthRequired` for a high-risk request without a fresh sign-in,
    /// `NotPending`, `Expired`, `Forbidden` for a non-person approver, or the
    /// command's own error.
    #[allow(clippy::too_many_arguments)]
    pub async fn approve_signed_in<C: Command + ?Sized, A: Authenticator>(
        &self,
        executor: &Executor<'_, A, impl Policy>,
        authenticator: &A,
        cmd: &C,
        approver_credential: Credential,
        ctx: CommandContext,
        approval_id: &str,
        signed_in_at: Option<i64>,
    ) -> Result<C::Output, ApprovalError> {
        let request = load_approval_request(self.db, approval_id).await?;
        if request.command != C::NAME {
            return Err(ApprovalError::Other(format!(
                "approval command mismatch: expected {}, got {}",
                C::NAME,
                request.command
            )));
        }
        if request.status != Status::Pending {
            return Err(ApprovalError::NotPending {
                status: request.status.as_str().to_string(),
            });
        }
        let now_ts = parse_rfc3339(&ctx.now)?;
        if now_ts > parse_rfc3339(&request.expires_at)? {
            self.mark_expired(approval_id).await;
            return Err(ApprovalError::Expired);
        }
        let approver = authenticator.authenticate(&approver_credential).await?;
        let Principal::Person { person_id } = &approver else {
            return Err(ApprovalError::Forbidden);
        };
        if !self.rules.may_approve(&approver, &request) {
            return Err(ApprovalError::Forbidden);
        }
        if request.risk == Risk::High && !signed_in_recently(signed_in_at, now_ts) {
            return Err(ApprovalError::ReauthRequired);
        }

        let input: C::Input = serde_json::from_str(&request.input).map_err(|e| {
            ApprovalError::Other(format!("failed to deserialize frozen input: {e}"))
        })?;
        let approval = Approval {
            approval_id: approval_id.to_string(),
            request_hash: request.request_hash.clone(),
            requested_by: request.requested_by.clone(),
            statements: Self::decision_statements(
                approval_id,
                &request.request_hash,
                Status::Approved,
                Some(person_id),
                &ctx.now,
                None,
                &ctx.event_id,
            ),
            objects: vec![format!("approval:{approval_id}")],
        };
        let output = match executor
            .run_approved(cmd, approver_credential, ctx.clone(), &input, approval)
            .await
        {
            Ok(output) => output,
            Err(err) => return Err(self.classify_failure(approval_id, err).await),
        };

        // The entry reference is known only after the command has run, so it
        // is recorded in a second, guarded statement once the decision is in.
        let result_json = serde_json::to_string(&output)?;
        let record_result = Statement::with_values(
            "UPDATE approval_request SET result = ? WHERE id = ? AND status = 'approved'"
                .to_owned(),
            vec![
                SeaValue::from(result_json.as_str()),
                SeaValue::from(approval_id),
            ],
        );
        self.db
            .batch_atomic(&[record_result])
            .await
            .map_err(|e| ApprovalError::Database(e.to_string()))?;
        Ok(output)
    }

    /// Rejects a pending request with an optional reason. Rejection needs no
    /// fresh sign-in. The status change and its audit event share one batch.
    ///
    /// # Errors
    /// `NotPending`, `Expired`, `Forbidden` for a non-person, or a database error.
    pub async fn reject_signed_in<A: Authenticator>(
        &self,
        executor: &Executor<'_, A, impl Policy>,
        authenticator: &A,
        approver_credential: Credential,
        approval_id: &str,
        ctx: &CommandContext,
        reason: Option<&str>,
    ) -> Result<(), ApprovalError> {
        let request = load_approval_request(self.db, approval_id).await?;
        if request.status != Status::Pending {
            return Err(ApprovalError::NotPending {
                status: request.status.as_str().to_string(),
            });
        }
        if parse_rfc3339(&ctx.now)? > parse_rfc3339(&request.expires_at)? {
            self.mark_expired(approval_id).await;
            return Err(ApprovalError::Expired);
        }
        let approver = authenticator.authenticate(&approver_credential).await?;
        let Principal::Person { person_id } = &approver else {
            return Err(ApprovalError::Forbidden);
        };
        if !self.rules.may_approve(&approver, &request) {
            return Err(ApprovalError::Forbidden);
        }

        let audit_ctx = AuditContext::new(
            approver.to_actor(),
            ctx.request_id.clone(),
            ctx.event_id.clone(),
        )
        .with_approval(approval_id);
        let record = AuditRecord::from_input(
            "approval.reject",
            &serde_json::json!({}),
            Outcome::Allowed,
            vec![format!("approval:{approval_id}")],
            reason.map(str::to_owned),
        );
        let mut statements = Self::decision_statements(
            approval_id,
            &request.request_hash,
            Status::Rejected,
            Some(person_id),
            &ctx.now,
            reason,
            &ctx.event_id,
        );
        statements.extend(
            executor
                .audit_statements(&request.company_id, &audit_ctx, &record, &ctx.now)
                .await?,
        );
        if self.db.batch_atomic(&statements).await.is_err() {
            let current = load_approval_request(self.db, approval_id).await?;
            if current.status != Status::Pending {
                return Err(ApprovalError::NotPending {
                    status: current.status.as_str().to_string(),
                });
            }
            return Err(ApprovalError::Database(
                "the decision batch was refused".to_owned(),
            ));
        }
        Ok(())
    }

    /// The status guard, the status update and the guard release for a decision.
    fn decision_statements(
        id: &str,
        request_hash: &str,
        to_status: Status,
        decided_by: Option<&str>,
        decided_at: &str,
        decision_reason: Option<&str>,
        audit_ref: &str,
    ) -> Vec<Statement> {
        let status_guard_key = guard::guard_key(&["status-pending", id]);
        let status_assert = guard::assert_statement(
            &status_guard_key,
            "EXISTS (SELECT 1 FROM approval_request \
             WHERE id = ? AND status = 'pending' AND request_hash = ? AND expires_at > ?)",
            vec![
                SeaValue::from(id),
                SeaValue::from(request_hash),
                SeaValue::from(decided_at),
            ],
        );
        let update = Statement::with_values(
            "UPDATE approval_request SET status = ?, decided_by = ?, decided_at = ?, \
             decision_reason = ?, audit_ref = ? WHERE id = ?"
                .to_owned(),
            vec![
                SeaValue::from(to_status.as_str()),
                SeaValue::from(decided_by.map(str::to_owned)),
                SeaValue::from(decided_at),
                SeaValue::from(decision_reason.map(str::to_owned)),
                SeaValue::from(audit_ref),
                SeaValue::from(id),
            ],
        );
        vec![
            status_assert,
            update,
            guard::release_statement(&status_guard_key),
        ]
    }

    async fn mark_expired(&self, approval_id: &str) {
        let stmt = Statement::with_values(
            "UPDATE approval_request SET status = ? WHERE id = ? AND status = 'pending'".to_owned(),
            vec![
                SeaValue::from(Status::Expired.as_str()),
                SeaValue::from(approval_id),
            ],
        );
        let _ = self.db.batch_atomic(&[stmt]).await;
    }

    /// Classifies a failed approval: a superseded request is marked so, and a
    /// request that is no longer pending (a concurrent decision won) reports
    /// its status instead of the guard's generic refusal.
    async fn classify_failure(&self, approval_id: &str, err: CommandError) -> ApprovalError {
        if matches!(err, CommandError::Superseded) {
            let stmt = Statement::with_values(
                "UPDATE approval_request SET status = ? WHERE id = ? AND status = 'pending'"
                    .to_owned(),
                vec![
                    SeaValue::from(Status::Superseded.as_str()),
                    SeaValue::from(approval_id),
                ],
            );
            let _ = self.db.batch_atomic(&[stmt]).await;
            return ApprovalError::Superseded;
        }
        if let Ok(current) = load_approval_request(self.db, approval_id).await
            && current.status != Status::Pending
        {
            return ApprovalError::NotPending {
                status: current.status.as_str().to_string(),
            };
        }
        ApprovalError::Command(err)
    }

    /// Issues a nonce for passkey step-up on this approval.
    ///
    /// The nonce is bound to the request hash and stored with short expiry.
    /// Returns the challenge bytes (SHA-256(request_hash || nonce)).
    ///
    /// # Errors
    /// Returns an error if the nonce cannot be stored or the approval is not found.
    pub async fn issue_nonce(
        &self,
        approval_id: &str,
        nonce: &[u8],
        now: &str,
    ) -> Result<Vec<u8>, ApprovalError> {
        // Load approval request to get request_hash
        let request = load_approval_request(self.db, approval_id).await?;

        // Compute expiry for the nonce
        let now_ts = parse_rfc3339(now)?;
        #[allow(clippy::cast_possible_wrap)]
        let nonce_ttl_secs = self.config.nonce_ttl.as_secs() as i64;
        let nonce_expires_ts = now_ts + nonce_ttl_secs;
        let nonce_expires_at = format_rfc3339(nonce_expires_ts)?;

        // Hex-encode the nonce for storage
        let nonce_hex = hex::encode(nonce);

        // Insert nonce row
        let insert_stmt = Statement::with_values(
            "INSERT INTO approval_nonce (nonce, approval_id, request_hash, expires_at, used_at) \
             VALUES (?, ?, ?, ?, ?)"
                .to_owned(),
            vec![
                SeaValue::from(nonce_hex.as_str()),
                SeaValue::from(approval_id),
                SeaValue::from(request.request_hash.as_str()),
                SeaValue::from(nonce_expires_at.as_str()),
                SeaValue::from(None::<String>),
            ],
        );

        self.db
            .batch_atomic(&[insert_stmt])
            .await
            .map_err(|e| ApprovalError::Database(e.to_string()))?;

        // Compute and return challenge: SHA-256(request_hash || nonce)
        let mut challenge_bytes = request.request_hash.as_bytes().to_vec();
        challenge_bytes.extend_from_slice(nonce);
        let challenge = Sha256::digest(&challenge_bytes);
        Ok(challenge.to_vec())
    }

    /// Verifies step-up (passkey or session) for an approval.
    /// Returns `Some(credential_id)` for passkey verification, None for session step-up.
    /// Does NOT create guard statements.
    ///
    /// # Errors
    /// Returns an error if step-up verification fails.
    async fn authorize(
        &self,
        row: &ApprovalRequest,
        approver: &Principal,
        proof: Option<&PasskeyProof>,
        now: &str,
    ) -> Result<Option<String>, ApprovalError> {
        let step_up = self.rules.step_up(row);

        if step_up != StepUp::Passkey {
            return Ok(None);
        }

        // Passkey step-up required
        let proof = proof.ok_or(ApprovalError::StepUpRequired)?;

        // Get the approver's person_id from the actor
        let actor = approver.to_actor();
        let approver_person_id = match actor {
            Actor::Person { person_id } => person_id,
            Actor::Agent { .. } => {
                // Should not happen due to may_approve check, but be safe
                return Err(ApprovalError::Forbidden);
            }
        };

        // Load and verify nonce (fast-fail check)
        let nonce_row = load_and_verify_nonce(self.db, &row.id, now).await?;

        // Verify the assertion: challenge = SHA-256(request_hash || nonce)
        let challenge =
            Sha256::digest([row.request_hash.as_bytes(), &nonce_row.nonce_bytes].concat());

        // Lookup the public key for this approver and credential
        let public_key = self
            .credential_keys
            .public_key(&approver_person_id, &proof.credential_id)
            .ok_or(ApprovalError::InvalidAssertion)?;

        self.verifier
            .verify(
                proof,
                challenge.as_ref(),
                &self.config.rp_id,
                &self.config.origin,
                &public_key,
            )
            .map_err(|_e| ApprovalError::InvalidAssertion)?;

        Ok(Some(proof.credential_id.clone()))
    }

    /// Creates guard assert, update, and release statements for nonce consumption.
    fn nonce_statements(
        approval_id: &str,
        request_hash: &str,
        nonce_hex: &str,
        now: &str,
    ) -> Vec<Statement> {
        let nonce_guard_key = guard::guard_key(&["nonce-used", approval_id, nonce_hex]);

        let nonce_assert = guard::assert_statement(
            &nonce_guard_key,
            "EXISTS (SELECT 1 FROM approval_nonce \
             WHERE nonce = ? AND approval_id = ? AND request_hash = ? AND used_at IS NULL AND expires_at > ?)",
            vec![
                SeaValue::from(nonce_hex),
                SeaValue::from(approval_id),
                SeaValue::from(request_hash),
                SeaValue::from(now),
            ],
        );

        let update_nonce = Statement::with_values(
            "UPDATE approval_nonce SET used_at = ? WHERE nonce = ?".to_owned(),
            vec![SeaValue::from(now), SeaValue::from(nonce_hex)],
        );

        let nonce_release = guard::release_statement(&nonce_guard_key);

        vec![nonce_assert, update_nonce, nonce_release]
    }

    /// Creates status guard assert, update, and release statements for approval transition.
    fn transition_statements(
        id: &str,
        request_hash: &str,
        to_status: &str,
        decided_by: Option<&str>,
        credential_id: Option<&str>,
        now: &str,
    ) -> Vec<Statement> {
        let status_guard_key = guard::guard_key(&["status-pending", id]);

        let status_assert = guard::assert_statement(
            &status_guard_key,
            "EXISTS (SELECT 1 FROM approval_request \
             WHERE id = ? AND status = 'pending' AND request_hash = ? AND expires_at > ?)",
            vec![
                SeaValue::from(id),
                SeaValue::from(request_hash),
                SeaValue::from(now),
            ],
        );

        let credential_id_value = if let Some(cred_id) = credential_id {
            SeaValue::from(cred_id)
        } else {
            SeaValue::from(None::<String>)
        };

        let decided_by_value = if let Some(decided) = decided_by {
            SeaValue::from(decided)
        } else {
            SeaValue::from(None::<String>)
        };

        let update_approval = Statement::with_values(
            "UPDATE approval_request SET status = ?, decided_by = ?, credential_id = ? WHERE id = ?".to_owned(),
            vec![
                SeaValue::from(to_status),
                decided_by_value,
                credential_id_value,
                SeaValue::from(id),
            ],
        );

        let status_release = guard::release_statement(&status_guard_key);

        vec![status_assert, update_approval, status_release]
    }

    /// Approves an approval request.
    ///
    /// If `step_up` requires passkey, the proof must be provided. The nonce
    /// is verified as single-use (via guard statement), the assertion is verified,
    /// and the command is executed. Returns an error if the request is not pending,
    /// expired, superseded, or if step-up verification fails.
    ///
    /// # Errors
    /// Returns an error if approval fails.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn approve<C: Command + ?Sized, A: Authenticator>(
        &self,
        executor: &Executor<'_, A, impl Policy>,
        cmd: &C,
        authenticator: &A,
        approver_credential: Credential,
        ctx: CommandContext,
        approval_id: &str,
        step_up_proof: Option<PasskeyProof>,
    ) -> Result<C::Output, ApprovalError> {
        // 1. Load approval request
        let request = load_approval_request(self.db, approval_id).await?;

        // 1b. Check that the command matches and deserialize the frozen input
        if request.command != C::NAME {
            return Err(ApprovalError::Other(format!(
                "approval command mismatch: expected {}, got {}",
                C::NAME,
                request.command
            )));
        }

        let input: C::Input = serde_json::from_str(&request.input).map_err(|e| {
            ApprovalError::Other(format!("failed to deserialize frozen input: {e}"))
        })?;

        // 2. Fast-fail check on status (will be verified by guard in batch)
        if request.status != Status::Pending {
            return Err(ApprovalError::NotPending {
                status: request.status.as_str().to_string(),
            });
        }

        // 3. Check not expired (fast-fail, will be verified by guard in batch)
        let now_ts = parse_rfc3339(&ctx.now)?;
        let expires_ts = parse_rfc3339(&request.expires_at)?;
        if now_ts > expires_ts {
            // Mark as expired and return error
            let update_stmt = Statement::with_values(
                "UPDATE approval_request SET status = ? WHERE id = ?".to_owned(),
                vec![
                    SeaValue::from(Status::Expired.as_str()),
                    SeaValue::from(approval_id),
                ],
            );
            let _ = self.db.batch_atomic(&[update_stmt]).await;
            return Err(ApprovalError::Expired);
        }

        // 4. Authenticate approver
        let approver_principal = authenticator.authenticate(&approver_credential).await?;

        // 5. Check may_approve
        if !self.rules.may_approve(&approver_principal, &request) {
            return Err(ApprovalError::Forbidden);
        }

        // 6. Authenticate and authorize
        let mut approval_statements = Vec::new();
        let mut approval_objects = vec![format!("approval:{}", approval_id)];
        let approved_by_id = serde_json::to_string(&approver_principal.to_actor())
            .map_err(|e| ApprovalError::Other(format!("failed to serialize actor: {e}")))?;

        // Verify step-up and get credential_id if passkey
        let credential_id = self
            .authorize(
                &request,
                &approver_principal,
                step_up_proof.as_ref(),
                &ctx.now,
            )
            .await?;

        let step_up = self.rules.step_up(&request);
        if step_up == StepUp::Passkey {
            // Load nonce again to get nonce_hex for statements
            let nonce_row = load_and_verify_nonce(self.db, approval_id, &ctx.now).await?;

            // Add nonce guard statements
            let nonce_stmts = Self::nonce_statements(
                approval_id,
                &request.request_hash,
                &nonce_row.nonce_hex,
                &ctx.now,
            );
            approval_statements.extend(nonce_stmts);

            if let Some(cred_id) = &credential_id {
                approval_objects.push(format!("passkey:{cred_id}"));
            }
        }

        // Add transition statements for status update
        let transition_stmts = Self::transition_statements(
            approval_id,
            &request.request_hash,
            Status::Approved.as_str(),
            Some(approved_by_id.as_str()),
            credential_id.as_deref(),
            &ctx.now,
        );
        approval_statements.extend(transition_stmts);

        // 7. Run executor.run_approved with the approval statements
        let approval = Approval {
            approval_id: approval_id.to_string(),
            request_hash: request.request_hash.clone(),
            requested_by: request.requested_by.clone(),
            statements: approval_statements,
            objects: approval_objects,
        };

        let result = executor
            .run_approved(cmd, approver_credential, ctx.clone(), &input, approval)
            .await
            .map_err(|e| match &e {
                ledgers_commands::CommandError::Superseded => ApprovalError::Superseded,
                ledgers_commands::CommandError::Conflict { .. } => ApprovalError::InvalidAssertion,
                _ => ApprovalError::Command(e),
            });

        // If Superseded, mark the approval row as superseded
        if matches!(&result, Err(ApprovalError::Superseded)) {
            let update_stmt = Statement::with_values(
                "UPDATE approval_request SET status = ? WHERE id = ? AND status = 'pending'"
                    .to_owned(),
                vec![
                    SeaValue::from(Status::Superseded.as_str()),
                    SeaValue::from(approval_id),
                ],
            );
            let _ = self.db.batch_atomic(&[update_stmt]).await;
        }

        // If the command execution failed (but not Superseded/Conflict), re-read state to classify
        if let Err(ref approval_err) = result
            && matches!(approval_err, ApprovalError::Command(_))
        {
            // Try to determine the real cause by re-reading state
            if let Ok(updated_request) = load_approval_request(self.db, approval_id).await {
                if updated_request.status != Status::Pending {
                    // Status changed, so this was likely a guard assertion that changed the status
                    return Err(ApprovalError::NotPending {
                        status: updated_request.status.as_str().to_string(),
                    });
                }
                // If step-up was Passkey, check if nonce is missing/used/expired
                if step_up == StepUp::Passkey {
                    let nonce_check = load_and_verify_nonce(self.db, approval_id, &ctx.now).await;
                    if nonce_check.is_err() {
                        // Nonce is missing, used, or expired
                        return Err(ApprovalError::InvalidAssertion);
                    }
                }
            }
        }

        result
    }

    /// Rejects an approval request.
    ///
    /// Passkey step-up is required if configured for this request. A rejection
    /// audit event is recorded in the same batch as the status update.
    ///
    /// # Errors
    /// Returns an error if rejection fails.
    pub async fn reject<A: Authenticator>(
        &self,
        executor: &Executor<'_, A, impl Policy>,
        authenticator: &A,
        approver_credential: Credential,
        approval_id: &str,
        ctx: &CommandContext,
        step_up_proof: Option<PasskeyProof>,
    ) -> Result<(), ApprovalError> {
        // 1. Load approval request
        let request = load_approval_request(self.db, approval_id).await?;

        // 2. Check status == pending (fast-fail)
        if request.status != Status::Pending {
            return Err(ApprovalError::NotPending {
                status: request.status.as_str().to_string(),
            });
        }

        // 3. Authenticate approver
        let approver_principal = authenticator.authenticate(&approver_credential).await?;

        // 4. Check may_approve
        if !self.rules.may_approve(&approver_principal, &request) {
            return Err(ApprovalError::Forbidden);
        }

        // 5. Authenticate and authorize
        let mut rejection_statements = Vec::new();
        let mut rejection_objects = vec![format!("approval:{}", approval_id)];

        // Verify step-up
        let credential_id = self
            .authorize(
                &request,
                &approver_principal,
                step_up_proof.as_ref(),
                &ctx.now,
            )
            .await?;

        if let Some(cred_id) = credential_id {
            rejection_objects.push(format!("passkey:{cred_id}"));
        }

        // 6. Build transition statements for status update to rejected
        let transition_stmts = Self::transition_statements(
            approval_id,
            &request.request_hash,
            Status::Rejected.as_str(),
            None,
            None,
            &ctx.now,
        );
        rejection_statements.extend(transition_stmts);

        // 7. Create audit event for rejection
        let audit_ctx = AuditContext::new(
            approver_principal.to_actor(),
            ctx.request_id.clone(),
            ctx.event_id.clone(),
        )
        .with_approval(approval_id);

        let record = AuditRecord::from_input(
            "approval.reject",
            &serde_json::json!({}),
            Outcome::Allowed,
            rejection_objects,
            None,
        );

        let audit_stmts = executor
            .audit_statements(&request.company_id, &audit_ctx, &record, &ctx.now)
            .await?;
        rejection_statements.extend(audit_stmts);

        self.db
            .batch_atomic(&rejection_statements)
            .await
            .map_err(|e| {
                // Map constraint/guard failure to InvalidAssertion
                ApprovalError::Database(e.to_string())
            })?;

        Ok(())
    }
}

/// How long a sign-in counts as fresh enough to approve a high-risk request.
pub const FRESH_SIGN_IN_SECS: i64 = 300;

/// What a new approval request is raised from: the frozen command and its context.
pub struct NewRequest<'a> {
    /// The approval request id.
    pub id: &'a str,
    /// The company id.
    pub company_id: &'a str,
    /// The command name.
    pub command: &'a str,
    /// The command input as the agent sent it.
    pub input: &'a serde_json::Value,
    /// The journal preview at request time.
    pub preview: &'a serde_json::Value,
    /// The object versions read at request time.
    pub reads: &'a BTreeMap<String, String>,
    /// The request hash binding the approval to this request.
    pub request_hash: &'a str,
    /// The policy reason approval is needed.
    pub reason: &'a str,
    /// The requester (agent key id).
    pub requested_by: &'a str,
    /// The counterparty named by the command, if any.
    pub counterparty: Option<&'a str>,
    /// When the request is created (RFC 3339 UTC).
    pub now: &'a str,
    /// How long the request stays pending.
    pub ttl: std::time::Duration,
    /// Per-currency low-risk limits as canonical amount strings.
    pub low_risk_limits: &'a BTreeMap<String, String>,
}

/// Builds the insert for a new pending approval request, and the request it stores.
///
/// # Errors
/// Returns an error if the preview or reads cannot be serialized or the times are invalid.
pub fn request_statement(
    req: &NewRequest<'_>,
) -> Result<(Statement, ApprovalRequest), ApprovalError> {
    let effect = extract_effect(req.preview)?;
    let risk = risk_for_effect(&effect, req.low_risk_limits);
    let created = parse_rfc3339(req.now)?;
    #[allow(clippy::cast_possible_wrap)]
    let ttl_secs = req.ttl.as_secs() as i64;
    let request = ApprovalRequest {
        id: req.id.to_owned(),
        company_id: req.company_id.to_owned(),
        command: req.command.to_owned(),
        input: serde_json::to_string(req.input)?,
        preview: serde_json::to_string(req.preview)?,
        reads: serde_json::to_string(req.reads)?,
        request_hash: req.request_hash.to_owned(),
        effect,
        requested_by: req.requested_by.to_owned(),
        reason: req.reason.to_owned(),
        status: Status::Pending,
        created_at: req.now.to_owned(),
        expires_at: format_rfc3339(created + ttl_secs)?,
        decided_by: None,
        credential_id: None,
        risk,
        counterparty: req.counterparty.map(str::to_owned),
        decided_at: None,
        decision_reason: None,
        result: None,
        audit_ref: None,
    };
    let effect_json = serde_json::to_string(&request.effect)?;
    let statement = Statement::with_values(
        "INSERT INTO approval_request \
         (id, company_id, command, input, preview, reads, request_hash, effect, \
          requested_by, reason, status, created_at, expires_at, risk, counterparty) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(request.id.as_str()),
            SeaValue::from(request.company_id.as_str()),
            SeaValue::from(request.command.as_str()),
            SeaValue::from(request.input.as_str()),
            SeaValue::from(request.preview.as_str()),
            SeaValue::from(request.reads.as_str()),
            SeaValue::from(request.request_hash.as_str()),
            SeaValue::from(effect_json.as_str()),
            SeaValue::from(request.requested_by.as_str()),
            SeaValue::from(request.reason.as_str()),
            SeaValue::from(Status::Pending.as_str()),
            SeaValue::from(request.created_at.as_str()),
            SeaValue::from(request.expires_at.as_str()),
            SeaValue::from(request.risk.as_str()),
            SeaValue::from(request.counterparty.clone()),
        ],
    );
    Ok((statement, request))
}

/// The risk of a money effect. Low only for a single currency whose amount is
/// within that currency's low-risk limit; everything else is high (no money
/// movement, several currencies, no limit for the currency, or above the limit).
#[must_use]
pub fn risk_for_effect(
    effect: &BTreeMap<String, String>,
    low_risk_limits: &BTreeMap<String, String>,
) -> Risk {
    let mut amounts = effect.iter();
    let (Some((code, amount)), None) = (amounts.next(), amounts.next()) else {
        return Risk::High;
    };
    let Some(limit) = low_risk_limits.get(code) else {
        return Risk::High;
    };
    let Ok(currency) = Currency::from_code(code) else {
        return Risk::High;
    };
    match (
        Amount::parse_canonical(amount, currency),
        Amount::parse_canonical(limit, currency),
    ) {
        (Ok(amount), Ok(limit)) if amount <= limit => Risk::Low,
        _ => Risk::High,
    }
}

/// Checks a bulk approval selection: every request low risk and in one currency.
///
/// # Errors
/// `BulkRefused` when the selection is empty, holds a high-risk request, or mixes currencies.
pub fn check_bulk(requests: &[ApprovalRequest]) -> Result<(), ApprovalError> {
    if requests.is_empty() {
        return Err(ApprovalError::BulkRefused(
            "no requests selected".to_owned(),
        ));
    }
    if requests.iter().any(|r| r.risk != Risk::Low) {
        return Err(ApprovalError::BulkRefused(
            "high-risk requests are approved one at a time".to_owned(),
        ));
    }
    let currencies: BTreeSet<&str> = requests
        .iter()
        .flat_map(|r| r.effect.keys().map(String::as_str))
        .collect();
    if currencies.len() != 1 {
        return Err(ApprovalError::BulkRefused(
            "bulk approval needs requests in one currency".to_owned(),
        ));
    }
    Ok(())
}

/// Which approval requests an inbox listing returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listing {
    /// Requests awaiting a decision that have not expired.
    Pending,
    /// Requests already decided: approved, rejected, expired or superseded.
    Decided,
    /// Every request.
    All,
}

/// Lists a company's approval requests, newest first, and the number pending now.
///
/// # Errors
/// A database or row error.
pub async fn list_requests(
    db: &dyn Database,
    company_id: &str,
    listing: Listing,
    now: &str,
    limit: u32,
) -> Result<(Vec<ApprovalRequest>, i64), ApprovalError> {
    let filter = match listing {
        Listing::Pending => "AND status = 'pending' AND expires_at > ?",
        Listing::Decided => "AND status <> 'pending'",
        Listing::All => "",
    };
    let mut values = vec![SeaValue::from(company_id)];
    if listing == Listing::Pending {
        values.push(SeaValue::from(now));
    }
    values.push(SeaValue::from(i64::from(limit)));
    let rows = db
        .query(&Statement::with_values(
            format!(
                "SELECT id FROM approval_request WHERE company_id = ? {filter} \
                 ORDER BY created_at DESC, id DESC LIMIT ?"
            ),
            values,
        ))
        .await
        .map_err(|e| ApprovalError::Database(e.to_string()))?;
    let mut requests = Vec::new();
    for row in &rows.rows {
        let id = row
            .get::<String>("id")
            .ok_or_else(|| ApprovalError::Database("missing id column".to_string()))?;
        requests.push(load_approval_request(db, &id).await?);
    }

    let count_rows = db
        .query(&Statement::with_values(
            "SELECT COUNT(*) AS n FROM approval_request \
             WHERE company_id = ? AND status = 'pending' AND expires_at > ?"
                .to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(now)],
        ))
        .await
        .map_err(|e| ApprovalError::Database(e.to_string()))?;
    let pending = count_rows
        .first()
        .and_then(|row| row.get::<i64>("n"))
        .unwrap_or(0);
    Ok((requests, pending))
}

/// One approval request of a company.
///
/// # Errors
/// `NotFound` when the id names no request of this company.
pub async fn get_request(
    db: &dyn Database,
    company_id: &str,
    approval_id: &str,
) -> Result<ApprovalRequest, ApprovalError> {
    let request = load_approval_request(db, approval_id)
        .await
        .map_err(|_| ApprovalError::NotFound)?;
    if request.company_id != company_id {
        return Err(ApprovalError::NotFound);
    }
    Ok(request)
}

/// Whether a sign-in at `signed_in_at` (epoch seconds) is fresh at `now`.
fn signed_in_recently(signed_in_at: Option<i64>, now: i64) -> bool {
    signed_in_at.is_some_and(|at| now - at <= FRESH_SIGN_IN_SECS && at <= now + 60)
}

/// A nonce row loaded from the database.
struct NonceRow {
    nonce_hex: String,
    nonce_bytes: Vec<u8>,
}

/// Loads an approval request from the database by id.
async fn load_approval_request(
    db: &dyn Database,
    approval_id: &str,
) -> Result<ApprovalRequest, ApprovalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT id, company_id, command, input, preview, reads, request_hash, effect, \
             requested_by, reason, status, created_at, expires_at, decided_by, credential_id, \
             risk, counterparty, decided_at, decision_reason, result, audit_ref \
             FROM approval_request WHERE id = ?"
                .to_owned(),
            vec![SeaValue::from(approval_id)],
        ))
        .await
        .map_err(|e| ApprovalError::Database(e.to_string()))?;

    let row = rows.first().ok_or_else(|| {
        ApprovalError::Other(format!("approval request not found: {approval_id}"))
    })?;

    let id = row
        .get::<String>("id")
        .ok_or_else(|| ApprovalError::Database("missing id column".to_string()))?;
    let company_id = row
        .get::<String>("company_id")
        .ok_or_else(|| ApprovalError::Database("missing company_id column".to_string()))?;
    let command = row
        .get::<String>("command")
        .ok_or_else(|| ApprovalError::Database("missing command column".to_string()))?;
    let input = row
        .get::<String>("input")
        .ok_or_else(|| ApprovalError::Database("missing input column".to_string()))?;
    let preview = row
        .get::<String>("preview")
        .ok_or_else(|| ApprovalError::Database("missing preview column".to_string()))?;
    let reads = row
        .get::<String>("reads")
        .ok_or_else(|| ApprovalError::Database("missing reads column".to_string()))?;
    let request_hash = row
        .get::<String>("request_hash")
        .ok_or_else(|| ApprovalError::Database("missing request_hash column".to_string()))?;
    let effect_json = row
        .get::<String>("effect")
        .ok_or_else(|| ApprovalError::Database("missing effect column".to_string()))?;
    let requested_by = row
        .get::<String>("requested_by")
        .ok_or_else(|| ApprovalError::Database("missing requested_by column".to_string()))?;
    let reason = row
        .get::<String>("reason")
        .ok_or_else(|| ApprovalError::Database("missing reason column".to_string()))?;
    let status_str = row
        .get::<String>("status")
        .ok_or_else(|| ApprovalError::Database("missing status column".to_string()))?;
    let created_at = row
        .get::<String>("created_at")
        .ok_or_else(|| ApprovalError::Database("missing created_at column".to_string()))?;
    let expires_at = row
        .get::<String>("expires_at")
        .ok_or_else(|| ApprovalError::Database("missing expires_at column".to_string()))?;
    let decided_by = row.get::<String>("decided_by");
    let credential_id = row.get::<String>("credential_id");
    let risk_str = row
        .get::<String>("risk")
        .unwrap_or_else(|| "high".to_owned());
    let risk = Risk::parse(&risk_str).map_err(ApprovalError::Other)?;
    let counterparty = row.get::<String>("counterparty");
    let decided_at = row.get::<String>("decided_at");
    let decision_reason = row.get::<String>("decision_reason");
    let result = row.get::<String>("result");
    let audit_ref = row.get::<String>("audit_ref");

    let status = Status::parse(&status_str).map_err(ApprovalError::Other)?;
    let effect: BTreeMap<String, String> = serde_json::from_str(&effect_json)
        .map_err(|e| ApprovalError::Other(format!("invalid effect JSON: {e}")))?;

    Ok(ApprovalRequest {
        id,
        company_id,
        command,
        input,
        preview,
        reads,
        request_hash,
        effect,
        requested_by,
        reason,
        status,
        created_at,
        expires_at,
        decided_by,
        credential_id,
        risk,
        counterparty,
        decided_at,
        decision_reason,
        result,
        audit_ref,
    })
}

/// Loads and verifies a nonce for this approval.
/// The nonce must exist, be for this approval, and not be used yet.
async fn load_and_verify_nonce(
    db: &dyn Database,
    approval_id: &str,
    now: &str,
) -> Result<NonceRow, ApprovalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT nonce, request_hash, expires_at FROM approval_nonce \
             WHERE approval_id = ? AND used_at IS NULL ORDER BY expires_at DESC LIMIT 1"
                .to_owned(),
            vec![SeaValue::from(approval_id)],
        ))
        .await
        .map_err(|e| ApprovalError::Database(e.to_string()))?;

    let row = rows.first().ok_or_else(|| {
        ApprovalError::Other(format!("nonce not found for approval: {approval_id}"))
    })?;

    let nonce_hex = row
        .get::<String>("nonce")
        .ok_or_else(|| ApprovalError::Database("missing nonce column".to_string()))?;
    let expires_at_str = row
        .get::<String>("expires_at")
        .ok_or_else(|| ApprovalError::Database("missing expires_at column".to_string()))?;

    // Check nonce is not expired
    let now_ts = parse_rfc3339(now)
        .map_err(|e| ApprovalError::Other(format!("invalid now timestamp: {e}")))?;
    let expires_ts = parse_rfc3339(&expires_at_str)
        .map_err(|e| ApprovalError::Other(format!("invalid nonce expires_at: {e}")))?;
    if now_ts > expires_ts {
        return Err(ApprovalError::Other("nonce has expired".to_string()));
    }

    // Decode hex nonce to bytes
    let nonce_bytes = hex::decode(&nonce_hex)
        .map_err(|e| ApprovalError::Other(format!("invalid nonce hex encoding: {e}")))?;

    Ok(NonceRow {
        nonce_hex,
        nonce_bytes,
    })
}

/// Extracts the money effect (per-currency totals) from a preview JSON.
fn extract_effect(
    preview_json: &serde_json::Value,
) -> Result<BTreeMap<String, String>, ApprovalError> {
    let totals = preview_json
        .get("totals")
        .and_then(|v| v.as_object())
        .ok_or_else(|| ApprovalError::Other("no totals in preview".to_string()))?;

    let mut effect = BTreeMap::new();
    for (currency, amount) in totals {
        if let Some(amount_str) = amount.as_str() {
            effect.insert(currency.clone(), amount_str.to_owned());
        }
    }
    Ok(effect)
}

/// Builds a human-readable card for the approval request.
fn build_card(
    request: &ApprovalRequest,
    counterparty: Option<&str>,
    payee_bank: Option<&str>,
    bank_changed: bool,
) -> ApprovalCard {
    let mut text = format!(
        "**Approval Request**\n\nCommand: `{}`\n\nReason: {}\n",
        request.command, request.reason
    );

    if !request.effect.is_empty() {
        text.push_str("\nEffect:\n");
        for (currency, amount) in &request.effect {
            let _ = writeln!(text, "- {amount} {currency}");
        }
    }

    if let Some(counterparty) = counterparty {
        let escaped = escape_document_text(counterparty);
        let _ = writeln!(
            text,
            "\nCounterparty (from document, unverified): {escaped}"
        );
    }

    if let Some(payee_bank) = payee_bank {
        let escaped = escape_document_text(payee_bank);
        let _ = writeln!(text, "\nPays to (from document, unverified): {escaped}");
        if bank_changed {
            let _ = writeln!(
                text,
                "\n⚠ Bank details differ from this vendor's known details — verify by phone before approving."
            );
        }
    }

    let _ = writeln!(
        text,
        "\n[View approval]({})",
        build_approval_url(&request.id)
    );

    ApprovalCard {
        approval_id: request.id.clone(),
        url: build_approval_url(&request.id),
        card: text,
    }
}

fn build_approval_url(approval_id: &str) -> String {
    // Would use config.base_url in real implementation
    format!("/approvals/{approval_id}")
}

/// Parses an RFC 3339 timestamp to Unix seconds.
fn parse_rfc3339(s: &str) -> Result<i64, ApprovalError> {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    OffsetDateTime::parse(s, &Rfc3339)
        .map(OffsetDateTime::unix_timestamp)
        .map_err(|e| ApprovalError::Other(format!("invalid timestamp: {e}")))
}

/// Formats Unix seconds to RFC 3339.
fn format_rfc3339(secs: i64) -> Result<String, ApprovalError> {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    let dt = OffsetDateTime::from_unix_timestamp(secs)
        .map_err(|e| ApprovalError::Other(format!("invalid timestamp: {e}")))?;
    dt.format(&Rfc3339)
        .map_err(|e| ApprovalError::Other(format!("format error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_effect_from_preview() {
        let preview = serde_json::json!({
            "lines": [],
            "totals": {
                "EUR": "10000",
                "USD": "11000"
            }
        });

        let effect = extract_effect(&preview).unwrap();
        assert_eq!(effect.get("EUR").map(String::as_str), Some("10000"));
        assert_eq!(effect.get("USD").map(String::as_str), Some("11000"));
    }

    #[test]
    fn approval_card_escapes_and_labels_document_data() {
        // Test escaping: markdown, bidi overrides, truncation
        assert!(escape_document_text("Acme**[evil](url)").contains("\\*\\*"));
        assert!(escape_document_text("Vendor\u{202A}Name").len() < "Vendor\u{202A}Name".len());
        // 120 characters plus the one-character truncation marker.
        assert_eq!(escape_document_text(&"a".repeat(150)).chars().count(), 121);

        // Test card labels and bank warning
        let request = ApprovalRequest {
            id: "appr1".to_string(),
            company_id: "co1".to_string(),
            command: "pay".to_string(),
            input: "{}".to_string(),
            preview: "{}".to_string(),
            reads: "{}".to_string(),
            request_hash: "hash1".to_string(),
            effect: BTreeMap::new(),
            requested_by: "agent1".to_string(),
            reason: "Payout".to_string(),
            status: Status::Pending,
            created_at: "2026-01-15T00:00:00Z".to_string(),
            expires_at: "2026-01-15T01:00:00Z".to_string(),
            decided_by: None,
            credential_id: None,
            risk: Risk::High,
            counterparty: None,
            decided_at: None,
            decision_reason: None,
            result: None,
            audit_ref: None,
        };
        let card = build_card(
            &request,
            Some("Acme GmbH"),
            Some("DE89370400440532013000"),
            true,
        );
        assert!(card.card.contains("from document, unverified"));
        assert!(card.card.contains("Pays to"));
        assert!(card.card.contains("⚠ Bank details differ"));
    }
}
