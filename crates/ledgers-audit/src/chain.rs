//! Appending to the chain: the statements a command's batch puts last,
//! and the standalone write for actions that change nothing else.
//!
//! An audit event is written **inside** the batch of the action it
//! records, so the event exists exactly when the action does: a rolled
//! back posting leaves no event claiming it happened. That is why
//! [`append_statements`] returns statements for the caller to place
//! rather than writing them itself.
//!
//! Sequencing is `ledgers_db::chain`'s: the event claims `head.seq + 1`
//! and links `head.hash`, and the `(company_id, seq)` primary key makes
//! a fork a unique violation rather than a stored disagreement
//! (docs/adr/0004-guard-statements.md). [`record`] is the one-shot form
//! for the commands with no other change — a denial, an audited read —
//! and it retries a fork through [`ledgers_db::retry`], re-reading the
//! head on each attempt.

use crate::event::{AuditContext, AuditEvent, AuditRecord};
use crate::{AuditError, canonical_json, event_hash};
use cratefield_core::{Database, Outbox, Statement};
use ledgers_db::chain::{self, Head};
use ledgers_db::outbox::OUTBOX_TABLE;
use ledgers_db::retry::{self, RetryPolicy};

/// The outbox topic an anchor is enqueued under.
pub const ANCHOR_TOPIC: &str = "audit.anchor";

/// The suffix the deterministic anchor outbox id carries. Derived from
/// the event id, so the at-least-once outbox never enqueues the same
/// anchor twice: a retried batch's second statement is the same
/// primary key.
pub const ANCHOR_ID_SUFFIX: &str = "-anchor";

/// How this company records and anchors its audit trail.
///
/// An **anchor** publishes the chain head at a known sequence number —
/// every `anchor_every` events, and on demand by a periodic job — which
/// is what makes a *deleted* tail detectable: the remaining file looks
/// fine, but its hashes stop agreeing with the anchored one.
/// `anchor_every` of 0 disables automatic anchors. Not every read is an
/// audited action — a list query is not a change, and recording it
/// would bury the actions that are; sensitive **exports** are, and
/// `audited_reads` names the commands whose reads write an event.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuditConfig {
    /// Anchor the new head every this many events; 0 disables.
    pub anchor_every: i64,
    /// The read commands that are audited actions.
    pub audited_reads: Vec<String>,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            anchor_every: 100,
            audited_reads: vec!["audit.export".to_owned()],
        }
    }
}

impl AuditConfig {
    /// Whether `command` is an audited read. Exact dotted-name match:
    /// a prefix rule would let a future `audit.export_everything`
    /// inherit an audit obligation nobody declared.
    #[must_use]
    pub fn audits_read(&self, command: &str) -> bool {
        self.audited_reads.iter().any(|name| name == command)
    }
}

/// What an append produced: the event, its chain position, and the
/// statements to place — append first, anchor second — at the **end**
/// of the caller's batch.
#[derive(Clone, Debug)]
pub struct Appended {
    /// The event as hashed, with `seq` filled in.
    pub event: AuditEvent,
    /// The chain position it claims, `head.seq + 1`.
    pub seq: i64,
    /// Its hash.
    pub hash: String,
    /// Append the audit event, then at an anchor point publish the
    /// head: [`chain::append_statement`], then the outbox enqueue.
    pub statements: Vec<Statement>,
}

/// The statements that append the event at `head.seq + 1` and, when
/// that position is an anchor point, publish the new head. The `seq` is
/// filled from `head` here.
///
/// # Errors
///
/// [`AuditError::Canonical`] when the event cannot be serialised
/// exactly — propagated rather than defaulted, so an event is never
/// hashed over bytes other than the ones stored.
pub fn append_statements(
    company_id: &str,
    head: &Head,
    context: &AuditContext,
    record: &AuditRecord,
    at: &str,
    config: &AuditConfig,
) -> Result<Appended, AuditError> {
    let seq = head.seq + 1;
    let event = context.event(company_id, seq, at, record);
    let hash = event_hash(&head.hash, &event)?;
    let payload =
        canonical_json(&serde_json::to_value(&event).map_err(|err| {
            AuditError::Canonical(format!("the event cannot be serialised: {err}"))
        })?)?;

    let mut statements = vec![chain::append_statement(
        company_id, head, &hash, &payload, at,
    )];
    // The anchor rides the same batch, so it can never describe a chain
    // that rolled back.
    if config.anchor_every > 0 && seq % config.anchor_every == 0 {
        statements.push(anchor_statement(
            company_id,
            &Head {
                seq,
                hash: hash.clone(),
            },
            &format!("{}{ANCHOR_ID_SUFFIX}", event.id),
            at,
        )?);
    }
    Ok(Appended {
        event,
        seq,
        hash,
        statements,
    })
}

/// The outbox enqueue publishing the head at `(seq, hash)`, as
/// canonical JSON `{"company_id":…,"hash":…,"seq":…}` — already in
/// sorted key order, so the payload is canonical by construction.
/// `outbox_id` is the caller's to choose (an event's `{id}-anchor` for
/// the automatic case, a job run id for the periodic one): it is the
/// chain's out-window, which is what detects a deleted tail.
///
/// # Errors
///
/// [`AuditError::Canonical`] if the payload cannot be serialised; it is
/// three scalars, so this cannot fail in practice.
pub fn anchor_statement(
    company_id: &str,
    head: &Head,
    outbox_id: &str,
    at: &str,
) -> Result<Statement, AuditError> {
    let payload = canonical_json(&serde_json::json!({
        "company_id": company_id,
        "hash": head.hash,
        "seq": head.seq,
    }))?;
    Ok(Outbox::new(OUTBOX_TABLE).enqueue_statement(outbox_id, ANCHOR_TOPIC, &payload, None, at))
}

/// The one-shot write: a batch of nothing but this event, for the
/// commands with no other change to record — a denial, or an audited
/// read. The head is read **inside** the plan closure, so a fork
/// (another writer took the same `seq`) is retried against a fresh head
/// rather than resolved by a read-then-write the caller had to get
/// right. `sleep` is injected and defaults to no waiting: a Workers
/// runtime must not block, and a native caller can pass
/// `std::thread::sleep`.
///
/// # Errors
///
/// [`AuditError::Guard`] for contention (retry budget exhausted) or a
/// database refusal.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    db: &dyn Database,
    config: &AuditConfig,
    company_id: &str,
    context: &AuditContext,
    record: &AuditRecord,
    at: &str,
    policy: RetryPolicy,
    mut sleep: impl FnMut(std::time::Duration),
) -> Result<Appended, AuditError> {
    // The last attempt's plan is what committed, shared via `Rc` with
    // the plan closure so the caller learns the seq and hash actually
    // written rather than ones an earlier attempt planned.
    let written = std::rc::Rc::new(std::cell::RefCell::new(None::<Appended>));

    retry::run_batch_with_retry(
        db,
        policy,
        || {
            let (context, record) = (context.clone(), record.clone());
            let written = std::rc::Rc::clone(&written);
            async move {
                let head = chain::read_head(db, company_id).await?;
                // A serialisation failure cannot become a `DbError`, but
                // the closure returns the port's error type, so it is
                // mapped onto "the statement failed" and stays final —
                // re-serialising the same event fails the same way.
                let planned = append_statements(company_id, &head, &context, &record, at, config)
                    .map_err(|err| cratefield_core::DbError::Execute(err.to_string()))?;
                *written.borrow_mut() = Some(planned.clone());
                Ok(planned.statements)
            }
        },
        &mut sleep,
    )
    .await
    .map_err(AuditError::from)?;

    // The batch succeeded, so the closure set `written` on the
    // attempt that committed.
    written.borrow().clone().ok_or(AuditError::Canonical(
        "the retry loop committed no batch".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Actor, Outcome};
    use ledgers_db::chain::GENESIS_HASH;
    use serde_json::json;

    const COMPANY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    /// Appends the standard company.open event onto `head`, anchoring
    /// every `anchor_every` events (`0` disables).
    fn append(head: &Head, anchor_every: i64) -> Appended {
        append_statements(
            COMPANY_ID,
            head,
            &AuditContext::new(
                Actor::Person {
                    person_id: concat!("usr_01HQZX9K3", "M7VN2P4R6T8W0", "Y1B3").to_owned(),
                },
                concat!("req_01HQZX9K3", "M7VN2P4R6T8W0Y", "1B3B4"),
                "01HQZX9K3M7VN2P4R6T8W0Y1B3D",
            ),
            &AuditRecord::from_input(
                "company.open",
                &json!({"company": "Halden Works GmbH"}),
                Outcome::Allowed,
                vec![COMPANY_ID.to_owned()],
                None,
            ),
            "2026-01-15T09:30:00Z",
            &AuditConfig {
                anchor_every,
                ..AuditConfig::default()
            },
        )
        .expect("appended")
    }

    /// The bound payload of an outbox statement: `sea_query::Value`
    /// carries text as `String(Option<Box<String>>)`.
    fn payload_of(statement: &Statement) -> String {
        match &statement.values.0[2] {
            sea_query::Value::String(Some(text)) => text.as_ref().clone(),
            other => panic!("expected a text payload, found {other:?}"),
        }
    }

    #[test]
    fn an_append_claims_the_next_seq_from_the_head_it_is_given() {
        let appended = append(
            &Head {
                seq: 0,
                hash: GENESIS_HASH.to_owned(),
            },
            AuditConfig::default().anchor_every,
        );
        assert_eq!((appended.seq, appended.event.seq), (1, 1));
        assert_eq!(
            appended.hash,
            event_hash(GENESIS_HASH, &appended.event).expect("hashed")
        );
        // One statement: the default interval of 100 is not reached at
        // seq 1, so there is no anchor.
        assert_eq!(appended.statements.len(), 1);
        assert!(
            appended.statements[0]
                .sql
                .contains("INSERT INTO audit_event")
        );
    }

    #[test]
    fn an_anchor_point_adds_the_outbox_enqueue_after_the_append() {
        for (every, at_seq, statements) in
            [(1_i64, 1_i64, 2_usize), (2, 1, 1), (2, 2, 2), (0, 100, 1)]
        {
            let appended = append(
                &Head {
                    seq: at_seq - 1,
                    hash: GENESIS_HASH.to_owned(),
                },
                every,
            );
            assert_eq!(
                appended.statements.len(),
                statements,
                "every {every}, seq {at_seq}"
            );
            if statements == 2 {
                // The append comes first: the anchor names a head that
                // exists only once the event above has committed.
                assert!(
                    appended.statements[0]
                        .sql
                        .contains("INSERT INTO audit_event")
                );
                assert!(appended.statements[1].sql.contains(OUTBOX_TABLE));
                let payload = payload_of(&appended.statements[1]);
                assert_eq!(
                    payload,
                    format!(
                        r#"{{"company_id":"{COMPANY_ID}","hash":"{}","seq":{at_seq}}}"#,
                        appended.hash
                    ),
                    "the anchor names the head it was appended with"
                );
                // Already canonical: re-serialising changes nothing.
                let value = serde_json::from_str::<serde_json::Value>(&payload).expect("json");
                assert_eq!(canonical_json(&value).expect("canonical"), payload);
                // Deterministic id, so a retry cannot enqueue twice.
                assert_eq!(
                    format!("{}{ANCHOR_ID_SUFFIX}", appended.event.id),
                    concat!("01HQZX9K3M7VN", "2P4R6T8W0Y1B3D", "-anchor")
                );
            }
        }
    }

    #[test]
    fn only_the_configured_reads_are_audited() {
        let config = AuditConfig::default();
        assert_eq!(config.anchor_every, 100);
        assert!(config.audits_read("audit.export"));
        assert!(!config.audits_read("audit.export_everything"));
        assert!(!config.audits_read("ledger.trial_balance"));
        assert!(
            !AuditConfig {
                audited_reads: vec![],
                ..AuditConfig::default()
            }
            .audits_read("audit.export")
        );
    }
}
