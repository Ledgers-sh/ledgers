//! Request timing for staging: a `Server-Timing` response header with the
//! database work a request did.
//!
//! Enabled by `LEDGERS_SERVER_TIMING=1` (set on the staging environment in
//! `wrangler.toml`, never on production). When it is off nothing is wrapped,
//! counted or written, so the request path is unchanged.
//!
//! The header carries three metrics, read from the request's own counters:
//!
//! - `total;dur=<ms>`: the Worker's wall time for the request (set by the
//!   Worker entry point around the runtime's `serve`).
//! - `db;dur=<ms>`: wall time spent in database calls.
//! - `db-calls;dur=<n>`: database round trips (one per `query`, `execute` or
//!   `batch_atomic`).
//! - `db-statements;dur=<n>`: SQL statements those calls carried (a batch of
//!   `n` counts as `n` statements in one call).
//!
//! The counts are not durations; the `dur` parameter is only the header's
//! carrier for a number.
//!
//! Counters live in a [`DbTiming`] created per request, so concurrent
//! requests in one isolate never see each other's numbers.

use async_trait::async_trait;
use cratefield_core::{Config, Database, DbError, Rows, Statement};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The configuration key that turns request timing on. Only the value `1`
/// enables it.
pub const TIMING_ENV: &str = "LEDGERS_SERVER_TIMING";

/// Whether request timing is enabled by configuration.
#[must_use]
pub fn enabled(cfg: &dyn Config) -> bool {
    cfg.get(TIMING_ENV).as_deref() == Some("1")
}

/// Milliseconds on the Worker's clock (`Date.now()`) or, natively, since the
/// Unix epoch. Monotonic enough for request-scale durations; `std::time::Instant`
/// is not available on `wasm32-unknown-unknown`.
#[must_use]
pub fn now_ms() -> f64 {
    now_since_epoch().as_secs_f64() * 1000.0
}

#[cfg(target_arch = "wasm32")]
fn now_since_epoch() -> Duration {
    Duration::from_millis(worker::Date::now().as_millis())
}

#[cfg(not(target_arch = "wasm32"))]
fn now_since_epoch() -> Duration {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
}

/// One request's database counters.
#[derive(Debug, Default)]
pub struct DbTiming {
    calls: AtomicU64,
    statements: AtomicU64,
    /// Accumulated wall time, in microseconds.
    micros: AtomicU64,
}

/// A read of [`DbTiming`]'s counters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DbSnapshot {
    /// Database round trips.
    pub calls: u64,
    /// Statements those round trips carried.
    pub statements: u64,
    /// Wall time in database calls, in milliseconds.
    pub ms: f64,
}

impl DbTiming {
    fn record(&self, statements: u64, started_ms: f64) {
        let elapsed = Duration::from_secs_f64((now_ms() - started_ms).max(0.0) / 1000.0);
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.statements.fetch_add(statements, Ordering::Relaxed);
        self.micros.fetch_add(micros, Ordering::Relaxed);
    }

    /// The counters as they stand.
    #[must_use]
    pub fn snapshot(&self) -> DbSnapshot {
        DbSnapshot {
            calls: self.calls.load(Ordering::Relaxed),
            statements: self.statements.load(Ordering::Relaxed),
            ms: Duration::from_micros(self.micros.load(Ordering::Relaxed)).as_secs_f64() * 1000.0,
        }
    }

    /// The `Server-Timing` value for the database counters.
    #[must_use]
    pub fn server_timing(&self) -> String {
        let snap = self.snapshot();
        format!(
            "db;dur={:.2}, db-calls;dur={}, db-statements;dur={}",
            snap.ms, snap.calls, snap.statements
        )
    }
}

/// A [`Database`] that counts and times every call it forwards.
pub struct CountingDb {
    inner: Arc<dyn Database>,
    timing: Arc<DbTiming>,
}

impl CountingDb {
    /// Wraps `inner`, recording into `timing`.
    #[must_use]
    pub fn new(inner: Arc<dyn Database>, timing: Arc<DbTiming>) -> Self {
        Self { inner, timing }
    }
}

#[async_trait]
impl Database for CountingDb {
    async fn execute(&self, stmt: &Statement) -> Result<u64, DbError> {
        let started = now_ms();
        let result = self.inner.execute(stmt).await;
        self.timing.record(1, started);
        result
    }

    async fn query(&self, stmt: &Statement) -> Result<Rows, DbError> {
        let started = now_ms();
        let result = self.inner.query(stmt).await;
        self.timing.record(1, started);
        result
    }

    async fn batch_atomic(&self, stmts: &[Statement]) -> Result<(), DbError> {
        let started = now_ms();
        let result = self.inner.batch_atomic(stmts).await;
        self.timing.record(stmts.len() as u64, started);
        result
    }
}
