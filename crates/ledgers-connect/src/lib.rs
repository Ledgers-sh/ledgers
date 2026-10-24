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
pub mod aws;
pub mod azure;
pub mod cloudflare;
pub mod commands;
pub mod connector;
pub mod cost;
pub mod demo;
pub mod gcp;
pub mod http;
pub mod model;
pub mod module;
pub mod money;
pub mod openai;
pub mod openrouter;
pub mod planetscale;
pub mod secret;
pub mod sigv4;
pub mod split;
pub mod store;
pub mod stripe;
pub mod supabase;
pub mod vercel;

/// The connections schema migrations, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "connect",
        include_str!("../migrations/sqlite/0001_connect.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "connect_providers",
        include_str!("../migrations/sqlite/0002_connect_providers.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0003",
        "connect_cloud",
        include_str!("../migrations/sqlite/0003_connect_cloud.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0004",
        "connect_llm",
        include_str!("../migrations/sqlite/0004_connect_llm.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0005",
        "connect_stripe",
        include_str!("../migrations/sqlite/0005_connect_stripe.sql"),
    ),
];

/// The Postgres set: the same portable DDL as SQLite, with the append-only
/// trigger written as a function.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] = &[
    cratefield_core::SqlMigration::new(
        "0001",
        "connect",
        include_str!("../migrations/postgres/0001_connect.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0002",
        "connect_providers",
        include_str!("../migrations/postgres/0002_connect_providers.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0003",
        "connect_cloud",
        include_str!("../migrations/postgres/0003_connect_cloud.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0004",
        "connect_llm",
        include_str!("../migrations/postgres/0004_connect_llm.sql"),
    ),
    cratefield_core::SqlMigration::new(
        "0005",
        "connect_stripe",
        include_str!("../migrations/postgres/0005_connect_stripe.sql"),
    ),
];

pub use module::ConnectModule;

const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
