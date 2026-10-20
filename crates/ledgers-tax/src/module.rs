//! The tax module: how the tax settings schema is declared to the harness.

use cratefield_core::axum;
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port};

/// The tax module: the `tax_settings` table for company-level configuration.
pub struct TaxModule;

impl Module for TaxModule {
    fn name(&self) -> &'static str {
        "ledgers-tax"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn tables(&self) -> &'static [&'static str] {
        &["tax_settings"]
    }

    fn migrations(&self) -> Migrations {
        Migrations {
            sqlite: crate::SQLITE_MIGRATIONS,
            postgres: crate::POSTGRES_MIGRATIONS,
        }
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        // The tax settings read no configuration keys.
        Ok(())
    }

    fn router(&self, _ctx: ModuleContext) -> axum::Router {
        // Zero routes is valid: this module exists for the schema, nothing more.
        axum::Router::new()
    }
}
