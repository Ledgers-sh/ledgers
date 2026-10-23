//! The connections schema, declared to the harness. It serves no routes: the
//! commands are reached through the registry and the server does the vendor
//! calls (`ledgers-server`, `dispatch/connect.rs`).

use cratefield_core::axum;
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port};

/// The connections module: connections, credentials, runs, invoices, usage,
/// splits and allocation rules.
pub struct ConnectModule;

impl Module for ConnectModule {
    fn name(&self) -> &'static str {
        "ledgers-connect"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn tables(&self) -> &'static [&'static str] {
        &[
            "connector_connection",
            "connector_credential",
            "connector_run",
            "connector_invoice",
            "connector_usage",
            "connector_split",
            "allocation_rule",
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
