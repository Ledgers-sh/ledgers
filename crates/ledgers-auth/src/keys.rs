//! Agent key management on the cratefield `ApiKeys` harness.
//!
//! Agent keys live on [`cratefield_core::ApiKeys`] with namespace "lk".
//! This module provides a wrapper [`AgentKeys`] for issue/rotate/revoke,
//! and an [`Authenticator`] impl ([`KeyAuthenticator`]) that verifies
//! bearer credentials via the harness (NO caching, so a revoked key fails
//! on the very next request).
//!
//! Each issue/rotate/revoke immediately appends an audit event with a
//! Person actor and the public key prefix (never the secret, never the full key).
//! Tier and limits come from a directory implementation passed at construction —
//! typically `ledgers_policy::KeyPolicy` or a test double.
//!
//! Note: the audit event is recorded *after* the harness `ApiKeys` write commits.
//! A crash between the two can leave a key operation unaudited.

use async_trait::async_trait;
use cratefield_core::{ApiKeyMode, ApiKeys, Database, DbError};
use ledgers_audit::{AuditConfig, AuditContext, AuditRecord, Outcome, record};
use ledgers_commands::{Authenticator, CommandError, Credential, Limits, Principal, Tier};
use ledgers_db::retry::RetryPolicy;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use time::OffsetDateTime;

/// No-op sleep function for audit record batches (used instead of actual async sleeping).
fn no_sleep(_: Duration) {}

/// Records an audit event for a key operation.
async fn log_key_audit(
    db: &dyn Database,
    action: &str,
    prefix: &str,
    company_id: &str,
    ctx: &AuditContext,
    now: &str,
) -> Result<(), KeyError> {
    let audit_record = AuditRecord::from_input(
        action,
        &serde_json::json!({ "public_prefix": prefix }),
        Outcome::Allowed,
        vec![prefix.to_string()],
        None,
    );
    record(
        db,
        &AuditConfig::default(),
        company_id,
        ctx,
        &audit_record,
        now,
        RetryPolicy::default(),
        no_sleep,
    )
    .await
    .map(|_| ())
    .map_err(|e| KeyError::Audit(e.to_string()))
}

/// The namespace for Ledgers agent keys on the harness `ApiKeys`.
pub const KEY_NAMESPACE: &str = "lk";

/// Errors that can occur during key operations.
#[derive(Debug, Clone, Error)]
pub enum KeyError {
    /// The harness `ApiKeys` operation failed.
    #[error("key operation failed: {0}")]
    ApiKeys(String),
    /// The directory lookup failed.
    #[error("directory lookup failed: {0}")]
    DirectoryLookup(String),
    /// The credential format is invalid.
    #[error("invalid credential format")]
    InvalidFormat,
    /// The credential is missing or malformed.
    #[error("missing or malformed credential")]
    MalformedCredential,
    /// An audit operation failed.
    #[error("audit failed: {0}")]
    Audit(String),
}

impl From<cratefield_core::ApiKeyError> for KeyError {
    fn from(err: cratefield_core::ApiKeyError) -> Self {
        KeyError::ApiKeys(err.to_string())
    }
}

impl From<ledgers_audit::AuditError> for KeyError {
    fn from(err: ledgers_audit::AuditError) -> Self {
        KeyError::Audit(err.to_string())
    }
}

impl From<DbError> for KeyError {
    fn from(err: DbError) -> Self {
        KeyError::Audit(format!("database error: {err}"))
    }
}

/// Public metadata returned when an agent key is issued.
///
/// The full key string (secret included) is returned exactly once via
/// [`AgentKeyIssue::key`]; further access is denied.
#[derive(Clone)]
pub struct AgentKeyIssue {
    /// The key id (public prefix `lk_<mode>_<id>`).
    pub key_id: String,
    /// The full key string, including secret. This is shown once only.
    full_key: String,
}

impl AgentKeyIssue {
    /// Creates a new issued key with a public key id and full token.
    #[must_use]
    pub fn new(key_id: String, full_key: String) -> Self {
        Self { key_id, full_key }
    }

    /// Returns the full key string (secret included), consuming the value
    /// so it cannot be accessed again.
    #[must_use]
    pub fn key(self) -> String {
        self.full_key
    }
}

impl std::fmt::Debug for AgentKeyIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentKeyIssue")
            .field("key_id", &self.key_id)
            .field("full_key", &"<REDACTED>")
            .finish()
    }
}

/// Trait for resolving a key id to its tier and limits.
///
/// Implementers typically delegate to [`ledgers_policy`] or return a
/// test double.
#[async_trait(?Send)]
pub trait AgentKeyDirectory: Send + Sync {
    /// Looks up tier and limits for a key, along with expiry/revocation status.
    async fn lookup(&self, key_id: &str) -> Result<KeyDirectoryEntry, KeyError>;
}

/// The directory entry returned by [`AgentKeyDirectory::lookup`].
#[derive(Clone, Debug)]
pub struct KeyDirectoryEntry {
    /// The tier for this key.
    pub tier: Tier,
    /// Per-currency limits.
    pub limits: Limits,
    /// Expiry time, if any.
    pub expires_at: Option<OffsetDateTime>,
    /// Revocation time, if any (should deny the key).
    pub revoked_at: Option<OffsetDateTime>,
}

/// A wrapper around the harness [`ApiKeys`] for agent key operations.
///
/// Issue, rotate, revoke, and pass-through last-used tracking. Each
/// operation logs to the audit chain.
pub struct AgentKeys {
    api_keys: Arc<ApiKeys>,
    db: Arc<dyn Database>,
}

impl AgentKeys {
    /// Wraps the harness `ApiKeys` for agent key operations.
    #[must_use]
    pub fn new(api_keys: Arc<ApiKeys>, db: Arc<dyn Database>) -> Self {
        Self { api_keys, db }
    }

    /// Issues a new agent key bound to an agent key id from policy.
    ///
    /// Returns the public key prefix and the full key string (shown once).
    /// The key is issued with a single scope (the key id itself).
    /// Appends an audit event immediately.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] if the `ApiKeys` or audit operation fails.
    pub async fn issue(
        &self,
        key_id: &str,
        company_id: &str,
        ctx: &AuditContext,
        now: &str,
    ) -> Result<AgentKeyIssue, KeyError> {
        let issued = self
            .api_keys
            .issue(KEY_NAMESPACE, key_id, &[key_id], ApiKeyMode::Live)
            .await?;

        let public_prefix = issued.prefix.clone();
        log_key_audit(
            self.db.as_ref(),
            "auth.key.issue",
            &public_prefix,
            company_id,
            ctx,
            now,
        )
        .await?;

        Ok(AgentKeyIssue::new(public_prefix, issued.token))
    }

    /// Rotates an existing agent key: generates a new secret, keeps the id and namespace.
    ///
    /// The old key continues to work until explicitly revoked.
    /// Returns the new key wrapped in [`AgentKeyIssue`] (shown once).
    /// Appends an audit event immediately.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] if the `ApiKeys` or audit operation fails.
    pub async fn rotate(
        &self,
        prefix: &str,
        company_id: &str,
        ctx: &AuditContext,
        now: &str,
    ) -> Result<AgentKeyIssue, KeyError> {
        let rotated = self.api_keys.rotate(prefix).await?;
        log_key_audit(
            self.db.as_ref(),
            "auth.key.rotate",
            prefix,
            company_id,
            ctx,
            now,
        )
        .await?;
        Ok(AgentKeyIssue::new(rotated.prefix, rotated.token))
    }

    /// Revokes an agent key (idempotent).
    ///
    /// Once revoked, the key fails on the next authenticate call.
    /// Appends an audit event immediately.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] if the `ApiKeys` or audit operation fails.
    pub async fn revoke(
        &self,
        prefix: &str,
        company_id: &str,
        ctx: &AuditContext,
        now: &str,
    ) -> Result<(), KeyError> {
        self.api_keys.revoke(prefix).await?;
        log_key_audit(
            self.db.as_ref(),
            "auth.key.revoke",
            prefix,
            company_id,
            ctx,
            now,
        )
        .await
    }
}

/// Trait for providing the current time, used instead of `OffsetDateTime::now_utc()`
/// to support wasm32-unknown-unknown and enable deterministic testing.
pub trait Clock: Send + Sync {
    /// Returns the current time.
    fn now(&self) -> OffsetDateTime;
}

/// Authenticator impl that verifies bearer credentials via harness `ApiKeys`.
///
/// No caching: every authenticate call goes to `ApiKeys` (via
/// [`cratefield_core::require_api_key`]), so a revoked key fails on the
/// very next request. The directory provides tier and limits.
pub struct KeyAuthenticator {
    api_keys: Arc<ApiKeys>,
    directory: Arc<dyn AgentKeyDirectory>,
    clock: Arc<dyn Clock>,
}

impl KeyAuthenticator {
    /// Creates a new key authenticator.
    #[must_use]
    pub fn new(
        api_keys: Arc<ApiKeys>,
        directory: Arc<dyn AgentKeyDirectory>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            api_keys,
            directory,
            clock,
        }
    }
}

#[async_trait(?Send)]
impl Authenticator for KeyAuthenticator {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        // Verify the key through the harness ApiKeys.verify(). No caching:
        // a revoked key fails on the next call.
        let principal = self
            .api_keys
            .verify(credential)
            .await
            .map_err(|_| CommandError::Unauthenticated)?
            .ok_or(CommandError::Unauthenticated)?;

        // Look up tier and limits from the directory for this key subject.
        let dir_entry = self
            .directory
            .lookup(&principal.subject)
            .await
            .map_err(|_| CommandError::Unauthenticated)?;

        // Check expiry and revocation using the injected clock.
        let now = self.clock.now();
        if let Some(revoked_at) = dir_entry.revoked_at
            && revoked_at <= now
        {
            return Err(CommandError::Unauthenticated);
        }
        if let Some(expires_at) = dir_entry.expires_at
            && expires_at <= now
        {
            return Err(CommandError::Unauthenticated);
        }

        Ok(Principal::Agent {
            key_id: principal.prefix,
            tier: dir_entry.tier,
            limits: dir_entry.limits,
        })
    }
}
