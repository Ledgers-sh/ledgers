//! Accounting periods and their three states: the write-time refusal
//! that keeps entries out of a period an operator has closed, plus the
//! `soft-closed` middle state that still admits people while it refuses
//! agent keys (docs/PLAN.md §3 — posted entries are immutable,
//! corrections are reversals, so nothing may be written into a closed
//! period at all).
//!
//! A period row's status is one of three constants — [`STATUS_OPEN`],
//! [`STATUS_SOFT_CLOSED`], [`STATUS_CLOSED`] — and the posting check is
//! a [`crate::guard`] probe over the `accounting_period` table that
//! **fails closed**:
//!
//! ```sql
//! (SELECT CASE WHEN status = 'open' THEN 1
//!              WHEN status = 'soft-closed' AND ? = 1 THEN 1
//!              ELSE NULL END
//!  FROM accounting_period WHERE company_id = ? AND period = ?) = 1
//! ```
//!
//! (the `? = 1` bind is the poster: 1 for a person, 0 for an agent
//! key). So `open` admits anyone, `soft-closed` admits only a person,
//! and `closed` — or a **missing** period row, whose scalar subquery
//! yields NULL and `NULL = 1` is not true — admits no one. An unknown
//! period is treated as closed: the safe side for an accounting system.
//! That probe is [`assert_postable_statement`], the guard the posting
//! path carries; [`assert_open_statement`] is the stricter sibling that
//! admits only a row whose status is literally [`STATUS_OPEN`],
//! whoever is asking.
//!
//! Period administration itself (the operator-only close/reopen
//! surface, with its own policy checks — an agent key can never close a
//! period) lives in `ledgers-ledger`, the crate landing alongside this
//! change; [`set_status_statement`] is the minimal knob the tests and
//! that surface share.

use crate::guard;
use cratefield_core::Statement;
use sea_query::Value as SeaValue;

/// A period anyone may post into: people and agent keys alike.
pub const STATUS_OPEN: &str = "open";
/// A period only a person may post into — the soft close that stops
/// agent keys while an adjustment window is still open.
pub const STATUS_SOFT_CLOSED: &str = "soft-closed";
/// A period nobody may post into: corrections are reversals in a later
/// period, never edits here.
pub const STATUS_CLOSED: &str = "closed";

/// The period key of an ISO 8601 calendar date: `"YYYY-MM-DD"` →
/// `"YYYY-MM"`, or `None` when the shape is not 4-2-2 digits with
/// dashes. Shape only — a well-shaped but impossible date
/// (`"2026-02-30"`) still yields its month string; the caller that
/// needs a real calendar date (the usage meter's day, say) parses
/// further.
#[must_use]
pub fn period_of_date(entry_date: &str) -> Option<String> {
    let bytes = entry_date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let digits = (0..4).chain(5..7).chain(8..10);
    if digits
        .into_iter()
        .any(|index| !bytes[index].is_ascii_digit())
    {
        return None;
    }
    Some(entry_date[..7].to_owned())
}

/// Sets a period's status — [`STATUS_OPEN`], [`STATUS_SOFT_CLOSED`] or
/// [`STATUS_CLOSED`] — inserting or updating in one statement so the
/// knob needs no read-modify-write.
#[must_use]
pub fn set_status_statement(company_id: &str, period: &str, status: &str) -> Statement {
    Statement::with_values(
        "INSERT INTO accounting_period (company_id, period, status)\n\
         VALUES (?, ?, ?)\n\
         ON CONFLICT (company_id, period) DO UPDATE SET status = excluded.status"
            .to_owned(),
        vec![
            SeaValue::from(company_id),
            SeaValue::from(period),
            SeaValue::from(status),
        ],
    )
}

/// The period-open guard: a [`guard::assert_statement`] probe that
/// aborts the batch unless `accounting_period` holds a row for
/// `(company_id, period)` whose status is `'open'`.
///
/// Fail-closed by construction — see the module docs: a missing row
/// (a period nobody has opened) refuses the post exactly like a closed
/// one. The posting path itself carries the softer
/// [`assert_postable_statement`] (whose `soft-closed` admits people);
/// this strict probe is for callers that need the period literally
/// open, whoever is asking. `guard_key` names the probe row; distinct
/// asserts in one batch need distinct keys ([`guard::guard_key`]
/// builds them).
#[must_use]
pub fn assert_open_statement(guard_key: &str, company_id: &str, period: &str) -> Statement {
    guard::assert_statement(
        guard_key,
        "(SELECT status FROM accounting_period WHERE company_id = ? AND period = ?) = 'open'",
        vec![SeaValue::from(company_id), SeaValue::from(period)],
    )
}

/// Guard: posting is allowed into an `open` period by anyone, into a
/// `soft-closed` period only by people (`human`), and into a `closed`
/// period (or a period with no row — fails closed) by no one.
///
/// The flag crosses the wire as INTEGER 1/0 (the repo's boolean
/// convention) and the SQL compares `? = 1` rather than a bare
/// `AND ?`: an untyped parameter in boolean position is something
/// Postgres refuses to infer, while an integer comparison is valid on
/// every dialect. Fail-closed like every probe — a missing row makes
/// the scalar subquery NULL, `NULL = 1` is not true, and the batch
/// aborts. `guard_key` names the probe row; distinct asserts in one
/// batch need distinct keys ([`guard::guard_key`] builds them).
#[must_use]
pub fn assert_postable_statement(
    guard_key: &str,
    company_id: &str,
    period: &str,
    human: bool,
) -> Statement {
    guard::assert_statement(
        guard_key,
        "(SELECT CASE WHEN status = 'open' THEN 1 \
         WHEN status = 'soft-closed' AND ? = 1 THEN 1 \
         ELSE NULL END \
         FROM accounting_period WHERE company_id = ? AND period = ?) = 1",
        vec![
            SeaValue::BigInt(Some(i64::from(human))),
            SeaValue::from(company_id),
            SeaValue::from(period),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_calendar_date_reduces_to_its_period() {
        assert_eq!(period_of_date("2026-04-10").as_deref(), Some("2026-04"));
        assert_eq!(period_of_date("1999-12-31").as_deref(), Some("1999-12"));
    }

    #[test]
    fn malformed_dates_have_no_period() {
        for bad in [
            "",
            "2026-4-10",
            "2026-04-1",
            "20260410",
            "2026/04/10",
            "2026-04-10T00",
            "abcd-ef-gh",
        ] {
            assert_eq!(period_of_date(bad), None, "{bad} must not be a period");
        }
        // Shape only: impossible but well-shaped dates still have a
        // month string (documented above).
        assert_eq!(period_of_date("2026-02-30").as_deref(), Some("2026-02"));
        assert_eq!(period_of_date("2026-13-45").as_deref(), Some("2026-13"));
    }

    #[test]
    fn the_open_guard_fails_closed_by_sql_shape() {
        let stmt = assert_open_statement("period-open:jrn:2026-04:e-1", "jrn", "2026-04");
        assert_eq!(
            stmt.sql,
            "INSERT INTO ledger_guard (guard_key, ok)\n\
             SELECT ?, CASE WHEN (SELECT status FROM accounting_period WHERE company_id = ? \
             AND period = ?) = 'open' THEN 1 ELSE NULL END"
        );
        assert_eq!(
            stmt.values.0,
            vec![
                SeaValue::from("period-open:jrn:2026-04:e-1"),
                SeaValue::from("jrn"),
                SeaValue::from("2026-04"),
            ]
        );
    }

    #[test]
    fn the_postable_guard_gates_the_three_states_by_sql_shape() {
        let stmt =
            assert_postable_statement("period-postable:jrn:2026-04:e-1", "jrn", "2026-04", false);
        assert_eq!(
            stmt.sql,
            "INSERT INTO ledger_guard (guard_key, ok)\n\
             SELECT ?, CASE WHEN (SELECT CASE WHEN status = 'open' THEN 1 \
             WHEN status = 'soft-closed' AND ? = 1 THEN 1 ELSE NULL END \
             FROM accounting_period WHERE company_id = ? AND period = ?) = 1 \
             THEN 1 ELSE NULL END"
        );
        assert_eq!(
            stmt.values.0,
            vec![
                SeaValue::from("period-postable:jrn:2026-04:e-1"),
                SeaValue::BigInt(Some(0)),
                SeaValue::from("jrn"),
                SeaValue::from("2026-04"),
            ]
        );
    }

    #[test]
    fn the_postable_guard_binds_the_human_flag_as_one_or_zero() {
        let person = assert_postable_statement("k", "jrn", "2026-04", true);
        let agent = assert_postable_statement("k", "jrn", "2026-04", false);
        assert_eq!(person.values.0[1], SeaValue::BigInt(Some(1)));
        assert_eq!(agent.values.0[1], SeaValue::BigInt(Some(0)));
    }
}
