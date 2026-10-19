#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
//! The chart of accounts and the fiscal-year/period machinery for
//! Ledgers — docs/PLAN.md §6, the `ledgers-ledger` row ("Chart of
//! accounts, journals, entries, periods, reversals, numbering,
//! dimensions"): this crate owns the accounts part and the periods
//! part (fiscal years, soft-close/close/reopen, the year-end close);
//! the entry tables and their posting engine are the journal crate's
//! own work, reached through the [`journal::Journal`] port.
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
//! - **Closed periods reject postings, and reopening is human-only**
//!   (§3 rule 4). The refusal itself lives in the posting batch's
//!   period-postable guard (the [`journal::Journal`] implementation's
//!   contract); this crate owns the states, the transitions and their
//!   audit trail ([`period`]).
//! - **Money is exact here too.** Balances and the year-end close
//!   compute in `ledgers-money` integer minor units with checked
//!   arithmetic only, which is why this crate denies
//!   `clippy::float_arithmetic` from day one (CONTRIBUTING.md, "Money
//!   crates").
//!
//! # Storage
//!
//! The `account`, `fiscal_year` and `fiscal_year_opening_balance`
//! tables follow the storage foundation's conventions (natural keys,
//! ISO 8601 TEXT timestamps, portable SQL) with one deliberate
//! difference: no cross-module foreign keys, because module application
//! order is undefined — see the migrations' comments. Period rows live
//! in ledgers-db's `accounting_period` table (the guard foundation
//! owns the write-time check; this crate owns the administration).
//! Migrations are embedded from `migrations/sqlite/NNNN_name.sql`,
//! exposed as [`SQLITE_MIGRATIONS`], compile-checked by
//! [`cratefield_core::assert_migration_set`], and declared to the
//! harness by [`LedgerModule`].

pub mod account;
pub mod chart;
pub mod close;
pub mod commands;
pub mod error;
pub mod fiscal;
pub mod i18n;
pub mod journal;
pub mod lint;
pub mod mapping;
pub mod module;
pub mod period;
pub mod store;

pub use account::{Account, AccountDraft, AccountEntry, AccountType, NormalBalance};
pub use chart::{ChartAccount, ChartTemplate};
pub use close::{
    Blocker, CloseCheck, CloseCheckContext, CloseCheckRegistry, CloseReport,
    DraftEntriesCloseCheck, gather_blockers,
};
pub use commands::ImportReport;
pub use error::AccountsError;
pub use fiscal::{FiscalYear, FiscalYearSpec};
pub use i18n::{I18N_DE_TOML, I18N_EN_TOML, I18N_ID_TOML};
pub use journal::{AccountBalance, BoxFuture, Journal, NewEntry, NewEntryLine, PostedEntry};
pub use lint::{REQUIRED_SYSTEM_TAGS, Violation, ViolationKind, lint_chart};
pub use mapping::{SKR03_MAPPING_TOML, SKR04_MAPPING_TOML};
pub use module::LedgerModule;
pub use period::{
    Actor, PeriodState, YearEndReport, close_fiscal_year, close_period, fiscal_year,
    fiscal_year_periods, open_fiscal_year, period_state, reopen_period, soft_close_period,
};

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

/// The chart-of-accounts and fiscal-year migrations, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "accounts",
        include_str!("../migrations/sqlite/0001_accounts.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "fiscal_year",
        include_str!("../migrations/sqlite/0002_fiscal_year.sql"),
    ),
];

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
        assert_eq!(SQLITE_MIGRATIONS.len(), 2);
        assert_eq!(SQLITE_MIGRATIONS[0].id, "0001");
        assert_eq!(SQLITE_MIGRATIONS[0].name, "accounts");
        assert_eq!(SQLITE_MIGRATIONS[1].id, "0002");
        assert_eq!(SQLITE_MIGRATIONS[1].name, "fiscal_year");
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
