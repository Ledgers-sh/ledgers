#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
//! Tax codes, rates, and calculations for Ledgers — docs/PLAN.md §6
//! milestone M0.
//!
//! # Tax handling
//!
//! This crate provides:
//!
//! - [`rate`]: Tax rates as dated data per jurisdiction (Germany first;
//!   generic builder for caller-supplied rates). Rates are validated at
//!   construction for gaps and overlaps.
//! - [`code`]: Tax codes and kinds (output VAT, input VAT, reverse charge,
//!   etc.) with account mappings and non-deductible shares.
//! - [`calc`]: Tax calculation on document lines, net-based and gross-based,
//!   with per-line or per-document rounding via [`Rounding`](ledgers_money::Rounding).
//!   Small-business (Kleinunternehmer) exemption: no tax charged, required
//!   invoice note returned.
//! - [`posting`]: Convert calculated tax lines into [`ledgers_ledger::Line`]s
//!   for entry posting. Reverse charge posts both output and input VAT
//!   (balanced). Non-deductible shares post to the original line's account.
//! - [`settings`]: Company-level tax settings (small-business exemption flag,
//!   VAT reporting method). Human-only writes, guarded and audited.
//!
//! # Money and precision
//!
//! All amounts are [`Amount`](ledgers_money::Amount) — integer minor units,
//! never float. Tax codes use exact [`Rate`](ledgers_money::Rate) (i128
//! scaled by 10¹², see ADR 0003). Calculation rounding is explicit via
//! [`Rounding`](ledgers_money::Rounding).
//!
//! # Storage
//!
//! Migrations are embedded from `migrations/sqlite/NNNN_name.sql`, exposed as
//! [`SQLITE_MIGRATIONS`], compile-checked by [`cratefield_core::assert_migration_set`],
//! and declared to the harness by [`TaxModule`].

pub mod calc;
pub mod code;
pub mod error;
pub mod module;
pub mod posting;
pub mod rate;
pub mod settings;

pub use calc::{
    Basis, CalculateParams, RoundingScope, TaxCalculation, TaxLine, TaxLineInput, calculate,
};
pub use code::{TaxCode, TaxKind, TaxSettings, VatBasis};
pub use error::TaxError;
pub use module::TaxModule;
pub use rate::{DatedRate, Jurisdiction, RateCategory, RateTable};
pub use settings::{SettingsError, set_tax_settings, tax_settings};

/// The tax settings migration, in apply order: the portable set (SQLite dialect,
/// lint-checked, runs on D1, rusqlite and — for the tables — Postgres).
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "tax_settings",
        include_str!("../migrations/sqlite/0001_tax_settings.sql"),
    )];

/// The Postgres override set: identical DDL for the tables.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "tax_settings",
        include_str!("../migrations/postgres/0001_tax_settings.sql"),
    )];

// Compile-time check on both sets: ids run contiguously from 0001 in
// the order they are listed (cratefield-core, `assert_migration_set`).
const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
