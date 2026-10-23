//! The group schema, declared to the harness. It serves no routes: the
//! commands are reached through the registry and the server's dispatch.

use cratefield_core::axum;
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port};

/// The group module: the group, member, agreement and period tables.
pub struct GroupModule;

impl Module for GroupModule {
    fn name(&self) -> &'static str {
        "ledgers-group"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn tables(&self) -> &'static [&'static str] {
        &[
            "company_group",
            "company_group_member",
            "intercompany_agreement",
            "intercompany_document",
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
