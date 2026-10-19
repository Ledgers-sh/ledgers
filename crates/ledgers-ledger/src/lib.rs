#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
//! The chart of accounts for Ledgers — docs/PLAN.md §6, the
//! `ledgers-ledger` row ("Chart of accounts, journals, entries, periods,
//! reversals, numbering, dimensions"): this crate lands the accounts
//! part; journals and periods build on it.
//!
//! # The rules this crate owns
//!
//! - **Accounts are never deleted.** Posted entries are immutable
//!   (docs/PLAN.md §3 rule 3) and reference accounts by number, so an
//!   account must outlive every entry it ever touched:
//!   [`commands::delete_account`] always fails with
//!   [`AccountsError::DeleteUnsupported`], and archiving
//!   ([`commands::archive_account`]) is the supported retirement.
//! - **Postable is derived, never stored.** An account with children is a
//!   heading; a leaf is postable ([`account::AccountEntry`]). The flag is
//!   recomputed from `parent_number` on read, so adding a child to a leaf
//!   makes it a heading atomically — a stored flag could go stale, and a
//!   stale flag is unsafe for agents.
//! - **Parents before children.** [`commands::create_account`] refuses a
//!   missing parent, and [`commands::import_template`] inserts a chart in
//!   [`chart::ChartTemplate::in_dependency_order`] as one atomic batch.
//! - **Names are i18n keys, not strings.** Accounts carry
//!   `"account.*"` keys translated through the bundled tables in
//!   [`i18n`]; document text is data, never instructions (§3 rule 8).
//! - **Currencies are validated by `ledgers-money`** wherever a code
//!   string is accepted; `Currency` values in the domain types have
//!   passed `Currency::from_code` by construction.
//! - **No money arithmetic here yet.** Balances arrive with the journal;
//!   when they do they will be `ledgers-money` integer minor units,
//!   which is why this crate denies `clippy::float_arithmetic` from day
//!   one (CONTRIBUTING.md, "Money crates").
//!
//! # Storage
//!
//! The `account` table follows the storage foundation's conventions
//! (ULID-free natural key `(company_id, number)`, ISO 8601 TEXT
//! timestamps, portable SQL) with one deliberate difference: no
//! cross-module foreign key to `company(id)`, because module application
//! order is undefined — see the migration's comments. Migrations are
//! embedded from `migrations/sqlite/NNNN_name.sql`, exposed as
//! [`SQLITE_MIGRATIONS`], compile-checked by
//! [`cratefield_core::assert_migration_set`], and declared to the harness
//! by [`LedgerModule`].

pub mod account;
pub mod chart;
pub mod commands;
pub mod error;
pub mod i18n;
pub mod lint;
pub mod mapping;
pub mod module;
pub mod store;

pub use account::{Account, AccountDraft, AccountEntry, AccountType, NormalBalance};
pub use chart::{ChartAccount, ChartTemplate};
pub use commands::ImportReport;
pub use error::AccountsError;
pub use i18n::{I18N_DE_TOML, I18N_EN_TOML, I18N_ID_TOML};
pub use lint::{REQUIRED_SYSTEM_TAGS, Violation, ViolationKind, lint_chart};
pub use mapping::{SKR03_MAPPING_TOML, SKR04_MAPPING_TOML};
pub use module::LedgerModule;

/// The generic small-business chart (`templates/generic-sme.toml`).
pub const GENERIC_SME_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/generic-sme.toml"
));
/// The German HGB chart (`templates/de-hgb.toml`).
pub const DE_HGB_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/de-hgb.toml"
));

/// The chart of accounts migrations, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "accounts",
        include_str!("../migrations/sqlite/0001_accounts.sql"),
    )];

// Compile-time check on the set: ids run contiguously from 0001 in the
// order they are listed (cratefield-core, `assert_migration_set`).
const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);

/// The bundled generic small-business chart, parsed and type-validated
/// (lint is the caller's or [`commands::import_template`]'s to run).
///
/// # Errors
/// [`AccountsError::Template`] when the file no longer parses — a broken
/// data edit, not a runtime condition.
pub fn generic_sme_template() -> Result<chart::ChartTemplate, error::AccountsError> {
    chart::parse_chart(GENERIC_SME_TOML)
}

/// The bundled German HGB chart, parsed and type-validated.
///
/// # Errors
/// [`AccountsError::Template`] when the file no longer parses.
pub fn de_hgb_template() -> Result<chart::ChartTemplate, error::AccountsError> {
    chart::parse_chart(DE_HGB_TOML)
}

/// The bundled SKR03 and SKR04 number mappings, parsed. Validation
/// against the German chart is [`mapping::NumberMapping::validate_against`],
/// run by the integration tests for the bundled pair.
///
/// # Errors
/// [`AccountsError::Template`] when a file no longer parses.
pub fn bundled_mappings() -> Result<Vec<mapping::NumberMapping>, error::AccountsError> {
    Ok(vec![
        mapping::parse_mapping(SKR03_MAPPING_TOML)?,
        mapping::parse_mapping(SKR04_MAPPING_TOML)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_migration_set_is_contiguous() {
        // assert_migration_set already ran at compile time; this pins
        // the set's shape for readers.
        assert_eq!(SQLITE_MIGRATIONS.len(), 1);
        assert_eq!(SQLITE_MIGRATIONS[0].id, "0001");
        assert_eq!(SQLITE_MIGRATIONS[0].name, "accounts");
    }

    #[test]
    fn every_bundled_data_file_is_nonempty() {
        for (name, text) in [
            ("generic-sme.toml", GENERIC_SME_TOML),
            ("de-hgb.toml", DE_HGB_TOML),
            ("mappings/de-skr03.toml", SKR03_MAPPING_TOML),
            ("mappings/de-skr04.toml", SKR04_MAPPING_TOML),
            ("i18n/en.toml", I18N_EN_TOML),
            ("i18n/de.toml", I18N_DE_TOML),
            ("i18n/id.toml", I18N_ID_TOML),
        ] {
            assert!(!text.trim().is_empty(), "{name} is empty");
        }
    }
}
