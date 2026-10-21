//! The `ApprovalsModule`: database schema and migrations.

use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port, axum};

/// The approvals module providing approval request and nonce tables.
pub struct ApprovalsModule;

impl Module for ApprovalsModule {
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn name(&self) -> &'static str {
        "approvals"
    }

    fn requires(&self) -> &'static [Port] {
        // No special ports required beyond the standard Db port
        &[]
    }

    fn validate_config(&self, _config: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }

    fn router(&self, _ctx: ModuleContext) -> axum::Router {
        // No HTTP routes in this module (yet)
        axum::Router::new()
    }

    fn migrations(&self) -> Migrations {
        Migrations {
            sqlite: super::SQLITE_MIGRATIONS,
            postgres: super::POSTGRES_MIGRATIONS,
        }
    }
}
