//! Closed accounting periods: the write-time refusal that keeps entries
//! out of a period an operator has closed (docs/PLAN.md §3 — posted
//! entries are immutable, corrections are reversals, so nothing may be
//! written into a closed period at all).
//!
//! The check is a [`crate::guard`] probe over the `accounting_period`
//! table, and it **fails closed**: the guard's condition is
//!
//! ```sql
//! (SELECT status FROM accounting_period WHERE company_id = ? AND period = ?)
//!     = 'open'
//! ```
//!
//! so a **missing** period row yields NULL, `NULL = 'open'` is unknown,
//! and the probe aborts the batch. An unknown period is treated as
//! closed — the safe side for an accounting system. Only a row whose
//! status is literally `'open'` admits a post.
//!
//! Period administration itself (the operator-only close/reopen
//! surface, with its own policy checks — an agent key can never close a
//! period) lives in a later crate; [`set_status_statement`] is the
//! minimal knob the tests and that future surface share.

use crate::guard;
use cratefield_core::Statement;
use sea_query::Value as SeaValue;

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

/// Sets a period's status (`'open'` / `'closed'`), inserting or
/// updating in one statement so the knob needs no read-modify-write.
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
/// one. `guard_key` names the probe row; distinct asserts in one batch
/// need distinct keys ([`guard::guard_key`] builds them).
#[must_use]
pub fn assert_open_statement(guard_key: &str, company_id: &str, period: &str) -> Statement {
    guard::assert_statement(
        guard_key,
        "(SELECT status FROM accounting_period WHERE company_id = ? AND period = ?) = 'open'",
        vec![SeaValue::from(company_id), SeaValue::from(period)],
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
}
