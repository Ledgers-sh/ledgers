//! Gapless numbering per `(journal_id, fiscal_year)`: entry numbers that
//! a rolled-back posting never leaves a hole in.
//!
//! The counter row is the serialisation point. A posting batch
//! provisions it (idempotently), bumps it, and inserts the entry
//! **reading the counter in the same batch** — so the number handed out
//! and the increment that retires it commit or roll back together:
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
//! SELECT ?, ?, ?, ?, ?, next_number FROM numbering_counter
//! WHERE journal_id = ? AND fiscal_year = ?;
//! ```
//!
//! (the third statement is [`insert_with_number`] rendering). Two
//! details make this correct rather than merely plausible:
//!
//! - **The bump takes the counter row's lock inside the batch**, so
//!   concurrent postings to one journal and year serialise on it: the
//!   second batch's `UPDATE` waits for the first to commit, then reads
//!   the advanced counter. The `UNIQUE (journal_id, fiscal_year,
//!   number)` constraint on the target table is the backstop for any
//!   path that bypasses the bump — its violation is the failure class
//!   [`crate::retry`] exists to retry.
//! - **Provision runs first, in the same batch.** An `INSERT ...
//!   SELECT` whose source row is missing inserts nothing — silently,
//!   because `batch_atomic` reports no row counts — so the order is
//!   what rules the missing counter out, and re-running the idempotent
//!   provision in every batch is what keeps the order honest.
//! - **`number` in the target table must be `NOT NULL`** all the same:
//!   `next_number` itself is `NOT NULL`, so a NULL can only reach the
//!   insert through schema drift (a target column loosened to nullable,
//!   a different source expression) — and the constraint turns that
//!   drift into a failed batch instead of a stored NULL.
//!
//! The canonical batch order is `[provision, bump, insert_with_number]`
//! — the exact reference [`crate::suite::plan_post`] keeps.

use cratefield_core::Statement;
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

/// Inserts into `table`, taking the entry's number from the counter row
/// the same batch just bumped.
///
/// The rendered statement is `INSERT INTO {table} ({columns},
/// {number_column}) SELECT {placeholders}, next_number FROM
/// numbering_counter WHERE journal_id = ? AND fiscal_year = ?`, binding
/// `values` followed by `(journal_id, fiscal_year)`. `number_column`
/// must be `NOT NULL` in the target table, and the batch must have run
/// [`provision_statement`] first (see the module docs for both). `table`,
/// `columns` and `number_column` are library-authored identifiers —
/// compile-time strings, never user input.
#[must_use]
pub fn insert_with_number(
    table: &str,
    columns: &[&str],
    values: Vec<SeaValue>,
    number_column: &str,
    journal_id: &str,
    fiscal_year: i64,
) -> Statement {
    // The insert's column list is the caller's columns plus the number
    // column; the SELECT binds the caller's values, then reads the
    // counter. An empty `columns` slice still renders (the insert then
    // carries only the number column, and the SELECT only the counter).
    let mut names = columns.to_vec();
    names.push(number_column);
    let select_list = if columns.is_empty() {
        "next_number".to_owned()
    } else {
        format!("{}, next_number", vec!["?"; columns.len()].join(", "))
    };
    let mut bound = values;
    bound.push(SeaValue::from(journal_id));
    bound.push(SeaValue::BigInt(Some(fiscal_year)));
    Statement::with_values(
        format!(
            "INSERT INTO {table} ({})\n\
             SELECT {select_list} FROM numbering_counter\n\
             WHERE journal_id = ? AND fiscal_year = ?",
            names.join(", "),
        ),
        bound,
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

    #[test]
    fn the_numbered_insert_binds_values_then_the_counter_key() {
        let stmt = insert_with_number(
            "demo_entry",
            &["entry_id", "entry_date"],
            vec![SeaValue::from("e-1"), SeaValue::from("2026-04-10")],
            "number",
            "jrn-1",
            2026,
        );
        assert_eq!(
            stmt.sql,
            "INSERT INTO demo_entry (entry_id, entry_date, number)\n\
             SELECT ?, ?, next_number FROM numbering_counter\n\
             WHERE journal_id = ? AND fiscal_year = ?"
        );
        assert_eq!(
            stmt.values.0,
            vec![
                SeaValue::from("e-1"),
                SeaValue::from("2026-04-10"),
                SeaValue::from("jrn-1"),
                SeaValue::BigInt(Some(2026)),
            ]
        );
    }

    #[test]
    fn an_empty_column_list_still_renders_the_number_column() {
        let stmt = insert_with_number("t", &[], Vec::new(), "number", "jrn-1", 2026);
        assert_eq!(
            stmt.sql,
            "INSERT INTO t (number)\n\
             SELECT next_number FROM numbering_counter\n\
             WHERE journal_id = ? AND fiscal_year = ?"
        );
    }
}
