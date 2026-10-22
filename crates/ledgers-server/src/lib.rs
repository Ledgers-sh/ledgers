#![forbid(unsafe_code)]
//! Ledgers server composition: harness and modules.

pub mod config;
pub mod documents;
pub mod mcp;
pub mod openapi;
pub mod rate_limiting;
pub mod routes;

/// Cloudflare Workers entry point (wasm32 only)
#[cfg(target_arch = "wasm32")]
pub mod worker;

pub use config::LedgersConfig;
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
    rate_limiter: Option<Arc<dyn cratefield_core::RateLimiter>>,
) -> Result<Harness, ConfigError> {
    // Create the venture
    let domain = cfg
        .get("LEDGERS_DOMAIN")
        .unwrap_or_else(|| "ledgers.localhost".to_string());
    let public_url = cfg
        .get("LEDGERS_PUBLIC_URL")
        .unwrap_or_else(|| format!("http://{domain}"));

    // Default CORS origins for development
    let cors_origins = cfg.get("LEDGERS_CORS_ORIGINS").map_or_else(
        || vec![public_url.clone(), "http://localhost:3000".to_string()],
        |s| s.split(',').map(|s| s.trim().to_string()).collect(),
    );

    let venture = Venture::new("ledgers", &domain)
        .public_url(public_url.clone())
        .cors_origins(cors_origins)
        .env(VentureEnv::Development);

    // Create dispatch with rate limiting wrapper
    let unwired = Arc::new(routes::Unwired);
    // Rate limiter is optional and must be passed by the caller (native/worker runtime).
    // Rate limiter errors fail closed for agent keys: we deny the request.
    let dispatch: Arc<dyn routes::Dispatch> = Arc::new(rate_limiting::RateLimitingDispatch::new(
        unwired,
        rate_limiter,
    ));

    // Generate domain modules
    let domain_mods = routes::domain_modules(registry, &dispatch).map_err(|e| {
        let mut errors = cratefield_core::ConfigError::default();
        errors.push(e);
        errors
    })?;

    // Build harness with modules in dependency order
    let builder = Harness::builder().venture(venture).runtime(runtime);

    let builder = builder.module(ledgers_storage::StorageModule);
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
