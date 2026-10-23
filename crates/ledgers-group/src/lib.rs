#![forbid(unsafe_code)]
//! Company groups and intercompany agreements (issue #128, priorities 3 to 5
//! and 7; docs/PLAN.md section 1 keeps consolidation in Ledgers Cloud).
//!
//! A group names its member companies and what each one is (holding, IP
//! holding, operating). An intercompany agreement is a licence, royalty,
//! management fee or loan between two members. Each period the agreement
//! produces one draft per side: the licensee's expense and payable (with any
//! withholding tax) and the licensor's receivable and income. Posting goes
//! through each company's own executor, licensor first, and a failure on the
//! second side reverses the first, so a period posts on both sides or on
//! neither. Settlement pays the net amount and clears the receivable gross.
//!
//! Every balance here is in one currency per agreement, which must be the base
//! currency of both companies. Cross-currency intercompany accruals need FX
//! gain and loss, which is not built yet (a TODO), so they are refused.
//!
//! The commands are human-only. Cross-company access is checked by the server,
//! which passes each command the companies the person may see.

pub mod accrual;
pub mod commands;
pub mod model;
pub mod module;
pub mod orchestrate;
pub mod reports;
pub mod store;

pub use module::GroupModule;

/// The group schema migrations, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "groups",
        include_str!("../migrations/sqlite/0001_groups.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "group_logo",
        include_str!("../migrations/sqlite/0002_group_logo.sql"),
    ),
];

/// The Postgres set: the same portable DDL as SQLite.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "groups",
        include_str!("../migrations/postgres/0001_groups.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "group_logo",
        include_str!("../migrations/postgres/0002_group_logo.sql"),
    ),
];

const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
