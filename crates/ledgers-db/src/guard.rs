//! The generic probe guard: one statement that fails the whole
//! `batch_atomic` when a scalar SQL condition is not true.
//!
//! The port's batches report no row counts, so "did nothing" is not a
//! signal a caller can see. The probe turns a condition into a failure
//! instead: it inserts into `ledger_guard`, whose `ok` column is
//! `NOT NULL`, a row whose `ok` is the condition itself —
//!
//! ```sql
//! INSERT INTO ledger_guard (guard_key, ok)
//! SELECT ?, CASE WHEN <condition> THEN 1 ELSE NULL END
//! ```
//!
//! — so a false **or unknown** (NULL) condition produces a `NULL` for a
//! `NOT NULL` column: the statement aborts, and with it the entire
//! batch. This is the same shape the harness's usage metering uses
//! (`cratefield_core::Usage::consume_statement`, whose guarded column
//! is `used`), lifted out as a reusable probe.
//!
//! `condition_sql` must be a **scalar SQL expression** (not a top-level
//! query): both engines evaluate it per row of the `SELECT`, and a NULL
//! anywhere in it (a missing row, a NULL column, a three-valued-logic
//! unknown) means the guard fires. Callers bind the condition's `?`
//! values through `condition_values`; the identifiers in the condition
//! itself are library-authored SQL, never user input.

use cratefield_core::Statement;
use sea_query::Value as SeaValue;

/// The probe insert: succeeds (with `ok = 1`) when `condition_sql`
/// evaluates true on this batch's view of the database, and aborts the
/// batch through the `NOT NULL` on `ok` when it evaluates false or
/// unknown.
///
/// The `guard_key` names the probe row; every assert in a batch pairs
/// with a [`release_statement`] later in the **same** batch (see that
/// function for why the pairing matters).
#[must_use]
pub fn assert_statement(
    guard_key: impl Into<String>,
    condition_sql: &str,
    condition_values: Vec<SeaValue>,
) -> Statement {
    let mut values = Vec::with_capacity(condition_values.len() + 1);
    values.push(SeaValue::from(guard_key.into().as_str()));
    values.extend(condition_values);
    Statement::with_values(
        format!(
            "INSERT INTO ledger_guard (guard_key, ok)\n\
             SELECT ?, CASE WHEN {condition_sql} THEN 1 ELSE NULL END"
        ),
        values,
    )
}

/// Removes a probe row inserted by [`assert_statement`].
///
/// Probe rows are transient bookkeeping, not audit state: the release
/// belongs later in the **same** batch as its assert, so a committed
/// batch leaves no `ledger_guard` rows behind. Pairing inside one batch
/// is enough because a failed batch rolls back completely — the assert
/// that would have been left dangling is rolled back with everything
/// else. Two asserts in one batch need distinct keys (the column is the
/// primary key); [`guard_key`] exists to keep them distinct.
#[must_use]
pub fn release_statement(guard_key: impl Into<String>) -> Statement {
    Statement::with_values(
        "DELETE FROM ledger_guard WHERE guard_key = ?".to_owned(),
        vec![SeaValue::from(guard_key.into().as_str())],
    )
}

/// Joins guard-key parts with `':'` — the namespacing helper that keeps
/// two asserts in one batch from colliding on the `ledger_guard`
/// primary key (`"period-open:jrn-1:2026-04:e-42"` cannot collide with
/// `"balanced:e-42"`).
#[must_use]
pub fn guard_key(parts: &[&str]) -> String {
    parts.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_join_on_the_separator() {
        assert_eq!(guard_key(&["balanced", "e-42"]), "balanced:e-42");
        assert_eq!(guard_key(&[]), "");
    }

    #[test]
    fn the_assert_binds_the_key_first_then_the_condition_values() {
        let stmt = assert_statement("k", "amount <= ?", vec![SeaValue::BigInt(Some(7))]);
        assert_eq!(
            stmt.sql,
            "INSERT INTO ledger_guard (guard_key, ok)\n\
             SELECT ?, CASE WHEN amount <= ? THEN 1 ELSE NULL END"
        );
        assert_eq!(stmt.values.0.len(), 2);
        assert_eq!(stmt.values.0[0], SeaValue::from("k"));
        assert_eq!(stmt.values.0[1], SeaValue::BigInt(Some(7)));
    }

    #[test]
    fn the_release_targets_one_key() {
        let stmt = release_statement("k");
        assert_eq!(stmt.sql, "DELETE FROM ledger_guard WHERE guard_key = ?");
        assert_eq!(stmt.values.0, vec![SeaValue::from("k")]);
    }
}
