#![forbid(unsafe_code)]
//! Ledgers server: Cloudflare Workers entry point.
//!
//! Runs the Ledgers harness on the Cloudflare runtime with D1 and R2 storage.

use crate::harness;
use cratefield_core::{Config, Harness};
use cratefield_runtime_cloudflare::{Cloudflare, serve};
use std::sync::OnceLock;
use worker::{Context, Env, Request, Response, event};

/// Global harness, built once at first request
static HARNESS: OnceLock<(Harness, Cloudflare)> = OnceLock::new();

/// Cloudflare Workers fetch event handler
#[event(fetch)]
pub async fn fetch(req: Request, env: Env, ctx: Context) -> worker::Result<Response> {
    let (harness_instance, runtime) = HARNESS.get_or_init(|| {
        // Create configuration from environment
        let cfg = WorkerConfig { env: env.clone() };

        // Set up the Cloudflare runtime with D1, R2, and rate limiting bindings.
        // The limiter binding (wrangler.toml) is resolved per request in serve().
        let runtime = Cloudflare::new()
            .db("DB")
            .blob("DOCUMENTS")
            .rate_limiter("RATE_LIMITER");

        // Build the harness with the runtime (so it knows which ports are provided)
        let registry = crate::dispatch::registry();
        let harness_instance =
            harness(&registry, &cfg, runtime.clone()).expect("Failed to build harness");

        (harness_instance, runtime)
    });

    // Call the runtime's serve function to handle the request
    serve(harness_instance, runtime, req, env, ctx).await
}

/// Cloudflare Workers configuration
struct WorkerConfig {
    env: Env,
}

impl Config for WorkerConfig {
    fn get(&self, key: &str) -> Option<String> {
        self.env.var(key).ok().map(|secret| secret.to_string())
    }
}
