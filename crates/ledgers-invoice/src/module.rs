//! Module wiring for ledgers-invoice commands and storage.

use cratefield_core::{Migrations, Module, Port};

/// The invoice module: migrations and commands.
pub struct InvoiceModule;

impl Module for InvoiceModule {
    fn name(&self) -> &'static str {
        "ledgers-invoice"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn tables(&self) -> &'static [&'static str] {
        &["invoice", "invoice_line", "credit_note", "credit_note_line"]
    }

    fn migrations(&self) -> Migrations {
        Migrations {
            sqlite: crate::SQLITE_MIGRATIONS,
            postgres: crate::POSTGRES_MIGRATIONS,
        }
    }

    fn validate_config(
        &self,
        _config: &dyn cratefield_core::Config,
    ) -> Result<(), cratefield_core::ConfigError> {
        Ok(())
    }

    fn router(&self, _ctx: cratefield_core::ModuleContext) -> cratefield_core::axum::Router {
        cratefield_core::axum::Router::new()
    }
}
