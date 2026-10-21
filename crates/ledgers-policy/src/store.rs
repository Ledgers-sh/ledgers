//! Storage: loading and persisting agent keys.
//!
//! This module provides helpers for serializing/deserializing agent keys
//! to and from the database. The actual database operations are performed
//! through the harness's Statement execution framework.

use crate::AgentKey;
use std::collections::BTreeMap;

/// Serialize an agent key to JSON for storage.
///
/// # Errors
/// Returns an error if serialization fails.
pub fn serialize_key(key: &AgentKey) -> Result<String, serde_json::Error> {
    serde_json::to_string(key)
}

/// Deserialize an agent key from JSON.
///
/// # Errors
/// Returns an error if deserialization fails.
pub fn deserialize_key(json: &str) -> Result<AgentKey, serde_json::Error> {
    serde_json::from_str(json)
}

/// Parse a loaded set of keys from (id, `policy_json`) tuples.
///
/// # Errors
///
/// Returns an error if deserialization fails for any key.
pub fn parse_keys(
    rows: Vec<(String, String)>,
) -> Result<BTreeMap<String, AgentKey>, Box<dyn std::error::Error>> {
    let mut keys = BTreeMap::new();
    for (id, policy_json) in rows {
        let key = deserialize_key(&policy_json)?;
        keys.insert(id, key);
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CurrencyLimit;
    use ledgers_commands::Tier;

    #[test]
    fn test_serialize_deserialize_key() {
        let now = time::OffsetDateTime::now_utc();
        let mut limits = BTreeMap::new();
        limits.insert(
            "EUR".to_string(),
            CurrencyLimit {
                per_entry: 10_000,
                per_day: 100_000,
            },
        );

        let key = AgentKey {
            id: "key-1".to_string(),
            label: "Test".to_string(),
            owner: "alice".to_string(),
            tier: Tier::PostWithinLimits,
            limits,
            allowed_journals: None,
            allowed_accounts: None,
            expires_at: None,
            created_at: now,
            last_used_at: None,
            revoked_at: None,
        };

        let json = serialize_key(&key).expect("serialize");
        let restored = deserialize_key(&json).expect("deserialize");
        assert_eq!(key.id, restored.id);
        assert_eq!(key.tier, restored.tier);
    }

    #[test]
    fn test_parse_keys() {
        let now = time::OffsetDateTime::now_utc();
        let mut limits = BTreeMap::new();
        limits.insert(
            "EUR".to_string(),
            CurrencyLimit {
                per_entry: 10_000,
                per_day: 100_000,
            },
        );

        let key = AgentKey {
            id: "key-1".to_string(),
            label: "Test".to_string(),
            owner: "alice".to_string(),
            tier: Tier::PostWithinLimits,
            limits,
            allowed_journals: None,
            allowed_accounts: None,
            expires_at: None,
            created_at: now,
            last_used_at: None,
            revoked_at: None,
        };

        let json = serialize_key(&key).expect("serialize");
        let rows = vec![("key-1".to_string(), json)];
        let keys = parse_keys(rows).expect("parse");
        assert_eq!(keys.len(), 1);
        assert!(keys.contains_key("key-1"));
    }
}
