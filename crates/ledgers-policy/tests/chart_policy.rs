//! The chart-of-accounts gate: an agent key may change the chart only when
//! its policy says so; a person is never gated here (their role is checked
//! at sign-in); reads are never gated.

use ledgers_commands::{CommandMeta, Decision, JournalPreview, Limits, Policy, Principal, Tier};
use ledgers_policy::key::AgentKey;
use ledgers_policy::policy::CHART_WRITE_COMMANDS;
use ledgers_policy::policy::KeyPolicy;
use std::collections::{BTreeMap, BTreeSet};
use time::{OffsetDateTime, UtcOffset};

fn policy(chart_changes: bool) -> (KeyPolicy, Principal) {
    let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
    let key = AgentKey {
        id: "key-chart".to_string(),
        label: "Bookkeeper".to_string(),
        owner: "owner-1".to_string(),
        tier: Tier::PostWithinLimits,
        limits: BTreeMap::new(),
        allowed_journals: None,
        allowed_accounts: None,
        expires_at: None,
        created_at: now,
        last_used_at: None,
        revoked_at: None,
        chart_changes,
        unit: None,
    };
    let keys = BTreeMap::from([("key-chart".to_string(), key)]);
    let policy = KeyPolicy::new(keys, BTreeSet::new(), now, UtcOffset::UTC);
    let agent = Principal::Agent {
        key_id: "key-chart".to_string(),
        tier: Tier::PostWithinLimits,
        limits: Limits::new(),
    };
    (policy, agent)
}

fn meta(name: &str) -> CommandMeta {
    CommandMeta {
        name: name.to_string(),
        tier: Tier::Draft,
        human_only: false,
        supports_dry_run: false,
        idempotent: false,
    }
}

#[test]
fn an_agent_without_the_flag_is_refused_every_chart_write() {
    let (policy, agent) = policy(false);
    for name in CHART_WRITE_COMMANDS {
        let decision = policy.decide(&agent, &meta(name), &JournalPreview::empty());
        match decision {
            Decision::Deny { reason } => assert!(
                reason.contains("chart of accounts"),
                "{name} is refused for the chart reason: {reason}"
            ),
            other => panic!("{name} must be denied for an agent without the flag, got {other:?}"),
        }
    }
}

#[test]
fn an_agent_with_the_flag_may_write_the_chart() {
    let (policy, agent) = policy(true);
    for name in CHART_WRITE_COMMANDS {
        let decision = policy.decide(&agent, &meta(name), &JournalPreview::empty());
        assert!(
            matches!(decision, Decision::Allow),
            "{name} is allowed once the policy says so, got {decision:?}"
        );
    }
}

#[test]
fn reads_and_persons_are_not_gated_by_the_chart_rule() {
    let (policy, agent) = policy(false);
    let read = policy.decide(&agent, &meta("accounts.get"), &JournalPreview::empty());
    assert!(
        matches!(read, Decision::Allow),
        "reads are not gated: {read:?}"
    );
    let person = Principal::Person {
        person_id: "usr_1".to_string(),
        role: ledgers_commands::Role::Owner,
    };
    let write = policy.decide(&person, &meta("accounts.create"), &JournalPreview::empty());
    assert!(
        matches!(write, Decision::Allow),
        "a person may write: {write:?}"
    );
}
