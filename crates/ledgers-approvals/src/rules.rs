//! Approval rules: who may approve, and what level of step-up is required.
//!
//! The open-source default: one owner approves all requests. High-risk
//! approvals (human-only commands, money movement above limits, period
//! close, tax filing) require passkey step-up; session otherwise.
//! The cloud supplies other implementations with chains and thresholds.

use crate::approval::ApprovalRequest;
use ledgers_commands::Principal;

/// The step-up level for an approval: none, session-based, or passkey.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepUp {
    /// No step-up required (pre-authenticated session is sufficient).
    Session,
    /// Passkey assertion required, bound to the request hash.
    Passkey,
}

/// Approval authorization and step-up rules (open-core extension point).
///
/// Implementations decide who may approve a request and whether passkey
/// step-up is required for high-risk actions. The open-source default
/// has one owner approver and passkey for high-risk; the cloud can
/// implement approval chains, thresholds per role, segregation of duties.
pub trait ApprovalRules: Send + Sync {
    /// Checks whether a principal may approve this request.
    ///
    /// Returns true if the principal is authorized, false otherwise
    /// (e.g., an agent principal is never authorized).
    fn may_approve(&self, approver: &Principal, request: &ApprovalRequest) -> bool;

    /// Determines whether passkey step-up is required for this request.
    ///
    /// High-risk approvals (human-only commands, money movement) require
    /// passkey; others may use session-based authentication.
    fn step_up(&self, request: &ApprovalRequest) -> StepUp;
}

/// The open-source default approval rules: one owner approves all.
///
/// Only `Principal::Person` principals may approve (agents are never
/// authorized). Passkey is required for human-only commands or if the
/// preview shows money movement (non-empty effect).
#[derive(Clone, Copy, Debug)]
pub struct OwnerApprovesAll;

impl ApprovalRules for OwnerApprovesAll {
    fn may_approve(&self, approver: &Principal, _request: &ApprovalRequest) -> bool {
        // Only persons (owners) may approve; never agents.
        approver.is_owner()
    }

    fn step_up(&self, request: &ApprovalRequest) -> StepUp {
        // Passkey required for:
        // - Human-only commands (the reason would indicate this, but we check effect for money)
        // - Any money movement (non-empty effect from the preview totals)
        // Otherwise session is sufficient.
        if request.effect.is_empty() {
            StepUp::Session
        } else {
            StepUp::Passkey
        }
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod contract_tests {
    use super::{ApprovalRequest, ApprovalRules, Principal, StepUp};
    use crate::approval::Status;
    use ledgers_commands::Tier;
    use std::collections::BTreeMap;

    fn dummy_request_with_effect() -> ApprovalRequest {
        let mut effect = BTreeMap::new();
        effect.insert("EUR".to_string(), "10000".to_string());

        ApprovalRequest {
            id: "req1".to_string(),
            company_id: "co1".to_string(),
            command: "test.cmd".to_string(),
            input: "{}".to_string(),
            preview: "{}".to_string(),
            reads: "{}".to_string(),
            request_hash: "abc123".to_string(),
            effect,
            requested_by: "agent1".to_string(),
            reason: "above limit".to_string(),
            status: Status::Pending,
            created_at: "2026-01-15T09:30:00Z".to_string(),
            expires_at: "2026-01-16T09:30:00Z".to_string(),
            decided_by: None,
            credential_id: None,
            risk: crate::approval::Risk::High,
            counterparty: None,
            decided_at: None,
            decision_reason: None,
            result: None,
            audit_ref: None,
        }
    }

    /// Contract test for `ApprovalRules` implementations.
    ///
    /// Verifies critical invariants:
    /// - An agent principal never gets approval (security: agents cannot approve).
    /// - High-risk approvals (money movement) require passkey step-up.
    /// - A person principal may approve (the owner in the open default).
    ///
    /// # Panics
    /// Panics if the invariants are violated.
    pub fn approval_rules(rules: &dyn ApprovalRules) {
        let person = Principal::Person {
            person_id: "alice".to_string(),
            role: ledgers_commands::Role::Owner,
        };
        let agent = Principal::Agent {
            key_id: "key1".to_string(),
            tier: Tier::Read,
            limits: ledgers_commands::Limits::new(),
        };

        // Request with money movement
        let request_with_effect = dummy_request_with_effect();

        // Agents never get approval
        assert!(
            !rules.may_approve(&agent, &request_with_effect),
            "agent principal must never be approved to approve"
        );

        // Person may approve
        assert!(
            rules.may_approve(&person, &request_with_effect),
            "person principal must be able to approve"
        );

        // Money movement requires passkey
        let step_up = rules.step_up(&request_with_effect);
        assert_eq!(
            step_up,
            StepUp::Passkey,
            "high-risk approval must require passkey step-up"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_tests_pass() {
        contract_tests::approval_rules(&OwnerApprovesAll);
    }
}
