//! The `PolicyModule`: how the `agent_key` schema is declared to the harness.

use cratefield_core::axum;
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port};

/// The `Policy` module: agent keys, policy enforcement, and the audit trail.
pub struct PolicyModule;

impl Module for PolicyModule {
    fn name(&self) -> &'static str {
        "ledgers-policy"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn tables(&self) -> &'static [&'static str] {
        &["agent_key", "policy_change"]
    }

    fn migrations(&self) -> Migrations {
        Migrations {
            sqlite: crate::SQLITE_MIGRATIONS,
            postgres: crate::POSTGRES_MIGRATIONS,
        }
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        // The Policy module reads no configuration keys.
        Ok(())
    }

    fn router(&self, _ctx: ModuleContext) -> axum::Router {
        // Zero routes is valid: this module exists for the schema, nothing more.
        axum::Router::new()
    }
}
