#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
//! Sales invoices and credit notes (docs/PLAN.md section 6, issue #24).
//! Invoices are immutable once issued; corrections are credit notes only.
//! Template rendering for legally required fields (German §14 `UStG`).

pub mod commands;
pub mod model;
pub mod module;
pub mod store;
pub mod template;

pub use commands::{CreditInvoice, IssueInvoice};
pub use module::InvoiceModule;

/// The invoice migrations, in apply order: the portable set (SQLite dialect,
/// lint-checked, runs on D1, rusqlite and — for the tables — Postgres).
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "invoices",
        include_str!("../migrations/sqlite/0001_invoices.sql"),
    )];

/// The Postgres override set: identical DDL for the tables.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "invoices",
        include_str!("../migrations/postgres/0001_invoices.sql"),
    )];

// Compile-time check on both sets: ids run contiguously from 0001 in
// the order they are listed (cratefield-core, `assert_migration_set`).
const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_migration_set_is_contiguous() {
        for set in [SQLITE_MIGRATIONS, POSTGRES_MIGRATIONS] {
            assert_eq!(set.len(), 1);
            assert_eq!(set[0].id, "0001");
            assert_eq!(set[0].name, "invoices");
        }
    }

    #[test]
    fn the_sqlite_migrations_are_portable() {
        for migration in SQLITE_MIGRATIONS {
            assert!(
                cratefield_core::lint_portable_sql(migration.sql).is_empty(),
                "{} left the portable-SQL subset",
                migration.id
            );
        }
    }
}
