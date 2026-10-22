//! The bank schema, declared to the harness. It serves no routes: the
//! statement upload is the server's, and every command is reached through the
//! registry.

use cratefield_core::axum;
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port};

/// The bank module: accounts, statements, lines and rules.
pub struct BankModule;

impl Module for BankModule {
    fn name(&self) -> &'static str {
        "ledgers-bank"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn tables(&self) -> &'static [&'static str] {
        &[
            "bank_account",
            "bank_statement",
            "bank_transaction",
            "bank_rule",
        ]
    }

    fn migrations(&self) -> Migrations {
        Migrations {
            sqlite: crate::SQLITE_MIGRATIONS,
            postgres: crate::POSTGRES_MIGRATIONS,
        }
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }

    fn router(&self, _ctx: ModuleContext) -> axum::Router {
        axum::Router::new()
    }
}
