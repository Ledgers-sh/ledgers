//! Approval request: what a command would do, frozen for review and approval.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The status of an approval request.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Awaiting approval.
    Pending,
    /// Approved and executed.
    Approved,
    /// Rejected by approver.
    Rejected,
    /// Expired before approval.
    Expired,
    /// Superseded by changes to the command or its reads.
    Superseded,
}

impl Status {
    /// Returns the string form of this status.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Approved => "approved",
            Status::Rejected => "rejected",
            Status::Expired => "expired",
            Status::Superseded => "superseded",
        }
    }

    /// Parses a status from a string.
    ///
    /// # Errors
    /// Returns an error if the string is not a valid status.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "pending" => Ok(Status::Pending),
            "approved" => Ok(Status::Approved),
            "rejected" => Ok(Status::Rejected),
            "expired" => Ok(Status::Expired),
            "superseded" => Ok(Status::Superseded),
            _ => Err(format!("unknown approval status: {s}")),
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How risky an approval request is. Low risk may be bulk-approved; high risk
/// needs a fresh sign-in to approve.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    /// A single-currency amount within the low-risk limit.
    Low,
    /// Anything else: no money movement, several currencies, or above the limit.
    High,
}

impl Risk {
    /// Returns the string form of this risk level.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::High => "high",
        }
    }

    /// Parses a risk level from a string.
    ///
    /// # Errors
    /// Returns an error if the string is not `low` or `high`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "low" => Ok(Risk::Low),
            "high" => Ok(Risk::High),
            _ => Err(format!("unknown approval risk: {s}")),
        }
    }
}

/// An approval request: command metadata and preview, frozen for review.
#[derive(Clone, Debug)]
pub struct ApprovalRequest {
    /// The approval request id (ULID).
    pub id: String,
    /// The company id.
    pub company_id: String,
    /// The command name (e.g., "bills.post").
    pub command: String,
    /// The command input (frozen as canonical JSON).
    pub input: String,
    /// The journal preview (frozen as canonical JSON).
    pub preview: String,
    /// Object versions the command reads at request time (frozen as canonical JSON).
    pub reads: String,
    /// The SHA-256 hash of command+input+preview+reads (for approval binding).
    pub request_hash: String,
    /// The money effect of the preview: per-currency canonical amount strings,
    /// empty for no movement.
    pub effect: BTreeMap<String, String>,
    /// The actor who requested approval (e.g., agent key id).
    pub requested_by: String,
    /// The policy reason approval is needed.
    pub reason: String,
    /// Current status (pending, approved, rejected, expired, superseded).
    pub status: Status,
    /// When the request was created (RFC 3339 UTC).
    pub created_at: String,
    /// When it expires if not approved (RFC 3339 UTC).
    pub expires_at: String,
    /// Who approved it (person id), if approved.
    pub decided_by: Option<String>,
    /// The credential (e.g., passkey credential id) used to approve, if approved.
    pub credential_id: Option<String>,
    /// The risk level the request was raised with.
    pub risk: Risk,
    /// The counterparty named in the request, if the command has one.
    pub counterparty: Option<String>,
    /// When the request was decided (RFC 3339 UTC), if it was.
    pub decided_at: Option<String>,
    /// The reason the person gave for the decision, if any.
    pub decision_reason: Option<String>,
    /// The command output after an approval (the entry reference), as JSON.
    pub result: Option<String>,
    /// The audit event that recorded the decision.
    pub audit_ref: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_as_str() {
        let pending = Status::Pending;
        assert_eq!(pending.as_str(), "pending");
    }

    #[test]
    fn status_parse() {
        assert_eq!(Status::parse("pending"), Ok(Status::Pending));
        assert_eq!(Status::parse("approved"), Ok(Status::Approved));
        assert_eq!(Status::parse("rejected"), Ok(Status::Rejected));
        assert_eq!(Status::parse("expired"), Ok(Status::Expired));
        assert_eq!(Status::parse("superseded"), Ok(Status::Superseded));
        assert_eq!(
            Status::parse("invalid"),
            Err("unknown approval status: invalid".to_string())
        );
    }
}
