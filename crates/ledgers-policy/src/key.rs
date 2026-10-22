//! Agent key types: the full policy carried by a credential.
//!
//! An [`AgentKey`] holds the identity, tier, per-currency limits, and lifecycle
//! state (expiry, revocation). The credential itself (bearer token, etc.) lives
//! with the authentication issue; this struct carries only policy fields.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use time::OffsetDateTime;

/// Per-currency limit for an agent key: per-entry and per-rolling-day ceilings.
///
/// Amounts are in minor units (i128), e.g., EUR cents. Both ceilings must be
/// non-negative. A limit of 0 for both means no transactions in that currency
/// are allowed for the agent.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CurrencyLimit {
    /// Maximum single entry amount in minor units; 0 = no transactions allowed.
    pub per_entry: i128,
    /// Maximum rolling-day total in minor units; 0 = no transactions allowed.
    pub per_day: i128,
}

impl CurrencyLimit {
    /// Validates that both limits are non-negative.
    ///
    /// # Errors
    /// Returns an error if either limit is negative.
    pub fn validate(&self) -> Result<(), LimitError> {
        if self.per_entry < 0 {
            return Err(LimitError::NegativeLimit(
                "per_entry cannot be negative".to_string(),
            ));
        }
        if self.per_day < 0 {
            return Err(LimitError::NegativeLimit(
                "per_day cannot be negative".to_string(),
            ));
        }
        Ok(())
    }
}

/// Errors for agent key operations.
#[derive(Clone, Debug, Error)]
pub enum LimitError {
    /// A limit value is negative.
    #[error("negative limit: {0}")]
    NegativeLimit(String),
}

/// An agent key: the full policy with tier, limits, expiry, and revocation status.
///
/// The credential lives with the auth issue; this struct carries only policy fields.
/// Times use RFC3339 serialization.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentKey {
    /// The key's unique identifier.
    pub id: String,
    /// A human-readable label for the key.
    pub label: String,
    /// The person (owner) id that created this key.
    pub owner: String,
    /// The tier: read, draft, or post-within-limits.
    pub tier: ledgers_commands::Tier,
    /// Per-currency limits: per-entry and per-rolling-day in minor units.
    /// A currency with no entry means no allowance in that currency (fail closed).
    pub limits: BTreeMap<String, CurrencyLimit>,
    /// Allowed journal identifiers, or None if all journals are allowed.
    pub allowed_journals: Option<BTreeSet<String>>,
    /// Allowed account identifiers, or None if all accounts are allowed.
    pub allowed_accounts: Option<BTreeSet<String>>,
    /// Expiry time (RFC3339), or None if the key never expires.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// Creation time (RFC3339).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last used time (RFC3339), or None if never used.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    /// Revocation time (RFC3339), or None if not revoked.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    /// Whether this agent may change the chart of accounts (create, update,
    /// archive or restore accounts). Off by default: a chart change is a
    /// human decision unless the key's policy says otherwise. Missing in
    /// stored policies reads as `false`.
    #[serde(default)]
    pub chart_changes: bool,
    /// The business unit this key is pinned to, or `None` for all units.
    /// A pinned key's entries carry its unit: a line with no unit takes it,
    /// a line for another unit is refused (the server enforces both before
    /// the command runs). Missing in stored policies reads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_validate_ok() {
        let limit = CurrencyLimit {
            per_entry: 1000,
            per_day: 10000,
        };
        assert!(limit.validate().is_ok());
    }

    #[test]
    fn limit_validate_zero_ok() {
        let limit = CurrencyLimit {
            per_entry: 0,
            per_day: 0,
        };
        assert!(limit.validate().is_ok());
    }

    #[test]
    fn limit_validate_negative_per_entry() {
        let limit = CurrencyLimit {
            per_entry: -1,
            per_day: 1000,
        };
        assert!(limit.validate().is_err());
    }

    #[test]
    fn limit_validate_negative_per_day() {
        let limit = CurrencyLimit {
            per_entry: 1000,
            per_day: -1,
        };
        assert!(limit.validate().is_err());
    }

    #[test]
    fn agent_key_serialize() {
        let now = time::OffsetDateTime::now_utc();
        let key = AgentKey {
            id: "key-1".to_string(),
            label: "Test Key".to_string(),
            owner: "alice".to_string(),
            tier: ledgers_commands::Tier::PostWithinLimits,
            limits: [(
                "EUR".to_string(),
                CurrencyLimit {
                    per_entry: 10_000,
                    per_day: 100_000,
                },
            )]
            .iter()
            .cloned()
            .collect(),
            allowed_journals: None,
            allowed_accounts: None,
            expires_at: None,
            created_at: now,
            last_used_at: None,
            revoked_at: None,
            chart_changes: false,
            unit: None,
        };

        let json = serde_json::to_string(&key).expect("serialize");
        let deserialized: AgentKey = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(key.id, deserialized.id);
        assert_eq!(key.tier, deserialized.tier);
    }
}
