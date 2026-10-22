//! The bills schema, declared to the harness. It serves no routes:
//! the upload and download routes live in the server, and the commands are
//! reached through the registry.

use cratefield_core::axum;
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port};

/// The bills module: the `vendor` and `bill` tables.
pub struct BillsModule;

impl Module for BillsModule {
    fn name(&self) -> &'static str {
        "ledgers-bills"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn tables(&self) -> &'static [&'static str] {
        &["vendor", "bill"]
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
