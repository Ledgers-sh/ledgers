//! Decision logic: `CommandClass`, `DecisionInput`, `decide`, and `explain`.
//!
//! A pure decision function that determines whether an actor may run a command.
//! The [`explain`] function records each check, and [`decide`] calls [`explain`]
//! to avoid divergence.

use crate::key::AgentKey;
use ledgers_commands::{CommandMeta, Tier};
use ledgers_money::{Amount, Currency};
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;

/// What a command does, categorized by authorization tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandClass {
    /// A read-only command.
    Read,
    /// A draft creation or extraction command.
    Draft,
    /// A posting command.
    Post,
    /// A human-only command (e.g., payouts, approvals, period changes).
    HumanOnly,
    /// A policy or key change (always denied to agents).
    PolicyChange,
}

impl CommandClass {
    /// Determines the class of a command from its metadata.
    ///
    /// Precedence:
    /// - If tier < Read → `PolicyChange` (defense in depth)
    /// - If `human_only` → `HumanOnly`
    /// - Otherwise: match tier (Read, Draft, PostWithinLimits→Post)
    #[must_use]
    pub fn of(meta: &CommandMeta) -> CommandClass {
        if meta.name.starts_with("policy.") && meta.tier != Tier::Read {
            return CommandClass::PolicyChange;
        }
        if meta.human_only {
            return CommandClass::HumanOnly;
        }
        match meta.tier {
            Tier::Read => CommandClass::Read,
            Tier::Draft => CommandClass::Draft,
            Tier::PostWithinLimits => CommandClass::Post,
        }
    }
}

/// Who's running the command.
#[derive(Clone, Copy, Debug)]
pub enum Actor<'a> {
    /// A person (the owner).
    Person,
    /// An agent with a key.
    Agent(&'a AgentKey),
}

/// The origin of an entry being reversed.
#[derive(Clone, Copy, Debug)]
pub enum Origin {
    /// Posted or approved by a person.
    Person,
    /// Posted by an agent.
    Agent,
}

/// Input to the decision function: the command and what it would do.
#[derive(Clone, Debug)]
pub struct DecisionInput {
    /// Current timestamp (for expiry checks, rolling-day accounting).
    pub now: OffsetDateTime,
    /// Per-currency effect in minor units (non-negative).
    pub effect: BTreeMap<String, i128>,
    /// Per-currency already-used today (for rolling-day checks).
    pub used_today: BTreeMap<String, i128>,
    /// Journal id, or None if journal-agnostic.
    pub journal: Option<String>,
    /// Accounts touched by the command.
    pub accounts: BTreeSet<String>,
    /// Protected accounts (bank, equity, AR/AP control, etc).
    pub protected_accounts: BTreeSet<String>,
    /// Origin of the entry being reversed (if any).
    pub reverses: Option<Origin>,
}

/// The outcome of a decision, matching [`ledgers_commands::Decision`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The actor is allowed.
    Allow,
    /// The actor needs approval (and a reason why).
    NeedsApproval { reason: String },
    /// The actor is denied (and a reason why).
    Deny { reason: String },
}

/// An explanation of a decision with detailed steps.
#[derive(Clone, Debug)]
pub struct Explanation {
    /// The final decision.
    pub decision: Decision,
    /// The command class.
    pub class: CommandClass,
    /// Each check performed in order.
    pub steps: Vec<String>,
}

/// Explains a decision, recording each check performed.
///
/// This is the authoritative decision function. Returns an [`Explanation`]
/// containing the decision and all steps taken. [`decide`] calls this.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn explain(actor: Actor, meta: &CommandMeta, input: &DecisionInput) -> Explanation {
    let mut steps = Vec::new();
    let class = CommandClass::of(meta);

    // For persons, always allow.
    if matches!(actor, Actor::Person) {
        steps.push("actor is person".to_string());
        return Explanation {
            decision: Decision::Allow,
            class,
            steps,
        };
    }

    // From here on, we're dealing with an agent.
    let Actor::Agent(key) = actor else {
        unreachable!("Person case already handled")
    };
    steps.push(format!("actor is agent: {}", key.id));

    // 1. Check revoked.
    if key.revoked_at.is_some() {
        let reason = format!("agent key {} is revoked", key.id);
        steps.push(reason.clone());
        return Explanation {
            decision: Decision::Deny { reason },
            class,
            steps,
        };
    }

    // 2. Check expired.
    #[allow(clippy::collapsible_if)]
    if let Some(expires) = key.expires_at {
        if input.now >= expires {
            let reason = format!("agent key {} is expired", key.id);
            steps.push(reason.clone());
            return Explanation {
                decision: Decision::Deny { reason },
                class,
                steps,
            };
        }
    }

    // 3. Check PolicyChange.
    if class == CommandClass::PolicyChange {
        let reason = "agents cannot change policy".to_string();
        steps.push(reason.clone());
        return Explanation {
            decision: Decision::Deny { reason },
            class,
            steps,
        };
    }

    // 4. Check tier >= command tier.
    if key.tier < meta.tier {
        let reason = format!(
            "agent tier {} insufficient for tier {}",
            key.tier, meta.tier
        );
        steps.push(reason.clone());
        return Explanation {
            decision: Decision::Deny { reason },
            class,
            steps,
        };
    }

    // 5. Check HumanOnly.
    if class == CommandClass::HumanOnly {
        let reason = "command is human-only".to_string();
        steps.push(reason.clone());
        return Explanation {
            decision: Decision::NeedsApproval { reason },
            class,
            steps,
        };
    }

    // 6. Check allowed_journals.
    if let Some(allowed) = &key.allowed_journals {
        if let Some(journal) = &input.journal {
            if !allowed.contains(journal) {
                let reason = format!("journal {journal} not allowed for agent {}", key.id);
                steps.push(reason.clone());
                return Explanation {
                    decision: Decision::Deny { reason },
                    class,
                    steps,
                };
            }
        } else if !allowed.is_empty() {
            // Journal is required but None.
            let reason = "journal required but not provided".to_string();
            steps.push(reason.clone());
            return Explanation {
                decision: Decision::Deny { reason },
                class,
                steps,
            };
        }
    }

    // 7. Check allowed_accounts.
    if let Some(allowed) = &key.allowed_accounts {
        for account in &input.accounts {
            if !allowed.contains(account) {
                let reason = format!("account {account} not allowed for agent {}", key.id);
                steps.push(reason.clone());
                return Explanation {
                    decision: Decision::Deny { reason },
                    class,
                    steps,
                };
            }
        }
    }

    // 8. Check reversal of person entry.
    if let Some(Origin::Person) = input.reverses {
        let reason = "reversing an entry posted by a person requires approval".to_string();
        steps.push(reason.clone());
        return Explanation {
            decision: Decision::NeedsApproval { reason },
            class,
            steps,
        };
    }

    // 9. Check protected accounts.
    if !input.protected_accounts.is_empty() {
        let touching_protected: Vec<&String> = input
            .accounts
            .iter()
            .filter(|a| input.protected_accounts.contains(*a))
            .collect();
        if !touching_protected.is_empty() {
            let reason = "posting touches protected account(s) and requires approval".to_string();
            steps.push(reason.clone());
            return Explanation {
                decision: Decision::NeedsApproval { reason },
                class,
                steps,
            };
        }
    }

    // 10. Check per-currency limits (only for Post class).
    if class == CommandClass::Post {
        for (code, amount) in &input.effect {
            if *amount <= 0 {
                continue;
            }

            // Check if currency has a limit.
            let Some(limit) = key.limits.get(code) else {
                let reason = format!("no limit for currency {code} for agent {}", key.id);
                steps.push(reason.clone());
                return Explanation {
                    decision: Decision::NeedsApproval { reason },
                    class,
                    steps,
                };
            };

            // Check per-entry limit.
            if *amount > limit.per_entry {
                let currency = Currency::from_code(code).ok();
                let amount_str = currency.map_or_else(
                    || amount.to_string(),
                    |c| Amount::new(*amount, c).to_canonical_string(),
                );
                let limit_str = currency.map_or_else(
                    || limit.per_entry.to_string(),
                    |c| Amount::new(limit.per_entry, c).to_canonical_string(),
                );
                let reason = format!("{amount_str} exceeds per-entry limit {limit_str} for {code}");
                steps.push(reason.clone());
                return Explanation {
                    decision: Decision::NeedsApproval { reason },
                    class,
                    steps,
                };
            }

            // Check rolling-day limit.
            let used_today = input.used_today.get(code).copied().unwrap_or(0);
            let Some(total) = used_today.checked_add(*amount) else {
                let reason = format!("rolling-day usage would overflow for {code}");
                steps.push(reason.clone());
                return Explanation {
                    decision: Decision::NeedsApproval { reason },
                    class,
                    steps,
                };
            };
            if total > limit.per_day {
                let currency = Currency::from_code(code).ok();
                let total_str = currency.map_or_else(
                    || total.to_string(),
                    |c| Amount::new(total, c).to_canonical_string(),
                );
                let limit_str = currency.map_or_else(
                    || limit.per_day.to_string(),
                    |c| Amount::new(limit.per_day, c).to_canonical_string(),
                );
                let reason =
                    format!("{total_str} total today exceeds per-day limit {limit_str} for {code}");
                steps.push(reason.clone());
                return Explanation {
                    decision: Decision::NeedsApproval { reason },
                    class,
                    steps,
                };
            }
        }
    }

    steps.push("all checks passed".to_string());
    Explanation {
        decision: Decision::Allow,
        class,
        steps,
    }
}

/// Decides whether an actor may run a command.
///
/// This is a pure function that returns [`Decision::Allow`], [`Decision::NeedsApproval`],
/// or [`Decision::Deny`]. It calls [`explain`] internally to ensure decisions and
/// explanations cannot diverge.
#[must_use]
pub fn decide(actor: Actor, meta: &CommandMeta, input: &DecisionInput) -> Decision {
    explain(actor, meta, input).decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CurrencyLimit;

    /// Table-driven test: (tier, `meta_tier`, `human_only`, `effect`, `used_today`, `reverses`, protected,
    /// `allowed_journals`, `allowed_accounts`, `revoked`, `expires`, `expected_decision_kind`)
    struct TestCase {
        name: &'static str,
        tier: Tier,
        meta_tier: Tier,
        human_only: bool,
        effect: &'static [(&'static str, i128)],
        used_today: &'static [(&'static str, i128)],
        reverses: Option<Origin>,
        protected_accounts: &'static [&'static str],
        revoked: bool,
        expired: bool,
        expected: &'static str, // "Allow", "NeedsApproval", or "Deny"
    }

    fn make_test_key(tc: &TestCase, now: OffsetDateTime) -> AgentKey {
        let mut limits = BTreeMap::new();
        // Default limit: per_entry 100_000, per_day 1_000_000
        limits.insert(
            "EUR".to_string(),
            CurrencyLimit {
                per_entry: 100_000,
                per_day: 1_000_000,
            },
        );

        AgentKey {
            id: "key-1".to_string(),
            label: "Test".to_string(),
            owner: "alice".to_string(),
            tier: tc.tier,
            limits,
            allowed_journals: None,
            allowed_accounts: None,
            expires_at: if tc.expired {
                Some(now - time::Duration::days(1))
            } else {
                None
            },
            created_at: now,
            last_used_at: None,
            revoked_at: if tc.revoked { Some(now) } else { None },
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn decision_table_driven() {
        let now = time::OffsetDateTime::now_utc();
        let cases = vec![
            // CommandClass tests
            TestCase {
                name: "Read tier",
                tier: Tier::Read,
                meta_tier: Tier::Read,
                human_only: false,
                effect: &[],
                used_today: &[],
                reverses: None,
                protected_accounts: &[],
                revoked: false,
                expired: false,
                expected: "Allow",
            },
            TestCase {
                name: "Draft tier",
                tier: Tier::Draft,
                meta_tier: Tier::Draft,
                human_only: false,
                effect: &[],
                used_today: &[],
                reverses: None,
                protected_accounts: &[],
                revoked: false,
                expired: false,
                expected: "Allow",
            },
            TestCase {
                name: "Post tier",
                tier: Tier::PostWithinLimits,
                meta_tier: Tier::PostWithinLimits,
                human_only: false,
                effect: &[],
                used_today: &[],
                reverses: None,
                protected_accounts: &[],
                revoked: false,
                expired: false,
                expected: "Allow",
            },
            // Lifecycle checks
            TestCase {
                name: "Revoked denied",
                tier: Tier::Draft,
                meta_tier: Tier::Draft,
                human_only: false,
                effect: &[],
                used_today: &[],
                reverses: None,
                protected_accounts: &[],
                revoked: true,
                expired: false,
                expected: "Deny",
            },
            TestCase {
                name: "Expired denied",
                tier: Tier::Draft,
                meta_tier: Tier::Draft,
                human_only: false,
                effect: &[],
                used_today: &[],
                reverses: None,
                protected_accounts: &[],
                revoked: false,
                expired: true,
                expected: "Deny",
            },
            // Tier checks
            TestCase {
                name: "Tier insufficient denied",
                tier: Tier::Read,
                meta_tier: Tier::Draft,
                human_only: false,
                effect: &[],
                used_today: &[],
                reverses: None,
                protected_accounts: &[],
                revoked: false,
                expired: false,
                expected: "Deny",
            },
            // HumanOnly & PolicyChange
            TestCase {
                name: "Human-only needs approval",
                tier: Tier::PostWithinLimits,
                meta_tier: Tier::PostWithinLimits,
                human_only: true,
                effect: &[],
                used_today: &[],
                reverses: None,
                protected_accounts: &[],
                revoked: false,
                expired: false,
                expected: "NeedsApproval",
            },
            // Reversals
            TestCase {
                name: "Reversal of person needs approval",
                tier: Tier::PostWithinLimits,
                meta_tier: Tier::PostWithinLimits,
                human_only: false,
                effect: &[],
                used_today: &[],
                reverses: Some(Origin::Person),
                protected_accounts: &[],
                revoked: false,
                expired: false,
                expected: "NeedsApproval",
            },
            // Per-entry limit exceeded
            TestCase {
                name: "Per-entry limit exceeded",
                tier: Tier::PostWithinLimits,
                meta_tier: Tier::PostWithinLimits,
                human_only: false,
                effect: &[("EUR", 150_000)],
                used_today: &[],
                reverses: None,
                protected_accounts: &[],
                revoked: false,
                expired: false,
                expected: "NeedsApproval",
            },
            // Per-day limit exceeded
            TestCase {
                name: "Per-day limit exceeded",
                tier: Tier::PostWithinLimits,
                meta_tier: Tier::PostWithinLimits,
                human_only: false,
                effect: &[("EUR", 600_000)],
                used_today: &[("EUR", 500_000)],
                reverses: None,
                protected_accounts: &[],
                revoked: false,
                expired: false,
                expected: "NeedsApproval",
            },
            // Protected account
            TestCase {
                name: "Protected account needs approval",
                tier: Tier::PostWithinLimits,
                meta_tier: Tier::PostWithinLimits,
                human_only: false,
                effect: &[],
                used_today: &[],
                reverses: None,
                protected_accounts: &["1100"],
                revoked: false,
                expired: false,
                expected: "NeedsApproval",
            },
        ];

        for tc in &cases {
            let key = make_test_key(tc, now);
            let mut meta_name = "test.".to_string();
            match tc.meta_tier {
                Tier::Read => meta_name.push_str("read"),
                Tier::Draft => meta_name.push_str("draft"),
                Tier::PostWithinLimits => meta_name.push_str("post"),
            }

            let meta = CommandMeta {
                name: meta_name,
                tier: tc.meta_tier,
                human_only: tc.human_only,
                supports_dry_run: false,
                idempotent: false,
            };

            let mut effect = BTreeMap::new();
            for (code, amount) in tc.effect {
                effect.insert(code.to_string(), *amount);
            }

            let mut used_today = BTreeMap::new();
            for (code, amount) in tc.used_today {
                used_today.insert(code.to_string(), *amount);
            }

            let mut accounts = BTreeSet::new();
            let mut protected_accounts = BTreeSet::new();
            for acc in tc.protected_accounts {
                accounts.insert(acc.to_string());
                protected_accounts.insert(acc.to_string());
            }

            let input = DecisionInput {
                now,
                effect,
                used_today,
                journal: None,
                accounts,
                protected_accounts,
                reverses: tc.reverses,
            };

            let decision = decide(Actor::Agent(&key), &meta, &input);
            let explanation = explain(Actor::Agent(&key), &meta, &input);

            // Verify decide() and explain() agree
            assert_eq!(
                decision, explanation.decision,
                "Test '{}': decide() and explain() disagree",
                tc.name
            );

            // Check decision matches expected
            let decision_kind = match decision {
                Decision::Allow => "Allow",
                Decision::NeedsApproval { .. } => "NeedsApproval",
                Decision::Deny { .. } => "Deny",
            };
            assert_eq!(
                decision_kind, tc.expected,
                "Test '{}': expected {}, got {}",
                tc.name, tc.expected, decision_kind
            );
        }
    }

    #[test]
    fn command_class_policy_change() {
        let meta = CommandMeta {
            name: "policy.update".to_string(),
            tier: Tier::PostWithinLimits,
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
        };
        assert_eq!(CommandClass::of(&meta), CommandClass::PolicyChange);
    }

    #[test]
    fn person_always_allowed() {
        let meta = CommandMeta {
            name: "test.policy".to_string(),
            tier: Tier::PostWithinLimits,
            human_only: true,
            supports_dry_run: false,
            idempotent: false,
        };
        let input = DecisionInput {
            now: time::OffsetDateTime::now_utc(),
            effect: BTreeMap::new(),
            used_today: BTreeMap::new(),
            journal: None,
            accounts: BTreeSet::new(),
            protected_accounts: BTreeSet::new(),
            reverses: None,
        };

        let decision = decide(Actor::Person, &meta, &input);
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn limit_edge_cases() {
        let now = time::OffsetDateTime::now_utc();
        let mut limits = BTreeMap::new();
        limits.insert(
            "EUR".to_string(),
            CurrencyLimit {
                per_entry: 100_000, // 1000.00 EUR
                per_day: 100_000,   // 1000.00 EUR
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

        let meta = CommandMeta {
            name: "test.post".to_string(),
            tier: Tier::PostWithinLimits,
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
        };

        // Exactly at per_entry limit: allowed
        {
            let mut effect = BTreeMap::new();
            effect.insert("EUR".to_string(), 100_000);
            let input = DecisionInput {
                now,
                effect,
                used_today: BTreeMap::new(),
                journal: None,
                accounts: BTreeSet::new(),
                protected_accounts: BTreeSet::new(),
                reverses: None,
            };
            assert_eq!(
                decide(Actor::Agent(&key), &meta, &input),
                Decision::Allow,
                "per_entry at exact limit should allow"
            );
        }

        // One minor unit over per_entry limit: needs approval
        {
            let mut effect = BTreeMap::new();
            effect.insert("EUR".to_string(), 100_001);
            let input = DecisionInput {
                now,
                effect,
                used_today: BTreeMap::new(),
                journal: None,
                accounts: BTreeSet::new(),
                protected_accounts: BTreeSet::new(),
                reverses: None,
            };
            match decide(Actor::Agent(&key), &meta, &input) {
                Decision::NeedsApproval { .. } => {}
                _ => panic!("per_entry over limit should need approval"),
            }
        }

        // Exactly at per_day limit: allowed (e.g., 40000+60000=100000)
        {
            let mut effect = BTreeMap::new();
            effect.insert("EUR".to_string(), 60_000);
            let mut used_today = BTreeMap::new();
            used_today.insert("EUR".to_string(), 40_000);
            let input = DecisionInput {
                now,
                effect,
                used_today,
                journal: None,
                accounts: BTreeSet::new(),
                protected_accounts: BTreeSet::new(),
                reverses: None,
            };
            assert_eq!(
                decide(Actor::Agent(&key), &meta, &input),
                Decision::Allow,
                "per_day at exact limit should allow"
            );
        }

        // One minor unit over per_day limit: needs approval
        {
            let mut effect = BTreeMap::new();
            effect.insert("EUR".to_string(), 60_001);
            let mut used_today = BTreeMap::new();
            used_today.insert("EUR".to_string(), 40_000);
            let input = DecisionInput {
                now,
                effect,
                used_today,
                journal: None,
                accounts: BTreeSet::new(),
                protected_accounts: BTreeSet::new(),
                reverses: None,
            };
            match decide(Actor::Agent(&key), &meta, &input) {
                Decision::NeedsApproval { .. } => {}
                _ => panic!("per_day over limit should need approval"),
            }
        }

        // Currency without limit: needs approval
        {
            let mut effect = BTreeMap::new();
            effect.insert("USD".to_string(), 1_000);
            let input = DecisionInput {
                now,
                effect,
                used_today: BTreeMap::new(),
                journal: None,
                accounts: BTreeSet::new(),
                protected_accounts: BTreeSet::new(),
                reverses: None,
            };
            match decide(Actor::Agent(&key), &meta, &input) {
                Decision::NeedsApproval { reason } => {
                    assert!(reason.contains("no limit"));
                }
                _ => panic!("currency without limit should need approval"),
            }
        }
    }
}
