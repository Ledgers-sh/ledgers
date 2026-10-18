#![forbid(unsafe_code)]
//! Storage foundation for Ledgers: one schema that runs on the harness
//! `Database` port — D1, SQLite and Postgres through one set of portable
//! migrations (docs/PLAN.md §2, "Storage on D1, SQLite, Postgres").
//!
//! # The storage rules
//!
//! - This crate owns the anchor schema every domain crate builds on: the
//!   single `company` row, and `company_setting` as the table that fixes
//!   the `company_id` convention for everything that follows. Domain
//!   crates (accounts, journal, …) embed their own migrations the same
//!   way and declare their tables for the harness.
//! - Migrations are embedded per crate with `include_str!` from
//!   `migrations/sqlite/NNNN_name.sql` and exposed as
//!   [`SQLITE_MIGRATIONS`], checked at compile time by
//!   [`cratefield_core::assert_migration_set`]. Applied migrations are
//!   never edited — corrections are new migrations.
//! - Conventions every Ledgers migration follows: ids are ULID `TEXT`;
//!   timestamps are ISO 8601 `TEXT` in UTC; amounts are `BIGINT` minor
//!   units — never `INTEGER`, because Postgres `INTEGER` is 32-bit while
//!   SQLite's type affinity maps `BIGINT` to a 64-bit integer; and the SQL
//!   stays inside the harness's portable subset (no `AUTOINCREMENT`,
//!   `NOW()`, `json_extract`, FTS5 or backticks).
//! - Queries are built with sea-query and rendered through
//!   [`cratefield_core::Statement::render`], which emits SQLite-dialect
//!   SQL with `?` placeholders; the Postgres adapter rebinds them to
//!   `$n`.
//! - [`suite`] holds the multi-backend conformance scenarios every
//!   backing engine must pass; `tests/matrix.rs` runs them over every
//!   dialect the environment provides.

pub mod module;
pub mod suite;

pub use module::StorageModule;

/// The storage foundation migrations: the `company` anchor and the
/// `company_setting` conventions table, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "company",
        include_str!("../migrations/sqlite/0001_company.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "company_setting",
        include_str!("../migrations/sqlite/0002_company_setting.sql"),
    ),
];

// Compile-time check on the set: ids run contiguously from 0001 in the
// order they are listed (cratefield-core, `assert_migration_set`).
const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
