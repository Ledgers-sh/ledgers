//! Policy decision-making: decide whether to allow, deny, or need approval.
//!
//! The policy engine checks if a principal may run a command, using the
//! command's tier requirement, human-only flag, and the agent's limits
//! against the preview.

use crate::auth::Principal;
use crate::journal_preview::JournalPreview;
use crate::registry::CommandMeta;
use cratefield_core::Statement;
use ledgers_money::{Amount, Currency};
use serde::{Deserialize, Serialize};

/// The outcome of a policy decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// The principal is allowed to run the command.
    Allow,
    /// The principal needs approval (e.g., amount exceeds a limit).
    NeedsApproval {
        /// The reason approval is needed.
        reason: String,
    },
    /// The principal is denied.
    Deny {
        /// The reason for the denial.
        reason: String,
    },
}

/// Trait for policy decision-making.
pub trait Policy: Send + Sync {
    /// Decides whether a principal may run a command, given its metadata
    /// and preview.
    fn decide(
        &self,
        principal: &Principal,
        meta: &CommandMeta,
        preview: &JournalPreview,
    ) -> Decision;

    /// Guard statements for the posting batch (optional, default empty).
    ///
    /// For allowed posts, the executor runs these statements in the same
    /// `batch_atomic` as the command's statements, inside the retry loop.
    /// A guard refusal (`NOT NULL` failure) fails the whole batch and
    /// surfaces as an error, not success.
    ///
    /// # Errors
    /// Returns an error if guard statements cannot be generated (e.g., from invalid input).
    fn guards(
        &self,
        principal: &Principal,
        meta: &CommandMeta,
        preview: &JournalPreview,
    ) -> Result<Vec<Statement>, crate::error::CommandError> {
        let _ = (principal, meta, preview);
        Ok(Vec::new())
    }
}

/// The default open-core policy: the owner is always allowed;
/// agents are checked against their tier and limits.
pub struct DefaultPolicy;

impl Policy for DefaultPolicy {
    #[allow(clippy::manual_let_else)]
    fn decide(
        &self,
        principal: &Principal,
        meta: &CommandMeta,
        preview: &JournalPreview,
    ) -> Decision {
        match principal {
            Principal::Person { .. } => {
                // The owner is always allowed.
                Decision::Allow
            }
            Principal::Agent {
                tier,
                limits,
                key_id,
            } => {
                // Human-only commands are denied to agents.
                if meta.human_only {
                    return Decision::Deny {
                        reason: "command is human-only".to_string(),
                    };
                }

                // Check tier requirement.
                if *tier < meta.tier {
                    return Decision::Deny {
                        reason: format!("tier {} insufficient for {}", tier, meta.tier),
                    };
                }

                // Empty preview means no money movement, so no limit check needed.
                if preview.is_empty() {
                    return Decision::Allow;
                }

                // Check per-currency limits against preview totals.
                for (currency_code, amount_str) in &preview.totals {
                    // Parse the currency code to get the Currency for parsing the amount.
                    let currency = match Currency::from_code(currency_code) {
                        Ok(c) => c,
                        Err(_) => {
                            // Unknown currency code (should not happen).
                            return Decision::Deny {
                                reason: format!("unknown currency: {currency_code}"),
                            };
                        }
                    };

                    let amount = match Amount::parse_canonical(amount_str, currency) {
                        Ok(a) => a,
                        Err(_) => {
                            // Malformed preview total (should not happen).
                            return Decision::Deny {
                                reason: "malformed preview total".to_string(),
                            };
                        }
                    };
                    let limit = limits.get(currency_code);

                    // No limit entry means ceiling is 0 (disallowed).
                    let (base_ceiling, txn_ceiling) = match limit {
                        Some(l) => (l.base_ceiling, l.transaction_ceiling),
                        None => (0, 0),
                    };

                    // Check against both base and transaction ceilings.
                    // A ceiling of 0 means no transactions allowed (need approval).
                    let amount_minor = amount.minor();
                    if amount_minor > base_ceiling {
                        return Decision::NeedsApproval {
                            reason: format!(
                                "{amount_str} exceeds base limit of {base_ceiling} for agent {key_id}"
                            ),
                        };
                    }
                    if amount_minor > txn_ceiling {
                        return Decision::NeedsApproval {
                            reason: format!(
                                "{amount_str} exceeds transaction limit of {txn_ceiling} for agent {key_id}"
                            ),
                        };
                    }
                }

                Decision::Allow
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Limits, Tier};

    #[test]
    fn owner_always_allowed() {
        let policy = DefaultPolicy;
        let owner = Principal::Person {
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

        let decision = policy.decide(&owner, &meta, &preview);
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    fn agent_human_only_denied() {
        let policy = DefaultPolicy;
        let agent = Principal::Agent {
            key_id: "key-1".to_string(),
            tier: Tier::PostWithinLimits,
            limits: Limits::new(),
        };
        let meta = CommandMeta {
            name: "test.command".to_string(),
            tier: Tier::Draft,
            human_only: true,
            supports_dry_run: false,
            idempotent: false,
        };
        let preview = JournalPreview::empty();

        let decision = policy.decide(&agent, &meta, &preview);
        match decision {
            Decision::Deny { reason } => {
                assert!(reason.contains("human-only"));
            }
            _ => panic!("expected Deny"),
        }
    }
}
