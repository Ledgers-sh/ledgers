//! The audit chain's sequencing spine: append-only, fork-free by
//! primary key.
//!
//! Hashing and verification belong to `ledgers-audit` later; here a
//! hash is opaque `TEXT`, and this module owns only the
//! concurrency-safe **append mechanism**:
//!
//! - [`read_head`] reads the latest `(seq, hash)` for a company — the
//!   only pre-batch read a posting needs.
//! - [`append_statement`] inserts `(company_id, head.seq + 1,
//!   head.hash, …)`: the new event claims the head's **next** sequence
//!   number and links to the head's hash.
//!
//! Because `(company_id, seq)` is the primary key, a fork is impossible
//! by storage: two batches that read the same head both append the same
//! next `seq`, one commits, and the other takes a **unique violation**
//! that aborts its whole batch. That failure is the one class
//! [`crate::retry`] retries — re-reading the head and re-building the
//! batch is exactly what a correct retry needs — which is why the head
//! is read fresh by the plan, never carried across attempts.
//!
//! An empty chain reads as [`GENESIS_HASH`] at `seq` 0, so the first
//! append links `"0"` — the conventional chain anchor.

use cratefield_core::{Database, DbError, Statement};
use sea_query::Value as SeaValue;

/// The `prev_hash` of the first event in a chain.
pub const GENESIS_HASH: &str = "0";

/// The chain state a new event appends onto: the latest event's
/// sequence number and hash, or `seq` 0 with [`GENESIS_HASH`] for an
/// empty chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// The last event's `seq`; 0 when the chain is empty.
    pub seq: i64,
    /// The last event's `hash`; [`GENESIS_HASH`] when the chain is
    /// empty.
    pub hash: String,
}

/// Reads the chain head for `company_id` — the latest event by `seq`,
/// or the genesis head when no event exists yet.
///
/// # Errors
///
/// [`DbError`] when the query fails or a returned row lacks an integer
/// `seq` / text `hash` — the port's row model, not chain semantics.
pub async fn read_head(db: &dyn Database, company_id: &str) -> Result<Head, DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT seq, hash FROM audit_event WHERE company_id = ?\n\
             ORDER BY seq DESC LIMIT 1"
                .to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await?;
    let Some(row) = rows.first() else {
        return Ok(Head {
            seq: 0,
            hash: GENESIS_HASH.to_owned(),
        });
    };
    let seq = row
        .get::<i64>("seq")
        .ok_or_else(|| DbError::Query("column seq missing or not an integer".to_owned()))?;
    let hash = row
        .get::<String>("hash")
        .ok_or_else(|| DbError::Query("column hash missing or not text".to_owned()))?;
    Ok(Head { seq, hash })
}

/// The append statement: claims `head.seq + 1` for `company_id` and
/// links `prev_hash` to `head.hash`. Place it last in the batch it
/// sequences — everything before it is the work the event records.
///
/// The primary key `(company_id, seq)` does the fork-freedom: a stale
/// head makes this statement take a unique violation, aborting the
/// batch (see the module docs).
#[must_use]
pub fn append_statement(
    company_id: &str,
    head: &Head,
    hash: &str,
    payload: &str,
    created_at: &str,
) -> Statement {
    Statement::with_values(
        "INSERT INTO audit_event (company_id, seq, prev_hash, hash, payload, created_at)\n\
         VALUES (?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(company_id),
            SeaValue::BigInt(Some(head.seq + 1)),
            SeaValue::from(head.hash.as_str()),
            SeaValue::from(hash),
            SeaValue::from(payload),
            SeaValue::from(created_at),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_append_claims_the_next_seq_and_links_the_head() {
        let head = Head {
            seq: 41,
            hash: "hash-41".to_owned(),
        };
        let stmt = append_statement("co", &head, "hash-42", "payload", "2026-04-10T10:00:00Z");
        assert_eq!(
            stmt.sql,
            "INSERT INTO audit_event (company_id, seq, prev_hash, hash, payload, created_at)\n\
             VALUES (?, ?, ?, ?, ?, ?)"
        );
        assert_eq!(
            stmt.values.0,
            vec![
                SeaValue::from("co"),
                SeaValue::BigInt(Some(42)),
                SeaValue::from("hash-41"),
                SeaValue::from("hash-42"),
                SeaValue::from("payload"),
                SeaValue::from("2026-04-10T10:00:00Z"),
            ]
        );
    }

    #[test]
    fn the_genesis_head_is_seq_zero_on_the_anchor_hash() {
        let head = Head {
            seq: 0,
            hash: GENESIS_HASH.to_owned(),
        };
        let stmt = append_statement("co", &head, "hash-1", "payload", "t");
        assert_eq!(stmt.values.0[1], SeaValue::BigInt(Some(1)));
        assert_eq!(stmt.values.0[2], SeaValue::from(GENESIS_HASH));
    }
}
