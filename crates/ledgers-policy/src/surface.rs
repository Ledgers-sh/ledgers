//! Command surface: `policy.put_key`, `policy.revoke_key`, and `policy.explain`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `PutKey`: create or update an agent key with new policy.
///
/// Tier: `PostWithinLimits`
/// `HumanOnly`: `true` (agents cannot change policy)
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PutKeyInput {
    /// The key id to create or update.
    pub key_id: String,
    /// Human-readable label.
    pub label: String,
    /// Tier: `read`, `draft`, or `post_within_limits`.
    pub tier: String,
    /// Per-currency limits: map of currency code to (`per_entry`, `per_day`).
    pub limits: BTreeMap<String, (i128, i128)>,
    /// Optional expiry (RFC3339), or None if no expiry.
    pub expires_at: Option<String>,
}

/// `PutKeyOutput`: the created/updated key.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PutKeyOutput {
    /// The key id.
    pub key_id: String,
    /// Success message.
    pub message: String,
}

/// `RevokeKey`: mark an agent key as revoked.
///
/// Tier: `PostWithinLimits`
/// `HumanOnly`: `true`
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RevokeKeyInput {
    /// The key id to revoke.
    pub key_id: String,
}

/// `RevokeKeyOutput`: confirmation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RevokeKeyOutput {
    /// The revoked key id.
    pub key_id: String,
    /// Success message.
    pub message: String,
}

/// `PolicyExplain`: get a decision explanation for a command.
///
/// Tier: `Read`
/// `HumanOnly`: `false`
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PolicyExplainInput {
    /// The command name to evaluate.
    pub command_name: String,
    /// The agent key id.
    pub key_id: String,
    /// Per-currency effect preview (optional).
    pub effect: Option<BTreeMap<String, i128>>,
}

/// `PolicyExplainOutput`: the decision explanation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PolicyExplainOutput {
    /// The final decision: `"Allow"`, `"NeedsApproval"`, or `"Deny"`.
    pub decision: String,
    /// The command class.
    pub class: String,
    /// The steps taken in the decision.
    pub steps: Vec<String>,
    /// The reason (if any).
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_key_input_serializable() {
        let input = PutKeyInput {
            key_id: "key-1".to_string(),
            label: "Test".to_string(),
            tier: "post_within_limits".to_string(),
            limits: BTreeMap::new(),
            expires_at: None,
        };
        let json = serde_json::to_string(&input).expect("serialize");
        assert!(!json.is_empty());
    }

    #[test]
    fn test_revoke_key_input_serializable() {
        let input = RevokeKeyInput {
            key_id: "key-1".to_string(),
        };
        let json = serde_json::to_string(&input).expect("serialize");
        assert!(!json.is_empty());
    }

    #[test]
    fn test_explain_input_serializable() {
        let input = PolicyExplainInput {
            command_name: "test.post".to_string(),
            key_id: "key-1".to_string(),
            effect: None,
        };
        let json = serde_json::to_string(&input).expect("serialize");
        assert!(!json.is_empty());
    }
}
