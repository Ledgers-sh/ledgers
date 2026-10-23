#![forbid(unsafe_code)]
//! Connected sources (docs/PLAN.md section 6, issue #142): vendors' invoices
//! and usage imported as draft bills, split across business units by the
//! usage each unit's resources carry.
//!
//! The crate has four parts. [`connector`] is the contract and the three
//! connectors ([`cloudflare`], [`anthropic`], [`planetscale`]). [`http`] is the
//! only way out: fixed hosts, timeouts and bounded retries. [`secret`] seals
//! the credentials per connection. [`allocation`], [`money`] and [`split`]
//! turn usage and totals into exact splits. The command surface is in
//! [`commands`], the reads in [`store`], and the tables in `migrations/`.

pub mod allocation;
pub mod anthropic;
pub mod cloudflare;
pub mod commands;
pub mod connector;
pub mod demo;
pub mod http;
pub mod model;
pub mod module;
pub mod money;
pub mod planetscale;
pub mod secret;
pub mod split;
pub mod store;

/// The connections schema migrations, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "connect",
        include_str!("../migrations/sqlite/0001_connect.sql"),
    )];

/// The Postgres set: the same portable DDL as SQLite, with the append-only
/// trigger written as a function.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "connect",
        include_str!("../migrations/postgres/0001_connect.sql"),
    )];

pub use module::ConnectModule;

const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
