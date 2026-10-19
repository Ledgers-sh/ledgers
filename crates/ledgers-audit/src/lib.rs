#![forbid(unsafe_code)]
//! The hash-chained audit log and its offline verifier (docs/PLAN.md §3
//! rule 6: every action is audited in an append-only, hash-chained log
//! that the export includes and a verifier can check offline).
//!
//! Every command is one [`AuditEvent`]: who acted ([`Actor`]), what they
//! asked for (`command`, plus a hash of the input so the input itself
//! need not be stored), what the policy engine decided ([`Outcome`],
//! with a reason), and which objects it touched. The event is written
//! **inside the batch of the action it records**, so it exists exactly
//! when the action does.
//!
//! Events are sequenced by [`ledgers_db::chain`]: `seq` is `head.seq +
//! 1`, `prev_hash` is the head's hash, and the `(company_id, seq)`
//! primary key makes a fork a unique violation rather than a stored
//! disagreement (docs/adr/0004-guard-statements.md). Each hash is
//! SHA-256 over the previous hash's bytes immediately followed by the
//! event's canonical JSON.
//!
//! [`canonical_json`] is this crate's own serialiser, not
//! `serde_json`'s: object keys sort by byte order, escaping is fixed
//! and only integers are numbers, so "unaltered" is a decidable
//! question about bytes rather than a matter of interpretation. See
//! [`canonical`] and docs/audit-format.md.
//!
//! [`export`] writes the chain as JSON Lines and [`verify`] checks such
//! a file offline, reporting the **first** break with the line, the
//! `seq` and the event id it could recover. `ledgers-audit verify
//! <file>` is the command-line form.
//!
//! Nothing here reads a clock: ids and timestamps are injected
//! ([`AuditContext`]), so the library is wasm-clean for the Workers
//! runtime and the same inputs produce the same event and hash on any
//! machine — which is what makes the vectors in docs/audit-format.md
//! worth pinning.

pub mod canonical;
pub mod chain;
pub mod event;
pub mod verify;

pub use canonical::{canonical_json, event_hash, input_hash, sha256_hex};
pub use chain::{
    ANCHOR_ID_SUFFIX, ANCHOR_TOPIC, Appended, AuditConfig, anchor_statement, append_statements,
    record,
};
pub use event::{Actor, AuditContext, AuditEvent, AuditRecord, Outcome};
pub use verify::{Break, BreakKind, Verified, export, verify, verify_db};

/// Everything that can go wrong in this crate. One enum, because a
/// caller acts on the audit log the same way in every case: do not
/// record the event, and say why.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    /// A value could not be canonicalised — a non-integer number, or a
    /// tree the walk cannot represent. Every case is a caller bug, and
    /// there is no lossy fallback: an event that cannot be hashed over
    /// its own bytes must not be recorded.
    #[error("audit value cannot be canonicalised: {0}")]
    Canonical(String),
    /// The query or batch failed (see [`cratefield_core::DbError`]).
    #[error(transparent)]
    Database(#[from] cratefield_core::DbError),
    /// An `audit_event` row is not the shape the table declares, or its
    /// payload is not a parseable event. Never expected from a chain
    /// this crate wrote.
    #[error("audit_event row{} is malformed: {detail}", .seq.map(|s| format!(" at seq {s}")).unwrap_or_default())]
    Malformed {
        /// The row's `seq`, when it was readable.
        seq: Option<i64>,
        /// What was wrong with it.
        detail: String,
    },
    /// Contention, or a database refusal (see
    /// [`ledgers_db::GuardError`]).
    #[error(transparent)]
    Guard(#[from] ledgers_db::GuardError),
    /// The exported chain does not verify: the same finding a file
    /// would produce, from the same bytes.
    #[error(transparent)]
    Break(#[from] Break),
}
