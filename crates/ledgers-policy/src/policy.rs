//! `KeyPolicy`: the [`ledgers_commands::Policy`] implementation using agent keys.
//!
//! [`KeyPolicy`] holds a set of agent keys indexed by id, protected account sets,
//! and company timezone info. It implements the policy trait: for persons,
//! allow; for agents, look up the key and decide.

use crate::decide::{self, Actor, CommandClass, DecisionInput};
use crate::key::AgentKey;
use crate::limit;
use cratefield_core::Statement;
use ledgers_commands::{CommandError, CommandMeta, Decision, JournalPreview, Policy, Principal};
use ledgers_money::Amount;
use std::collections::{BTreeMap, BTreeSet};
use time::{OffsetDateTime, UtcOffset};

/// The policy engine for agent keys: tier checks, per-currency limits,
/// rolling-day accounting, and protected account protection.
/// The commands that change the chart of accounts. An agent key may run
/// them only when its policy sets `chart_changes`; a person always may
/// (their role is checked at sign-in).
pub const CHART_WRITE_COMMANDS: &[&str] = &[
    "accounts.create",
    "accounts.update",
    "accounts.archive",
    "accounts.restore",
];

pub struct KeyPolicy {
    /// Keys indexed by id.
    keys: BTreeMap<String, AgentKey>,
    /// Protected account identifiers (bank, equity, tax, AR/AP control, etc).
    protected_accounts: BTreeSet<String>,
    /// Current timestamp (for expiry checks).
    now: OffsetDateTime,
    /// Company timezone offset (for rolling-day boundaries).
    company_offset: UtcOffset,
}

impl KeyPolicy {
    /// Adds one key to the engine, replacing any key with the same id. The
    /// server uses this for the in-process seeding principal, which has no
    /// `agent_key` row; a stored key is never replaced by a request.
    #[must_use]
    pub fn with_key(mut self, key: AgentKey) -> Self {
        self.keys.insert(key.id.clone(), key);
        self
    }

    /// Creates a policy engine.
    #[must_use]
    pub fn new(
        keys: BTreeMap<String, AgentKey>,
        protected_accounts: BTreeSet<String>,
        now: OffsetDateTime,
        company_offset: UtcOffset,
    ) -> Self {
        KeyPolicy {
            keys,
            protected_accounts,
            now,
            company_offset,
        }
    }
}

impl Policy for KeyPolicy {
    fn decide(
        &self,
        principal: &Principal,
        meta: &CommandMeta,
        preview: &JournalPreview,
    ) -> Decision {
        match principal {
            Principal::Person { .. } => Decision::Allow,
            Principal::Agent { key_id, .. } => {
                // Look up the key.
                let Some(key) = self.keys.get(key_id) else {
                    return Decision::Deny {
                        reason: format!("unknown agent key: {key_id}"),
                    };
                };

                // Chart changes are a person's decision unless this key's
                // policy says otherwise (docs/PLAN.md section 5).
                if CHART_WRITE_COMMANDS.contains(&meta.name.as_str()) && !key.chart_changes {
                    return Decision::Deny {
                        reason: format!(
                            "{} changes the chart of accounts; an agent key may do that only when its policy allows chart changes",
                            meta.name
                        ),
                    };
                }

                // Build DecisionInput from the preview and key policy.
                let (effect, accounts) = preview_to_effect_and_accounts(preview);

                let input = DecisionInput {
                    now: self.now,
                    effect,
                    used_today: BTreeMap::new(), // Updated by guard statements, not pre-computed.
                    journal: None,               // Journal isn't part of the preview yet.
                    accounts,
                    protected_accounts: self.protected_accounts.clone(),
                    reverses: None, // Not available from preview; would require audit lookup.
                    risk_flags: Vec::new(), // Screening happens in intake, not here yet.
                };

                // Decide.
                let decision = decide::decide(Actor::Agent(key), meta, &input);

                // Convert our Decision to ledgers_commands::Decision.
                match decision {
                    crate::decide::Decision::Allow => Decision::Allow,
                    crate::decide::Decision::NeedsApproval { reason } => {
                        Decision::NeedsApproval { reason }
                    }
                    crate::decide::Decision::Deny { reason } => Decision::Deny { reason },
                }
            }
        }
    }

    fn guards(
        &self,
        principal: &Principal,
        meta: &CommandMeta,
        preview: &JournalPreview,
    ) -> Result<Vec<Statement>, CommandError> {
        match principal {
            Principal::Person { .. } => Ok(Vec::new()),
            Principal::Agent { key_id, .. } => {
                // Look up the key.
                let Some(key) = self.keys.get(key_id) else {
                    return Ok(Vec::new());
                };

                // Determine the command class.
                let class = CommandClass::of(meta);

                // Only emit guard statements for Post-class commands with non-empty effect.
                if class != CommandClass::Post {
                    return Ok(Vec::new());
                }

                let (effect, _) = preview_to_effect_and_accounts(preview);
                if effect.is_empty() {
                    return Ok(Vec::new());
                }

                // Emit consumption statements for rolling-daily limits.
                limit::consume_statements(
                    key_id,
                    &effect,
                    self.now,
                    self.company_offset,
                    &key.limits,
                )
                .map_err(|e| CommandError::InvalidInput(format!("guard statement error: {e}")))
            }
        }
    }
}

/// Helper: converts a `JournalPreview` to per-currency effect and accounts set.
///
/// The effect is per-currency debit-side amounts summed from the preview.
/// Accounts are extracted from all lines.
fn preview_to_effect_and_accounts(
    preview: &JournalPreview,
) -> (BTreeMap<String, i128>, BTreeSet<String>) {
    let mut effect = BTreeMap::new();
    let mut accounts = BTreeSet::new();

    for line in &preview.lines {
        accounts.insert(line.account.clone());
    }
    // Per-currency debit totals: the preview keys them by currency code, and
    // its lines carry bare canonical amounts with no currency to parse.
    for (code, total) in &preview.totals {
        if let Ok(currency) = ledgers_money::Currency::from_code(code)
            && let Ok(amount) = Amount::parse_canonical(total, currency)
        {
            *effect.entry(currency.code().to_string()).or_insert(0) += amount.minor();
        }
    }

    (effect, accounts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CurrencyLimit;
    use ledgers_commands::Tier;

    #[test]
    fn policy_person_always_allowed() {
        let policy = KeyPolicy::new(
            BTreeMap::new(),
            BTreeSet::new(),
            OffsetDateTime::now_utc(),
            UtcOffset::UTC,
        );
        let person = Principal::Person {
            person_id: "alice".to_string(),
        };
        let meta = CommandMeta {
            name: "test.command".to_string(),
            tier: Tier::PostWithinLimits,
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
        };
        let preview = JournalPreview::empty();

        let decision = policy.decide(&person, &meta, &preview);
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    fn policy_agent_unknown_key_denied() {
        let policy = KeyPolicy::new(
            BTreeMap::new(),
            BTreeSet::new(),
            OffsetDateTime::now_utc(),
            UtcOffset::UTC,
        );
        let agent = Principal::Agent {
            key_id: "unknown".to_string(),
            tier: Tier::Draft,
            limits: ledgers_commands::Limits::new(),
        };
        let meta = CommandMeta {
            name: "test.command".to_string(),
            tier: Tier::Draft,
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
        };
        let preview = JournalPreview::empty();

        let decision = policy.decide(&agent, &meta, &preview);
        match decision {
            Decision::Deny { reason } => {
                assert!(reason.contains("unknown"));
            }
            _ => panic!("expected Deny"),
        }
    }

    #[test]
    fn policy_agent_allowed() {
        let now = OffsetDateTime::now_utc();
        let mut keys = BTreeMap::new();
        let mut limits = BTreeMap::new();
        limits.insert(
            "EUR".to_string(),
            CurrencyLimit {
                per_entry: 100_000,
                per_day: 1_000_000,
            },
        );

        keys.insert(
            "key-1".to_string(),
            AgentKey {
                id: "key-1".to_string(),
                label: "Test".to_string(),
                owner: "alice".to_string(),
                tier: Tier::Draft,
                limits,
                allowed_journals: None,
                allowed_accounts: None,
                expires_at: None,
                created_at: now,
                last_used_at: None,
                revoked_at: None,
                chart_changes: false,
            },
        );

        let policy = KeyPolicy::new(keys, BTreeSet::new(), now, UtcOffset::UTC);
        let agent = Principal::Agent {
            key_id: "key-1".to_string(),
            tier: Tier::Draft,
            limits: ledgers_commands::Limits::new(),
        };
        let meta = CommandMeta {
            name: "test.draft".to_string(),
            tier: Tier::Draft,
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
        };
        let preview = JournalPreview::empty();

        let decision = policy.decide(&agent, &meta, &preview);
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    fn policy_agent_revoked_denied() {
        let now = OffsetDateTime::now_utc();
        let mut keys = BTreeMap::new();

        keys.insert(
            "key-1".to_string(),
            AgentKey {
                id: "key-1".to_string(),
                label: "Test".to_string(),
                owner: "alice".to_string(),
                tier: Tier::Draft,
                limits: BTreeMap::new(),
                allowed_journals: None,
                allowed_accounts: None,
                expires_at: None,
                created_at: now,
                last_used_at: None,
                revoked_at: Some(now),
                chart_changes: false,
            },
        );

        let policy = KeyPolicy::new(keys, BTreeSet::new(), now, UtcOffset::UTC);
        let agent = Principal::Agent {
            key_id: "key-1".to_string(),
            tier: Tier::Draft,
            limits: ledgers_commands::Limits::new(),
        };
        let meta = CommandMeta {
            name: "test.draft".to_string(),
            tier: Tier::Draft,
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
        };
        let preview = JournalPreview::empty();

        let decision = policy.decide(&agent, &meta, &preview);
        match decision {
            Decision::Deny { reason } => {
                assert!(reason.contains("revoked"));
            }
            _ => panic!("expected Deny"),
        }
    }
}
