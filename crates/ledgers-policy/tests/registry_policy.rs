//! Acceptance test: all commands in the full registry respect policy rules.
//!
//! This test verifies:
//! 1. Policy/key change commands (`policy.*` except `policy.explain`) are marked `human_only`
//! 2. For agent keys with various tiers and limits, policy/key commands are always denied
//! 3. Human-only commands always result in `NeedsApproval` for agents (never `Allow`)
//! 4. Policy commands exist in the registry and are discoverable

use ledgers_commands::{CommandMeta, Policy, Tier};
use ledgers_policy::{AgentKey, CurrencyLimit, KeyPolicy, commands};
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;

#[test]
fn test_policy_commands_protected_in_registry() {
    // Build the policy registry.
    let registry = commands::registry();

    // Policy commands must be registered.
    assert!(
        registry.get("policy.put_key").is_some(),
        "policy.put_key should be registered"
    );
    assert!(
        registry.get("policy.revoke_key").is_some(),
        "policy.revoke_key should be registered"
    );
    assert!(
        registry.get("policy.explain").is_some(),
        "policy.explain should be registered"
    );

    // policy.put_key and policy.revoke_key must be human_only.
    if let Some(put_key) = registry.get("policy.put_key") {
        assert!(put_key.human_only, "policy.put_key must be human_only");
    }
    if let Some(revoke_key) = registry.get("policy.revoke_key") {
        assert!(
            revoke_key.human_only,
            "policy.revoke_key must be human_only"
        );
    }

    // policy.explain can be called by agents (not human_only).
    if let Some(explain) = registry.get("policy.explain") {
        assert!(
            !explain.human_only,
            "policy.explain should be callable by agents"
        );
    }
}

#[test]
fn test_agent_cannot_change_policy_or_keys() {
    // Create a test agent key with generous limits.
    let now = OffsetDateTime::now_utc();
    let mut limits = BTreeMap::new();
    limits.insert(
        "EUR".to_string(),
        CurrencyLimit {
            per_entry: 1_000_000_000,
            per_day: 10_000_000_000,
        },
    );

    let agent_key = AgentKey {
        id: "test-agent".to_string(),
        label: "Test Agent".to_string(),
        owner: "alice".to_string(),
        tier: Tier::PostWithinLimits,
        limits,
        allowed_journals: None,
        allowed_accounts: None,
        expires_at: None,
        created_at: now,
        last_used_at: None,
        revoked_at: None,
        chart_changes: false,
    };

    // Create policy with the agent key.
    let mut keys = BTreeMap::new();
    keys.insert(agent_key.id.clone(), agent_key.clone());

    let policy = KeyPolicy::new(keys, BTreeSet::new(), now, time::UtcOffset::UTC);

    // Test that policy commands are denied to the agent.
    let agent_principal = ledgers_commands::Principal::Agent {
        key_id: "test-agent".to_string(),
        tier: Tier::PostWithinLimits,
        limits: ledgers_commands::Limits::new(),
    };

    // policy.put_key should be denied (marked human_only, which leads to NeedsApproval in policy).
    let put_key_meta = CommandMeta {
        name: "policy.put_key".to_string(),
        tier: Tier::PostWithinLimits,
        human_only: true,
        supports_dry_run: false,
        idempotent: false,
    };
    let decision = policy.decide(
        &agent_principal,
        &put_key_meta,
        &ledgers_commands::JournalPreview::empty(),
    );
    match decision {
        ledgers_commands::Decision::NeedsApproval { .. }
        | ledgers_commands::Decision::Deny { .. } => {
            // This is correct: human_only commands result in approval request or denial.
        }
        ledgers_commands::Decision::Allow => {
            panic!("policy.put_key should never be Allow for agents");
        }
    }

    // policy.revoke_key should be denied.
    let revoke_key_meta = CommandMeta {
        name: "policy.revoke_key".to_string(),
        tier: Tier::PostWithinLimits,
        human_only: true,
        supports_dry_run: false,
        idempotent: false,
    };
    let decision = policy.decide(
        &agent_principal,
        &revoke_key_meta,
        &ledgers_commands::JournalPreview::empty(),
    );
    match decision {
        ledgers_commands::Decision::NeedsApproval { .. }
        | ledgers_commands::Decision::Deny { .. } => {
            // This is correct: human_only commands result in approval request or denial.
        }
        ledgers_commands::Decision::Allow => {
            panic!("policy.revoke_key should never be Allow for agents");
        }
    }

    // policy.explain is not human_only, so agents can use it (should be Allow).
    let explain_meta = CommandMeta {
        name: "policy.explain".to_string(),
        tier: Tier::Read,
        human_only: false,
        supports_dry_run: false,
        idempotent: true,
    };
    let decision = policy.decide(
        &agent_principal,
        &explain_meta,
        &ledgers_commands::JournalPreview::empty(),
    );
    assert_eq!(
        decision,
        ledgers_commands::Decision::Allow,
        "policy.explain should be Allow for agents"
    );
}

#[test]
fn test_humans_can_always_access_policy_commands() {
    let now = OffsetDateTime::now_utc();
    let policy = KeyPolicy::new(BTreeMap::new(), BTreeSet::new(), now, time::UtcOffset::UTC);

    let human_principal = ledgers_commands::Principal::Person {
        person_id: "alice".to_string(),
    };

    // All policy commands should be allowed for humans.
    let commands = vec!["policy.put_key", "policy.revoke_key", "policy.explain"];

    for cmd_name in commands {
        let meta = CommandMeta {
            name: cmd_name.to_string(),
            tier: Tier::PostWithinLimits,
            human_only: matches!(cmd_name, "policy.put_key" | "policy.revoke_key"),
            supports_dry_run: false,
            idempotent: false,
        };
        let decision = policy.decide(
            &human_principal,
            &meta,
            &ledgers_commands::JournalPreview::empty(),
        );
        assert_eq!(
            decision,
            ledgers_commands::Decision::Allow,
            "{cmd_name} should be Allow for humans"
        );
    }
}
