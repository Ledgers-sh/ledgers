//! The retry policy for guard batches — and, just as deliberately, the
//! failures it refuses to retry.
//!
//! A guard batch fails for exactly two reasons:
//!
//! - a **unique violation** — a race the batch lost fairly (a stale
//!   chain head, a numbering backstop trip). Another attempt, rebuilt
//!   against a fresh read of whatever went stale, can legitimately
//!   win; retrying is correct.
//! - a **`NOT NULL` refusal** — a guard fired (closed period, spent
//!   allowance, unbalanced entry) or something is genuinely broken.
//!   The batch said *no*. Re-running it re-asks the same question and
//!   must get the same answer; retrying a refusal would turn a policy
//!   refusal into a timing attack on it, so [`run_batch_with_retry`]
//!   returns those immediately as [`GuardError::Database`].
//!
//! Retrying correctly means **rebuilding**: the batch is re-planned
//! from fresh reads every attempt (the chain head, say, via
//! [`crate::chain::read_head`]), which is why the batch arrives as an
//! async `build` closure the loop re-runs, never as a prepared
//! `Vec<Statement>`.
//!
//! Sleeping is injected: the library takes a `FnMut(Duration)` and
//! never touches `std::thread` itself, so it stays wasm-clean — the
//! callers that can block pass `std::thread::sleep`, and a Workers
//! caller can back off its own way.

use cratefield_core::{DbError, Statement};
use std::time::Duration;

/// Whether a driver error is a uniqueness violation, best-effort across
/// engines: SQLite says `UNIQUE constraint failed: …`, Postgres
/// `duplicate key value violates unique constraint …`. The one failure
/// class a guard batch may legally retry.
///
/// `ledgers-ledger`'s store carries a private copy of this sniffing
/// for its account-number race; adopting this shared one is that
/// crate's refactor to make, not something this crate does for it.
#[must_use]
pub fn is_unique_violation(err: &DbError) -> bool {
    let message = match err {
        DbError::Execute(message) | DbError::Query(message) | DbError::Batch(message) => message,
    };
    let lowered = message.to_ascii_lowercase();
    lowered.contains("unique constraint") || lowered.contains("duplicate key")
}

/// How a guard batch retries: at most `max_attempts` runs of the whole
/// (re-planned) batch, sleeping the exponential ladder between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts, including the first: `8` means one try plus up
    /// to seven retries.
    pub max_attempts: u32,
    /// The delay before the first retry; doubles each retry.
    pub base_delay: Duration,
    /// The cap the doubling ladder saturates at.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(500),
        }
    }
}

impl RetryPolicy {
    /// The sleep ladder between attempts: `base`, `2×base`, `4×base`,
    /// … capped at `max_delay`, with exactly `max_attempts − 1` rungs
    /// (one sleep per gap between attempts).
    pub fn delays(&self) -> impl Iterator<Item = Duration> + '_ {
        let gaps = self.max_attempts.saturating_sub(1);
        (0..gaps).map(|rung| {
            // Saturating doubling: a huge rung clamps instead of
            // panicking, then the cap applies.
            let factor = 1u32.checked_shl(rung).unwrap_or(u32::MAX);
            self.base_delay.saturating_mul(factor).min(self.max_delay)
        })
    }
}

/// What a guard batch failure comes back as.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    /// The batch still conflicted after every retry attempt — the race
    /// it keeps losing did not clear. Callers surface this as
    /// contention (busy, try again later), not as a policy refusal.
    #[error("the batch still conflicts after {attempts} attempts")]
    Conflict { attempts: u32 },
    /// The database refused the batch: either a guard fired (the
    /// `NOT NULL` refusals — closed period, spent allowance, unbalanced
    /// entry) or the engine failed. Not retried; see the module docs
    /// for why a refusal must stay final.
    #[error(transparent)]
    Database(#[from] DbError),
}

/// Runs a guard batch under `policy`, re-planning it from scratch on
/// every attempt.
///
/// Each attempt calls `build` (which re-runs the pre-batch reads — the
/// chain head, say — against the database as it stands **now**), then
/// submits the statements with `db.batch_atomic`. Success returns
/// `Ok(())`; a unique violation with attempts remaining sleeps the next
/// ladder rung through `sleep` and re-plans; a unique violation with
/// none left returns [`GuardError::Conflict`]; anything else returns
/// [`GuardError::Database`] immediately — refusals are final.
///
/// # Errors
///
/// [`GuardError::Conflict`] when the retry budget is exhausted on
/// races; [`GuardError::Database`] for every refusal or failure —
/// including a `build` failure, since a failed read has nothing to
/// retry against.
pub async fn run_batch_with_retry(
    db: &dyn cratefield_core::Database,
    policy: RetryPolicy,
    mut build: impl AsyncFnMut() -> Result<Vec<Statement>, DbError>,
    mut sleep: impl FnMut(Duration),
) -> Result<(), GuardError> {
    let mut delays = policy.delays();
    let mut attempt: u32 = 1;
    loop {
        let statements = build().await.map_err(GuardError::Database)?;
        match db.batch_atomic(&statements).await {
            Ok(()) => return Ok(()),
            Err(err) if is_unique_violation(&err) => {
                if attempt >= policy.max_attempts {
                    return Err(GuardError::Conflict {
                        attempts: policy.max_attempts,
                    });
                }
                // Unreachable in practice (the ladder has one rung per
                // gap), but a policy is data: never divide by its
                // assumptions.
                sleep(delays.next().unwrap_or(policy.max_delay));
                attempt += 1;
            }
            Err(err) => return Err(GuardError::Database(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_violations_are_recognised_from_both_engines() {
        let sqlite = DbError::Batch(
            "UNIQUE constraint failed: audit_event.company_id, audit_event.seq".to_owned(),
        );
        let postgres = DbError::Batch(
            "duplicate key value violates unique constraint \"audit_event_pkey\"".to_owned(),
        );
        assert!(is_unique_violation(&sqlite));
        assert!(is_unique_violation(&postgres));
    }

    #[test]
    fn guard_refusals_are_not_unique_violations() {
        // The NOT NULL refusal a fired guard produces, per engine.
        let sqlite = DbError::Batch("NOT NULL constraint failed: ledger_guard.ok".to_owned());
        let postgres = DbError::Batch(
            "null value in column \"ok\" of relation \"ledger_guard\" violates not-null \
             constraint"
                .to_owned(),
        );
        assert!(!is_unique_violation(&sqlite));
        assert!(!is_unique_violation(&postgres));
        assert!(!is_unique_violation(&DbError::Execute(
            "connection closed".to_owned()
        )));
    }

    #[test]
    fn the_default_ladder_doubles_then_caps() {
        let policy = RetryPolicy::default();
        let ladder: Vec<Duration> = policy.delays().collect();
        assert_eq!(
            ladder,
            vec![
                Duration::from_millis(10),
                Duration::from_millis(20),
                Duration::from_millis(40),
                Duration::from_millis(80),
                Duration::from_millis(160),
                Duration::from_millis(320),
                Duration::from_millis(500),
            ]
        );
        assert_eq!(
            ladder.len(),
            usize::try_from(policy.max_attempts - 1).expect("small")
        );
    }

    #[test]
    fn a_single_attempt_policy_has_no_gaps_to_sleep() {
        let policy = RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        };
        assert_eq!(policy.delays().count(), 0);
    }
}
