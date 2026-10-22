#![forbid(unsafe_code)]
//! Ledgers server composition: harness and modules.

pub mod agent_keys;
pub mod config;
pub mod demo;
pub mod dispatch;
pub mod documents;
pub mod mcp;
/// The native (SQLite) composition `ledgers-server` and `ledgers serve` share.
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
pub mod openapi;
pub mod people;
pub mod rate_limiting;
pub mod routes;
pub mod timing;

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
    let cors_origins = app_origins(cfg);
    // Cookie-authenticated (person) calls may come from these origins only.
    let people_config =
        people::PeopleConfig::from_config(cfg, &public_url, cors_origins.clone(), dev).map_err(
            |e| {
                let mut errors = ConfigError::default();
                errors.push(e);
                errors
            },
        )?;

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
        .cors_origins(cors_origins.clone())
        .env(VentureEnv::Development);

    // Every command surface (HTTP routes, MCP tool calls) admits agent keys
    // through the rate limiter before dispatch (`rate_limiting::admit`). The
    // limiter comes from the request's ports (`Deps`), so it is resolved per
    // request on Workers as well as natively; persons pass through.
    let document_key = document_signing_key(cfg, dev).map_err(|e| {
        let mut errors = ConfigError::default();
        errors.push(e);
        errors
    })?;
    let dispatch: Arc<dyn routes::Dispatch> = Arc::new(
        dispatch::ServerDispatch::new(company_id.clone())
            .with_app_origins(cors_origins.clone())
            .with_app_url(
                cfg.get("LEDGERS_APP_URL")
                    .unwrap_or_else(|| public_url.clone()),
            )
            .with_document_key(document_key.clone())
            .with_extract_key(cfg.get("LEDGERS_EXTRACT_API_KEY")),
    );

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

    // Documents module - the upload and download routes (Blob and IdGen ports).
    let docs_module = documents::DocumentsModule::new(
        company_id,
        document_key,
        dispatch.clone(),
        routes::domain_routes(registry, "documents"),
    );
    let builder = builder.module(docs_module);

    // Add OpenAPI module
    let openapi_module = openapi::OpenApiModule::new(registry);
    let builder = builder.module(openapi_module);

    // Add MCP module
    let mcp_module = mcp::McpModule::new(Arc::new(registry.clone()), dispatch.clone(), public_url);
    let builder = builder.module(mcp_module);

    // Last, so its migration follows every existing module's: the D1 files
    // are numbered in this order and staging has already applied them.
    let builder = builder.module(people::PeopleModule::new(people_config));
    // The approval inbox columns come after every other module's migrations.
    let builder = builder.module(ledgers_approvals::module::ApprovalsInboxModule);
    // The notification outbox comes last of all, after the inbox.
    let builder = builder.module(ledgers_approvals::module::ApprovalsNotifyModule);
    // Source documents and vendor bills come last: their migrations follow every
    // module above, the same way the D1 files are numbered.
    let builder = builder.module(ledgers_documents::DocumentsStoreModule);
    let builder = builder.module(ledgers_bills::BillsModule);

    builder.build()
}

/// A value built once per isolate and shared by every request after that.
///
/// The Worker entry point keeps the composed Ledgers harness in one of these,
/// so a request never composes it again. `builds` counts how often the value
/// was made (1 after any number of requests), which the tests check.
pub struct Composed<T> {
    cell: std::sync::OnceLock<T>,
    builds: std::sync::atomic::AtomicUsize,
}

impl<T> Default for Composed<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Composed<T> {
    /// An empty cell; nothing is built until the first [`Composed::get_or_build`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cell: std::sync::OnceLock::new(),
            builds: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The shared value, building it with `build` on the first call only.
    pub fn get_or_build(&self, build: impl FnOnce() -> T) -> &T {
        self.cell.get_or_init(|| {
            self.builds
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            build()
        })
    }

    /// How many times the value has been built: 0 or 1.
    #[must_use]
    pub fn builds(&self) -> usize {
        self.builds.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The origins the web app is served from, which the API allows for CORS and
/// for cookie-authenticated calls: `LEDGERS_CORS_ORIGINS` (comma-separated), or
/// the API's own origin, the local web origin and, in development, Vite's.
#[must_use]
pub fn app_origins(cfg: &dyn Config) -> Vec<String> {
    let domain = cfg
        .get("LEDGERS_DOMAIN")
        .unwrap_or_else(|| "ledgers.localhost".to_string());
    let public_url = cfg
        .get("LEDGERS_PUBLIC_URL")
        .unwrap_or_else(|| format!("http://{domain}"));
    let mut default_origins = vec![public_url, "http://localhost:3000".to_string()];
    if is_dev(cfg) {
        default_origins.push(DEV_WEB_ORIGIN.to_string());
    }
    cfg.get("LEDGERS_CORS_ORIGINS").map_or_else(
        || default_origins,
        |s| s.split(',').map(|s| s.trim().to_string()).collect(),
    )
}

/// The key that signs document download links. Required outside development
/// (`LEDGERS_DOCUMENT_SIGNING_KEY`, at least 32 characters); development uses a
/// fixed development key so local links work without setup.
///
/// # Errors
/// A message when the key is missing outside development or too short.
pub fn document_signing_key(cfg: &dyn Config, dev: bool) -> Result<Vec<u8>, String> {
    match cfg.get("LEDGERS_DOCUMENT_SIGNING_KEY") {
        Some(key) if key.len() >= 32 => Ok(key.into_bytes()),
        Some(_) => Err("LEDGERS_DOCUMENT_SIGNING_KEY must be at least 32 characters".into()),
        None if dev => Ok(DEV_DOCUMENT_KEY.as_bytes().to_vec()),
        None => Err("LEDGERS_DOCUMENT_SIGNING_KEY is required unless LEDGERS_DEV=1".into()),
    }
}

/// The development download-link key. Never used outside `LEDGERS_DEV=1`.
pub const DEV_DOCUMENT_KEY: &str = "ledgers-dev-document-signing-key-do-not-use";

/// The web app origin the development mode allows (Vite's default port).
pub const DEV_WEB_ORIGIN: &str = "http://localhost:5173";

/// Whether the server runs in local development mode (`LEDGERS_DEV=1`).
/// Development mode alone enables the dev bootstrap and the dev CORS origin;
/// nothing else is relaxed by it.
#[must_use]
pub fn is_dev(cfg: &dyn Config) -> bool {
    cfg.get("LEDGERS_DEV").as_deref() == Some("1")
}
