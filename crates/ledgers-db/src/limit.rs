//! Agent daily limits, delegated to the harness implementation —
//! reused, not reimplemented.
//!
//! `cratefield_core::Usage` already ships the race-free allowance
//! counter this crate's rules need ("this agent may make N posts
//! today"): admission and increment are **one statement**, the guarded
//! upsert whose `used` column resolves to NULL when the allowance
//! cannot carry the amount, so a spent limit fails the statement — the
//! same guard-statement discipline as [`crate::guard`], with the table
//! it probes owned by the harness pattern itself. This module pins that
//! pattern's table into this crate's migrations (so the schema arrives
//! with the rest of the guard foundation) and hands back the statement:
//!
//! - [`AGENT_USAGE_TABLE`] is the table name;
//! - [`USAGE_TABLE_SQL`] is its DDL, kept byte-identical to
//!   `Usage::new(AGENT_USAGE_TABLE).create_table_sql()` by a test, so
//!   the migration can never drift from the implementation that reads
//!   and writes it;
//! - [`daily_consume_statement`] is the guarded consume for a UTC day,
//!   for the caller's `batch_atomic` beside the work it pays for.
//!
//! A spent allowance surfaces as a `NOT NULL` failure on `used` — a
//! permanent refusal [`crate::retry`] does not retry (retrying a
//! refusal would be wrong; only unique-violation races are legal to
//! retry).

use cratefield_core::{Period, Statement, Usage};
use time::OffsetDateTime;

/// The usage meter's table, as owned by this crate's migration.
pub const AGENT_USAGE_TABLE: &str = "agent_usage";

/// The DDL for [`AGENT_USAGE_TABLE`]: exactly what
/// `Usage::new(AGENT_USAGE_TABLE).create_table_sql()` returns, embedded
/// verbatim in `0001_guards.sql`. A test asserts the equality, so the
/// migration and the harness implementation cannot drift apart.
pub const USAGE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS agent_usage (\n    subject TEXT NOT NULL,\n    meter TEXT NOT NULL,\n    period_start TEXT NOT NULL,\n    used BIGINT NOT NULL,\n    PRIMARY KEY (subject, meter, period_start)\n);";

/// The guarded daily consume for `(subject, meter)`: admits `amount`
/// against `limit` in the UTC day containing `day`, or fails the
/// statement (`NOT NULL` on `used`) when the allowance cannot carry it.
///
/// The statement is the harness's own
/// `Usage::new(AGENT_USAGE_TABLE).consume_statement(..)` with
/// [`Period::Day`] and `Some(limit)` — put it in the same
/// `batch_atomic` as the work it pays for; on admission the work
/// commits, on refusal the whole batch rolls back and nothing was
/// spent.
#[must_use]
pub fn daily_consume_statement(
    subject: &str,
    meter: &str,
    day: OffsetDateTime,
    amount: u64,
    limit: u64,
) -> Statement {
    Usage::new(AGENT_USAGE_TABLE).consume_statement(
        subject,
        meter,
        Period::Day,
        day,
        amount,
        Some(limit),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_const_is_the_harness_ddl_verbatim() {
        assert_eq!(
            USAGE_TABLE_SQL,
            Usage::new(AGENT_USAGE_TABLE).create_table_sql(),
            "USAGE_TABLE_SQL must equal Usage::new(AGENT_USAGE_TABLE).create_table_sql(); \
             if the harness DDL changed, update the const and the migration together"
        );
    }

    #[test]
    fn the_migration_carries_the_harness_ddl_verbatim() {
        let migration = include_str!("../migrations/sqlite/0001_guards.sql");
        assert!(
            migration.contains(USAGE_TABLE_SQL),
            "0001_guards.sql must contain USAGE_TABLE_SQL byte for byte, or the migrated \
             table drifts from the implementation that reads it"
        );
    }

    #[test]
    fn the_daily_consume_binds_the_harness_statement_shape() {
        let day = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, time::Month::April, 10)
                .expect("a real calendar date"),
            time::Time::MIDNIGHT,
        )
        .assume_utc();
        let stmt = daily_consume_statement("agent-a", "post", day, 1, 2);
        // The harness's guarded upsert: 7 bindings (subject, meter,
        // period_start, amount, limit, amount, limit) and the CASE WHEN
        // NULL guard on `used`.
        assert!(stmt.sql.contains("CASE WHEN ? <= ? THEN ? ELSE NULL END"));
        assert!(
            stmt.sql
                .contains("ON CONFLICT (subject, meter, period_start) DO UPDATE")
        );
        assert_eq!(stmt.values.0.len(), 7);
        assert_eq!(stmt.values.0[0], sea_query::Value::from("agent-a"));
        assert_eq!(stmt.values.0[1], sea_query::Value::from("post"));
        assert_eq!(
            stmt.values.0[2],
            sea_query::Value::from("2026-04-10T00:00:00Z")
        );
        assert_eq!(stmt.values.0[3], sea_query::Value::BigInt(Some(1)));
        assert_eq!(stmt.values.0[4], sea_query::Value::BigInt(Some(2)));
    }
}
