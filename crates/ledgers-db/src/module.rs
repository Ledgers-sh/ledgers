//! The ledgers-db module: how the guard-statement schema is declared to
//! the harness. It serves no routes — the writing crates building on
//! these tables own their routers — but it carries the migrations and
//! the table declarations, so a venture composition (and `fz doctor`)
//! sees the guard foundation with nothing hidden behind a domain crate.

use cratefield_core::axum;
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port};

/// The guard-statement module: the five small tables every write-time
/// invariant reads or writes. Unit struct — the module is its schema,
/// nothing more; there is no per-instance state to configure.
pub struct DbModule;

impl Module for DbModule {
    fn name(&self) -> &'static str {
        "ledgers-db"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    // Migrations are applied through the `Database` port by the runtime,
    // not through a port a handler holds, so the module requires nothing.
    fn requires(&self) -> &'static [Port] {
        &[]
    }

    // The tables this module owns; duplicates across modules are a build
    // error, and doctor cross-checks the created tables against these
    // declarations, so the list cannot drift from the migrations.
    fn tables(&self) -> &'static [&'static str] {
        &[
            "numbering_counter",
            "ledger_guard",
            "accounting_period",
            "audit_event",
            "agent_usage",
            "outbox",
        ]
    }

    fn migrations(&self) -> Migrations {
        // The portable set: the SQLite dialect runs on D1, rusqlite and
        // (lint-checked) Postgres. A postgres override set is added here
        // only where the SQL truly differs.
        Migrations::sqlite(super::SQLITE_MIGRATIONS)
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        // The guard foundation reads no configuration keys.
        Ok(())
    }

    // Zero routes is valid (cratefield-cli's doctor dummies do the same):
    // this module exists for the schema, and its router is the empty one
    // until a domain crate takes the surface over.
    fn router(&self, _ctx: ModuleContext) -> axum::Router {
        axum::Router::new()
    }
}
