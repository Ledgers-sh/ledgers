//! The audit event: who acted, what they asked for, what the policy
//! engine decided — the record the hash chain covers.
//!
//! Two shapes, and the difference matters. [`AuditEvent`] is the
//! **recorded** form: every field always present, `None` serialising as
//! `null` and never absent, because a field that appears and disappears
//! is two events. [`AuditRecord`] is what a **caller** supplies: what a
//! handler knows before the chain is read, with no `seq` or
//! `company_id` (the chain's, assigned at append time) and no `id` or
//! `at` (the injected [`AuditContext`]'s).
//!
//! Time and identity are always parameters: this crate generates neither
//! a timestamp nor an id, so it holds no clock and stays wasm-clean for
//! the Workers runtime, and the same inputs produce the same event and
//! hash on any machine — which is what makes the vectors in
//! docs/audit-format.md worth pinning.

use serde::{Deserialize, Serialize};

/// Who is acting — the policy input every command is decided against
/// (docs/PLAN.md §5). An agent key never becomes a person by asking
/// differently, so the canonical form records which it was: `person`
/// with a `person_id`, or `agent` with a `key_id` and its `tier`.
/// Note what is *not* here: no field named `id`, so the event's own
/// `"id":"` is the only occurrence of that byte sequence in a canonical
/// line — which lets the verifier name a tampered event from a line it
/// could not parse (see [`crate::verify`]).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Actor {
    /// A person, by user id.
    Person {
        /// The person's id.
        person_id: String,
    },
    /// An agent key, with the tier it was granted: `read`, `draft` or
    /// `post-within-limits`.
    Agent {
        /// The key's id.
        key_id: String,
        /// The granted tier.
        tier: String,
    },
}

impl Actor {
    /// Whether a person (not an agent key) is acting — the gate for
    /// close, reopen and year-end close.
    #[must_use]
    pub fn is_person(&self) -> bool {
        matches!(self, Self::Person { .. })
    }
}

/// What the policy engine decided (docs/PLAN.md §5: allow,
/// needs-approval or deny, with a reason; `error` is this crate's
/// addition for a command that failed outside that decision).
/// `needs-approval` is a decision, not a denial: the event records that
/// an approval was requested, with its id in [`AuditEvent::approval_id`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Outcome {
    /// Policy allowed the command.
    Allowed,
    /// Policy required a person's approval.
    NeedsApproval,
    /// Policy refused the command, with a reason.
    Denied,
    /// The command failed: a guard fired, the database refused, the
    /// input would not parse. A failure is exactly what an auditor wants
    /// in the chain.
    Error,
}

/// One audited action, as recorded. Deserialize is strict
/// (`deny_unknown_fields`): a verifier that skipped fields it did not
/// understand would verify a document it had not read.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEvent {
    /// The event's own id, a ULID string from the caller.
    pub id: String,
    /// The company the action belongs to; constant across a chain.
    pub company_id: String,
    /// The chain position, 1-based and gapless.
    pub seq: i64,
    /// When it happened: RFC 3339 UTC, bound by the caller.
    pub at: String,
    /// Who acted.
    pub actor: Actor,
    /// The command's dotted name, e.g. `journal.post_entry`.
    pub command: String,
    /// Lowercase hex SHA-256 over the canonical input ([`crate::input_hash`]).
    pub input_hash: String,
    /// The policy decision.
    pub outcome: Outcome,
    /// The ids of the objects touched, so an auditor can ask
    /// "everything that ever touched this invoice" without prose.
    pub objects: Vec<String>,
    /// The policy engine's reason, when it gave one.
    pub reason: Option<String>,
    /// The approval request this action is bound to.
    pub approval_id: Option<String>,
    /// The request id — ties the event to the HTTP request, log line or
    /// MCP call that caused it.
    pub request_id: String,
}

/// What a command handler knows before the chain is read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuditRecord {
    /// The command's dotted name.
    pub command: String,
    /// The input's hash, computed by [`AuditRecord::from_input`].
    pub input_hash: String,
    /// The policy decision.
    pub outcome: Outcome,
    /// The objects the action touched.
    pub objects: Vec<String>,
    /// The policy reason, if any.
    pub reason: Option<String>,
}

impl AuditRecord {
    /// A record whose `input_hash` is computed from the command input.
    /// The input itself is not stored: it can hold personal data and it
    /// can be large.
    #[must_use]
    pub fn from_input(
        command: &str,
        input: &serde_json::Value,
        outcome: Outcome,
        objects: Vec<String>,
        reason: Option<String>,
    ) -> Self {
        Self {
            command: command.to_owned(),
            input_hash: crate::input_hash(input),
            outcome,
            objects,
            reason,
        }
    }

    /// Attaches the policy reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// Who is acting and which request, event and approval this belongs to
/// — the per-command context a handler threads through
/// [`crate::chain`]. Every value is injected: no clock, no id generator.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuditContext {
    /// Who is acting.
    pub actor: Actor,
    /// The request id, shared with the access log.
    pub request_id: String,
    /// The event id (ULID), minted by the caller.
    pub event_id: String,
    /// The approval request this action is bound to, if any.
    pub approval_id: Option<String>,
}

impl AuditContext {
    /// A context for one command: actor, request, event.
    pub fn new(actor: Actor, request_id: impl Into<String>, event_id: impl Into<String>) -> Self {
        Self {
            actor,
            request_id: request_id.into(),
            event_id: event_id.into(),
            approval_id: None,
        }
    }

    /// Binds the action to an approval request.
    #[must_use]
    pub fn with_approval(mut self, approval_id: impl Into<String>) -> Self {
        self.approval_id = Some(approval_id.into());
        self
    }

    /// The full event this context and `record` describe, at chain
    /// position `seq` for `company_id`, timestamped `at`.
    #[must_use]
    pub fn event(&self, company_id: &str, seq: i64, at: &str, record: &AuditRecord) -> AuditEvent {
        AuditEvent {
            id: self.event_id.clone(),
            company_id: company_id.to_owned(),
            seq,
            at: at.to_owned(),
            actor: self.actor.clone(),
            command: record.command.clone(),
            input_hash: record.input_hash.clone(),
            outcome: record.outcome,
            objects: record.objects.clone(),
            reason: record.reason.clone(),
            approval_id: self.approval_id.clone(),
            request_id: self.request_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context() -> AuditContext {
        AuditContext::new(
            Actor::Person {
                person_id: "usr_01HQZX9K3M7VN2P4R6T8W0Y1B3".to_owned(),
            },
            "req_01HQZX9K3M7VN2P4R6T8W0Y1B4",
            "01HQZX9K3M7VN2P4R6T8W0Y1B3D",
        )
    }

    #[test]
    fn the_actor_records_which_it_was_and_carries_no_id_field() {
        assert!(context().actor.is_person());
        let agent = Actor::Agent {
            key_id: "key_01HQZX9K3M7VN2P4R6T8W0Y1B3".to_owned(),
            tier: "read".to_owned(),
        };
        assert!(!agent.is_person());
        // No field named `id`: the verifier's fallback scan for the
        // first `"id":"` must find the event's own id, not the actor's.
        for actor in [&context().actor, &agent] {
            let value = serde_json::to_value(actor).expect("an actor serialises");
            let fields = value.as_object().expect("an object");
            assert!(!fields.contains_key("id"), "no `id` field: {value}");
            assert!(
                value
                    .get("kind")
                    .is_some_and(|k| k == "person" || k == "agent"),
                "the actor carries its kind tag: {value}"
            );
        }
        // Deserialisation is strict in both directions.
        assert!(serde_json::from_str::<Outcome>(r#""maybe""#).is_err());
        assert!(serde_json::from_str::<Actor>(r#"{"kind":"robot"}"#).is_err());
    }

    #[test]
    fn outcomes_serialise_in_the_kebab_case_policy_speaks() {
        for (outcome, text) in [
            (Outcome::Allowed, r#""allowed""#),
            (Outcome::NeedsApproval, r#""needs-approval""#),
            (Outcome::Denied, r#""denied""#),
            (Outcome::Error, r#""error""#),
        ] {
            assert_eq!(serde_json::to_string(&outcome).expect("an outcome"), text);
        }
    }

    #[test]
    fn the_context_supplies_the_id_at_and_approval_the_record_supplies_the_rest() {
        // A field that appears and disappears would be two events with
        // one meaning, so the canonical bytes always carry the key —
        // `None` is `null`, never absent.
        let record =
            AuditRecord::from_input("period.close", &json!({}), Outcome::Allowed, vec![], None);
        let plain = context().event("co", 1, "2026-01-15T09:30:00Z", &record);
        let fields = serde_json::to_value(&plain)
            .expect("an event serialises")
            .as_object()
            .expect("an object")
            .clone();
        assert_eq!(fields.len(), 12);
        assert_eq!(fields["reason"], json!(null));
        assert_eq!(fields["approval_id"], json!(null));

        let ctx = context().with_approval("apr_01HQZX9K3M7VN2P4R6T8W0Y1B9");
        let record = AuditRecord::from_input(
            "journal.post_entry",
            &json!({"amount_minor": 100}),
            Outcome::NeedsApproval,
            vec!["01HQZX9K3M7VN2P4R6T8W0Y1B3D".to_owned()],
            None,
        )
        .with_reason("above the daily limit");
        let event = ctx.event("co", 7, "2026-01-15T09:30:00Z", &record);
        assert_eq!(event.id, ctx.event_id);
        assert_eq!(event.approval_id.as_deref(), ctx.approval_id.as_deref());
        assert_eq!(event.reason.as_deref(), Some("above the daily limit"));
        assert_eq!(event.seq, 7);
        assert_eq!(record.input_hash.len(), 64);

        // Round-trips strictly; an unknown field is refused, not dropped.
        let bytes = serde_json::to_vec(&event).expect("serialises");
        assert_eq!(
            serde_json::from_slice::<AuditEvent>(&bytes).expect("deserialises"),
            event
        );
        let mut extra = serde_json::to_value(&event).expect("a value");
        extra["extra"] = json!(true);
        assert!(serde_json::from_value::<AuditEvent>(extra).is_err());
    }
}
