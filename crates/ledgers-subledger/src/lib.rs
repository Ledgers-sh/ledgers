#![forbid(unsafe_code)]
//! Open items, AR/AP subledgers and control-account reconciliation.
//! (docs/PLAN.md section 6, issue #47): tables, schemas, commands and
//! settlement logic for open items (invoices, bills, credit notes) and
//! allocations (payments, receipts, offsets, write-offs).

pub mod allocate;
pub mod builders;
pub mod close_check;
pub mod commands;
pub mod error;
pub mod item;
pub mod module;
pub mod offset;
pub mod reconcile;
pub mod reconcile_command;
pub mod write_off;

pub use module::SubledgerModule;

/// The subledger schema migrations, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "open_item",
        include_str!("../migrations/sqlite/0001_open_item.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "allocation",
        include_str!("../migrations/sqlite/0002_allocation.sql"),
    ),
];

/// The Postgres set: the same portable DDL as SQLite.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "open_item",
        include_str!("../migrations/postgres/0001_open_item.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "allocation",
        include_str!("../migrations/postgres/0002_allocation.sql"),
    ),
];

const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
