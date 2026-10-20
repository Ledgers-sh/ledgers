//! The FX module: how the `exchange_rate` schema is declared to the harness.

use cratefield_core::axum;
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port};

/// The FX module: the `exchange_rate` table for foreign currency rates.
pub struct FxModule;

impl Module for FxModule {
    fn name(&self) -> &'static str {
        "ledgers-fx"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn tables(&self) -> &'static [&'static str] {
        &["exchange_rate"]
    }

    fn migrations(&self) -> Migrations {
        Migrations {
            sqlite: crate::SQLITE_MIGRATIONS,
            postgres: crate::POSTGRES_MIGRATIONS,
        }
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        // The FX module reads no configuration keys.
        Ok(())
    }

    fn router(&self, _ctx: ModuleContext) -> axum::Router {
        // Zero routes is valid: this module exists for the schema, nothing more.
        axum::Router::new()
    }
}
