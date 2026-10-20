#![forbid(unsafe_code)]
//! Shared database guard statements for Ledgers: the write-time
//! invariant patterns every writing crate uses on the harness `Database`
//! port — gapless numbering, closed-period refusal, agent limits and the
//! audit chain's sequencing (docs/PLAN.md §2, "Atomic writes").
//!
//! # Why guard statements
//!
//! The port offers `execute`, `query` and `batch_atomic` — an
//! all-or-nothing batch, **no interactive transactions** — and
//! `batch_atomic` reports no row counts. A statement that "does nothing"
//! when its invariant is broken (an `UPDATE` matching zero rows, a
//! conditional insert skipped) is therefore invisible: the batch
//! commits, the work it paid for never happened, and the caller cannot
//! tell. Every invariant that must hold at write time instead has to
//! **fail the statement**, so the whole batch rolls back.
//!
//! The pattern is the one the harness itself ships for usage metering
//! (`cratefield_core::Usage::consume_statement`): a guarded write whose
//! guarded column resolves to `NULL` when the invariant is violated —
//! the `NOT NULL` constraint then aborts the statement, and with it the
//! batch. This crate is that pattern, generalised:
//!
//! ```sql
//! INSERT INTO ledger_guard (guard_key, ok)
//! SELECT ?, CASE WHEN <condition> THEN 1 ELSE NULL END
//! ```
//!
//! `ok` is `NOT NULL`, so the insert succeeds only when `<condition>`
//! is true; false or unknown (NULL) aborts the batch. [`guard`]
//! builds the probe, [`period::assert_open_statement`] is the worked
//! closed-period instance, and [`limit`] delegates to the harness's
//! own guarded upsert for daily agent allowances. [`numbering`] makes
//! journal entry numbers gapless (bump and insert read one counter row
//! inside the same batch), [`chain`] makes audit-chain forks impossible
//! (the head's `seq` is part of the primary key), and [`retry`] is the
//! policy for the one failure class that is legal to retry — a unique
//! violation from a race, never a `NOT NULL` refusal.
//!
//! [`suite`] is the worked posting example the guards ADR references
//! (docs/adr/0004-guard-statements.md): one `batch_atomic` posting an
//! entry under all five guards at once.
//!
//! # Conventions
//!
//! Money never appears here, but the storage rules it rests on do:
//! amounts elsewhere are `BIGINT` minor units (never `INTEGER` —
//! Postgres `INTEGER` is 32-bit), ids and timestamps are ISO 8601
//! `TEXT` bound from code (never `NOW()`), and the SQL is the
//! hand-written portable SQLite dialect that runs on D1, rusqlite and
//! Postgres (`?` placeholders, rebound by the Postgres adapter).

pub mod chain;
pub mod guard;
pub mod limit;
pub mod module;
pub mod numbering;
pub mod outbox;
pub mod period;
pub mod retry;
pub mod suite;

pub use module::DbModule;
pub use retry::{GuardError, RetryPolicy};

/// The guard-statement migrations: the numbering counter, the probe
/// table, period status, the audit chain's spine, the usage meter's
/// table, the outbox queue and the audit chain's append-only triggers,
/// in apply order.
const GUARDS: cratefield_core::SqlMigration = cratefield_core::SqlMigration::new(
    "0001",
    "guards",
    include_str!("../migrations/sqlite/0001_guards.sql"),
);

const OUTBOX: cratefield_core::SqlMigration = cratefield_core::SqlMigration::new(
    "0002",
    "outbox",
    include_str!("../migrations/sqlite/0002_outbox.sql"),
);

/// The guard-statement migrations, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    GUARDS,
    OUTBOX,
    cratefield_core::SqlMigration::new(
        "0003",
        "audit_event_append_only",
        include_str!("../migrations/sqlite/0003_audit_event_append_only.sql"),
    ),
];

/// The Postgres set: the portable migrations *themselves* — a copy
/// under `migrations/postgres/` would be a second statement of what
/// the schema is, free to drift — and the one migration whose SQL is
/// dialect-specific (the append-only triggers: `RAISE(ABORT, …)` on
/// SQLite, a plpgsql function on Postgres).
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    GUARDS,
    OUTBOX,
    cratefield_core::SqlMigration::new(
        "0003",
        "audit_event_append_only",
        include_str!("../migrations/postgres/0003_audit_event_append_only.sql"),
    ),
];

// Compile-time checks on both sets: ids run contiguously from 0001 in
// the order they are listed (cratefield-core, `assert_migration_set`).
const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_migration_set_is_contiguous() {
        // assert_migration_set already ran at compile time; this pins
        // the sets' shape for readers.
        for set in [SQLITE_MIGRATIONS, POSTGRES_MIGRATIONS] {
            assert_eq!(set.len(), 3);
            assert_eq!(set[0].id, "0001");
            assert_eq!(set[0].name, "guards");
            assert_eq!(set[1].id, "0002");
            assert_eq!(set[1].name, "outbox");
            assert_eq!(set[2].id, "0003");
            assert_eq!(set[2].name, "audit_event_append_only");
        }
    }

    #[test]
    fn the_migration_stays_in_the_portable_subset() {
        // The workspace-wide gate in ledgers-storage's matrix lints this
        // file too; the crate checks its own migration so a standalone
        // `cargo test -p ledgers-db` fails the same way. The trigger
        // file lints clean like ledgers-ledger's journal triggers do:
        // the lint rejects portability escapes (NOW(), AUTOINCREMENT,
        // json_extract, FTS5, backticks), and trigger DDL is not among
        // them.
        for migration in SQLITE_MIGRATIONS {
            assert!(
                cratefield_core::lint_portable_sql(migration.sql).is_empty(),
                "{} left the portable-SQL subset",
                migration.id
            );
        }
    }

    #[test]
    fn the_postgres_set_overrides_only_the_dialect_sql() {
        // If a fourth migration arrives, this is the test that says
        // whether it needs a postgres copy.
        for (portable, postgres) in SQLITE_MIGRATIONS.iter().zip(POSTGRES_MIGRATIONS) {
            assert_eq!(
                portable.id != "0003",
                portable.sql == postgres.sql,
                "migration {} must be the portable set's own SQL unless its SQL is \
                 dialect-specific",
                portable.id
            );
        }
    }
}
