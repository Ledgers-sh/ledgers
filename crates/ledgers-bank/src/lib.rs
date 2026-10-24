#![forbid(unsafe_code)]
//! Bank accounts, statement import and reconciliation (docs/PLAN.md section 6,
//! issues #26 and #48).
//!
//! - [`iban`]: IBAN validation with the mod-97 check (owned by ledgers-bills).
//! - [`parse`]: pure readers for `camt.053`, MT940 and delimited CSV with a
//!   saved column mapping. They return [`statement::ParsedStatement`]s whose
//!   balances are checked by [`statement::check_balances`].
//! - [`fingerprint`]: duplicate detection, so a re-import adds nothing and an
//!   overlapping file adds only its new lines.
//! - [`rules`] and [`suggest`]: bank rules and the deterministic ranking of
//!   suggested matches (bills, posted entries, rules).
//!
//! Commands, storage and the module live in [`commands`], [`store`] and
//! [`module`]. Remittance and counterparty text is data: no reader or ranker
//! interprets it as an instruction.

pub mod close;
pub mod commands;
pub mod fingerprint;
pub use ledgers_bills::iban;
pub mod model;
pub mod module;
pub mod money;
pub mod parse;
pub mod party_balances;
pub mod rules;
pub mod statement;
pub mod store;
pub mod suggest;

pub use module::BankModule;

/// The bank schema migrations, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "bank",
        include_str!("../migrations/sqlite/0001_bank.sql"),
    )];

/// The Postgres set: the same portable DDL as SQLite.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "bank",
        include_str!("../migrations/postgres/0001_bank.sql"),
    )];

const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
