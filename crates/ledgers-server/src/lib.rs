#![forbid(unsafe_code)]
//! Ledgers server composition: harness and modules.

pub mod config;
pub mod dispatch;
pub mod documents;
pub mod mcp;
/// The native (SQLite) composition `ledgers-server` and `ledgers serve` share.
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
pub mod openapi;
pub mod routes;

/// Cloudflare Workers entry point (wasm32 only)
#[cfg(target_arch = "wasm32")]
pub mod worker;

pub use config::LedgersConfig;
pub use dispatch::{Deps, ServerDispatch};
pub use mcp::McpModule;
pub use routes::Dispatch;

use cratefield_core::{Config, ConfigError, Harness, Runtime, Venture, VentureEnv};
use ledgers_commands::Registry;
use std::sync::Arc;

/// Builds the Ledgers Harness with all modules.
///
/// # Arguments
///
/// * `registry` - The command registry
/// * `cfg` - Configuration provider (reads environment variables)
/// * `runtime` - The runtime (native, cloudflare, etc.) that provides ports
///
/// # Errors
///
/// Returns `ConfigError` if configuration is invalid.
pub fn harness(
    registry: &Registry,
    cfg: &dyn Config,
    runtime: impl Runtime + 'static,
) -> Result<Harness, ConfigError> {
    // Create the venture
    let domain = cfg
        .get("LEDGERS_DOMAIN")
        .unwrap_or_else(|| "ledgers.localhost".to_string());
    let public_url = cfg
        .get("LEDGERS_PUBLIC_URL")
        .unwrap_or_else(|| format!("http://{domain}"));

    // LEDGERS_DEV=1 is the local development mode: a local web app on Vite's
    // default port may call the API, and the dev bootstrap runs at start.
    let dev = is_dev(cfg);
    let mut default_origins = vec![public_url.clone(), "http://localhost:3000".to_string()];
    if dev {
        default_origins.push(DEV_WEB_ORIGIN.to_string());
    }
    let cors_origins = cfg.get("LEDGERS_CORS_ORIGINS").map_or_else(
        || default_origins,
        |s| s.split(',').map(|s| s.trim().to_string()).collect(),
    );

    let company_id = match cfg.get("LEDGERS_COMPANY_ID") {
        Some(id) => id,
        None if dev => dispatch::DEV_COMPANY_ID.to_string(),
        None => {
            let mut errors = ConfigError::default();
            errors.push("LEDGERS_COMPANY_ID is required unless LEDGERS_DEV=1".to_string());
            return Err(errors);
        }
    };

    let venture = Venture::new("ledgers", &domain)
        .public_url(public_url.clone())
        .cors_origins(cors_origins)
        .env(VentureEnv::Development);

    // Create dispatch
    let dispatch: Arc<dyn routes::Dispatch> = Arc::new(dispatch::ServerDispatch::new(company_id));

    // Generate domain modules
    let domain_mods = routes::domain_modules(registry, &dispatch).map_err(|e| {
        let mut errors = cratefield_core::ConfigError::default();
        errors.push(e);
        errors
    })?;

    // Build harness with modules in dependency order
    let builder = Harness::builder().venture(venture).runtime(runtime);

    let builder = builder.module(ledgers_storage::StorageModule);
    let builder = builder.module(dispatch::ApiKeysModule);
    let builder = builder.module(ledgers_db::module::DbModule);
    let builder = builder.module(ledgers_ledger::module::LedgerModule);
    let builder = builder.module(ledgers_policy::module::PolicyModule);
    let builder = builder.module(ledgers_approvals::module::ApprovalsModule);
    let builder = builder.module(ledgers_fx::FxModule);
    let builder = builder.module(ledgers_tax::module::TaxModule);

    let mut builder = builder;
    for module in domain_mods {
        builder = builder.module_arc(module);
    }

    // Documents module - requires Blob and IdGen ports
    let docs_module = documents::DocumentsModule::from_config(cfg).map_err(|e| {
        let mut errors = cratefield_core::ConfigError::default();
        errors.push(e);
        errors
    })?;
    let builder = builder.module(docs_module);

    // Add OpenAPI module
    let openapi_module = openapi::OpenApiModule::new(registry);
    let builder = builder.module(openapi_module);

    // Add MCP module
    let mcp_module = mcp::McpModule::new(Arc::new(registry.clone()), dispatch.clone(), public_url);
    let builder = builder.module(mcp_module);

    builder.build()
}

/// The web app origin the development mode allows (Vite's default port).
pub const DEV_WEB_ORIGIN: &str = "http://localhost:5173";

/// Whether the server runs in local development mode (`LEDGERS_DEV=1`).
/// Development mode alone enables the dev bootstrap and the dev CORS origin;
/// nothing else is relaxed by it.
#[must_use]
pub fn is_dev(cfg: &dyn Config) -> bool {
    cfg.get("LEDGERS_DEV").as_deref() == Some("1")
}
