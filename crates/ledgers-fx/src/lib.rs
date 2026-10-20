#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
//! Exchange rates for foreign currency transactions — docs/PLAN.md §6 milestone M0.
//!
//! # Exchange rate handling
//!
//! This crate provides:
//!
//! - [`lookup`]: Rate lookup by date within a configurable window (default 7 days).
//!   Same-day rates take precedence; if not found, the most recent earlier rate
//!   within the window is used; if no rate exists, a clear error is returned.
//! - [`RateSource`](lookup::RateSource): Manual, CSV, or ECB sources.
//! - [`ExchangeRate`](lookup::ExchangeRate): One rate record with date, currencies, rate, source.
//! - [`record`]: Write rates to the database (all-or-nothing batches).
//! - [`csv`]: Strict CSV import (header `date,base,quote,rate`, all-or-nothing on errors).
//! - [`ecb`]: ECB reference rates feed parser (behind `ecb` feature; fetch is caller's responsibility).
//!
//! # Money and precision
//!
//! All amounts are [`Amount`](ledgers_money::Amount) — integer minor units, never float.
//! Exchange rates use exact [`Rate`](ledgers_money::Rate) (i128 scaled by 10¹², ADR 0003).
//! Conversions use [`mul_rate`](ledgers_money::Amount::mul_rate) with half-even rounding.
//!
//! # Immutability and audit
//!
//! Posted entries carry their own rate copy in `journal_line.rate_scaled`, so later
//! rate edits (upserts) never change posted amounts. Manual entries are audited in
//! the append-only chain. Rates are reference data, never posted entries themselves.
//!
//! # Storage
//!
//! Migrations are embedded from `migrations/sqlite/NNNN_name.sql` and
//! `migrations/postgres/NNNN_name.sql`, exposed as [`SQLITE_MIGRATIONS`] and
//! [`POSTGRES_MIGRATIONS`], compile-checked by [`cratefield_core::assert_migration_set`],
//! and declared to the harness by [`FxModule`].

pub mod csv;
pub mod ecb;
pub mod error;
pub mod lookup;
pub mod module;
pub mod record;

pub use error::FxError;
pub use lookup::{ExchangeRate, LookupWindow, RateSource, lookup};
pub use module::FxModule;
pub use record::{ManualRateParams, RateToRecord, record_manual_rate, record_rates};

/// The exchange rates migration, in apply order: the portable set (SQLite dialect,
/// lint-checked, runs on D1, rusqlite and — for the tables — Postgres).
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "exchange_rate",
        include_str!("../migrations/sqlite/0001_exchange_rate.sql"),
    )];

/// The Postgres override set: identical DDL for the tables.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "exchange_rate",
        include_str!("../migrations/postgres/0001_exchange_rate.sql"),
    )];

// Compile-time check on both sets: ids run contiguously from 0001 in
// the order they are listed (cratefield-core, `assert_migration_set`).
const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
