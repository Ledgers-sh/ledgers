#![forbid(unsafe_code)]
//! Vendor bills (docs/PLAN.md section 6, issue #22): a bill is an extracted
//! (or hand-typed) vendor invoice linked to its document. It becomes a
//! balanced journal entry (expense lines, input VAT, accounts payable) when a
//! person or an approved agent posts it, through the ordinary command executor
//! and policy.
//!
//! Status is derived on every read from the stored checks, flags and vendor:
//! `needs-review` while any check fails or any flag is open, `ready` when
//! nothing is open, `posted` once the entry exists. A bill never posts from
//! an extraction on its own: posting is always a command.

pub mod commands;
pub mod model;
pub mod module;
pub mod parties;
pub mod store;

pub use module::BillsModule;

/// The bill schema migrations, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "bills",
        include_str!("../migrations/sqlite/0001_bills.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "bill_reader",
        include_str!("../migrations/sqlite/0002_bill_reader.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0003",
        "parties",
        include_str!("../migrations/sqlite/0003_parties.sql"),
    ),
];

/// The Postgres set: the same portable DDL as SQLite.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "bills",
        include_str!("../migrations/postgres/0001_bills.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "bill_reader",
        include_str!("../migrations/postgres/0002_bill_reader.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0003",
        "parties",
        include_str!("../migrations/postgres/0003_parties.sql"),
    ),
];

const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
