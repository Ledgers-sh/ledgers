//! Gapless numbering per `(journal_id, fiscal_year)`: entry numbers that
//! a rolled-back posting never leaves a hole in.
//!
//! The counter row is the serialisation point. A posting workflow:
//!
//! 1. **Pre-batch:** [`read_counter`] queries the current counter value
//!    (`next_number`), which may not exist yet (returns `None`); the caller
//!    computes `expected_number = current + 1`. The number must be known
//!    before the batch because it goes into the audit event's hashed input
//!    (computed before the batch), allowing `post_entry` to return it
//!    without reading after commit — so idempotent resubmits via `entry_id`
//!    can safely retry. The read is outside the batch because the number
//!    must be part of the pre-batch audit hash.
//!
//! 2. **In the batch:** [`provision_statement`] creates the counter row at
//!    `next_number = 0` if it does not exist (idempotent), [`bump_statement`]
//!    advances it by one (taking its lock, which serialises concurrent posts
//!    to one journal and year), and the caller's `INSERT` statement uses the
//!    pre-batch `expected_number` as a literal — not a `SELECT` from the
//!    counter. The batch follows with [`assert_counter_advanced_statement`],
//!    a guard that verifies the counter is now exactly `expected_number`:
//!
//! ```sql
//! INSERT INTO numbering_counter (journal_id, fiscal_year, next_number)
//! VALUES (?, ?, 0) ON CONFLICT (journal_id, fiscal_year) DO NOTHING;
//!
//! UPDATE numbering_counter SET next_number = next_number + 1
//! WHERE journal_id = ? AND fiscal_year = ?;
//!
//! INSERT INTO demo_entry (entry_id, journal_id, fiscal_year,
//!                          entry_date, created_at, number)
//! VALUES (?, ?, ?, ?, ?, ?);  -- number is expected_number, a literal
//!
//! INSERT INTO ledger_guard (guard_key, ok)
//! SELECT ?, CASE WHEN (SELECT next_number FROM numbering_counter
//!                      WHERE journal_id = ? AND fiscal_year = ?) = ?
//!        THEN 1 ELSE NULL END;
//! ```
//!
//! Three properties make this correct rather than merely plausible:
//!
//! - **The counter exists by batch-end:** [`provision_statement`] is
//!   idempotent, so re-running the batch (on retry) re-provisions it, and
//!   re-reading the guard mid-batch always sees it. A caller must
//!   [`read_counter`] and retry the whole plan on any error: the retry
//!   re-reads the counter (getting a fresh value if another post won the race),
//!   re-computes `expected_number`, and resubmits the batch with the new value.
//!
//! - **The bump takes the counter row's lock inside the batch**, so
//!   concurrent postings to one journal and year serialise on it: the
//!   second batch's `UPDATE` waits for the first to commit, then reads
//!   the advanced counter. The `UNIQUE (journal_id, fiscal_year, number)`
//!   constraint on the target table is the backstop for any path that
//!   bypasses the bump — its violation is the failure class [`crate::retry`]
//!   exists to retry.
//!
//! - **The guard verifies the counter equals the expected value:** if
//!   another post won the race and bumped the counter past `expected_number`
//!   before this post's `INSERT`, the `INSERT` hits the unique constraint,
//!   and retry re-plans with the new counter. If the counter somehow did not
//!   advance (schema drift, a bypass outside this code path), the guard fails
//!   with `NOT NULL`, preventing a gap.
//!
//! The canonical batch order is `[provision, bump, insert with literal number, assert_counter_advanced]`
//! — the exact reference [`crate::suite::plan_post`] keeps.

use cratefield_core::{Database, DbError, Statement};
use sea_query::Value as SeaValue;

/// Creates the counter row at `next_number = 0` if it does not exist
/// yet, and does nothing otherwise — idempotent, so every posting batch
/// can carry it without a separate provisioning step.
///
/// `next_number` is the number the **next** bump assigns: the first
/// post after provisioning takes number 1.
#[must_use]
pub fn provision_statement(journal_id: &str, fiscal_year: i64) -> Statement {
    Statement::with_values(
        "INSERT INTO numbering_counter (journal_id, fiscal_year, next_number)\n\
         VALUES (?, ?, 0)\n\
         ON CONFLICT (journal_id, fiscal_year) DO NOTHING"
            .to_owned(),
        vec![
            SeaValue::from(journal_id),
            SeaValue::BigInt(Some(fiscal_year)),
        ],
    )
}

/// Advances the counter by one inside the batch. The `UPDATE` takes the
/// counter row's lock, which is what serialises concurrent postings per
/// journal and year; a rolled-back batch rolls the bump back with it,
/// which is what keeps the sequence gapless.
#[must_use]
pub fn bump_statement(journal_id: &str, fiscal_year: i64) -> Statement {
    Statement::with_values(
        "UPDATE numbering_counter SET next_number = next_number + 1\n\
         WHERE journal_id = ? AND fiscal_year = ?"
            .to_owned(),
        vec![
            SeaValue::from(journal_id),
            SeaValue::BigInt(Some(fiscal_year)),
        ],
    )
}

/// Reads the current counter value (`next_number`) for a journal and fiscal
/// year. Returns `None` if no counter row exists (which should not happen
/// after [`provision_statement`] runs, but is handled for safety).
///
/// This is used pre-batch to compute the expected number, enabling
/// `post_entry` to return the number without reading it back after
/// commit (idempotent resubmits via `entry_id` check can then safely retry).
///
/// # Errors
///
/// Returns [`DbError`] when the database query fails.
pub async fn read_counter(
    db: &dyn Database,
    journal_id: &str,
    fiscal_year: i64,
) -> Result<Option<i64>, DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT next_number FROM numbering_counter WHERE journal_id = ? AND fiscal_year = ?"
                .to_owned(),
            vec![
                SeaValue::from(journal_id),
                SeaValue::BigInt(Some(fiscal_year)),
            ],
        ))
        .await?;
    Ok(rows.first().and_then(|row| row.get::<i64>("next_number")))
}

/// A guard statement that the counter moved from `expected - 1` to
/// `expected`: verifies a number assignment is exactly as planned.
/// Fires (NOT NULL fails) if the counter has any other value, e.g. a lost
/// race rewound it, a bypass inserted without bumping, or schema drift
/// broke the process.
///
/// Used as a backstop after a literal INSERT of the number: the counter
/// only grows and every number ≤ counter is already taken in `journal_entry`
/// for that (journal, fiscal year), so if another post won the race, the
/// counter was already ≥ expected, our INSERT hit the unique constraint,
/// and retry re-planned with fresh reads. This guard verifies the counter
/// is exactly what we expected, turning any broken invariant into a hard
/// refusal instead of a gap (issue #71).
#[must_use]
pub fn assert_counter_advanced_statement(
    guard_key: &str,
    journal_id: &str,
    fiscal_year: i64,
    expected: i64,
) -> Statement {
    Statement::with_values(
        "INSERT INTO ledger_guard (guard_key, ok)\n\
         SELECT ?, CASE WHEN (SELECT next_number FROM numbering_counter WHERE journal_id = ? AND fiscal_year = ?) = ? THEN 1 ELSE NULL END".to_owned(),
        vec![
            SeaValue::from(guard_key),
            SeaValue::from(journal_id),
            SeaValue::BigInt(Some(fiscal_year)),
            SeaValue::BigInt(Some(expected)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provision_starts_at_zero_and_conflicts_away() {
        let stmt = provision_statement("jrn-1", 2026);
        assert_eq!(
            stmt.sql,
            "INSERT INTO numbering_counter (journal_id, fiscal_year, next_number)\n\
             VALUES (?, ?, 0)\n\
             ON CONFLICT (journal_id, fiscal_year) DO NOTHING"
        );
        assert_eq!(
            stmt.values.0,
            vec![SeaValue::from("jrn-1"), SeaValue::BigInt(Some(2026))]
        );
    }
}
