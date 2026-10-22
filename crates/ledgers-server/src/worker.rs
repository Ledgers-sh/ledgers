#![forbid(unsafe_code)]
//! Ledgers server: Cloudflare Workers entry point.
//!
//! Runs the Ledgers harness on the Cloudflare runtime with D1 and R2 storage.
//! One route sits in front of the harness: the one-shot deployment bootstrap
//! (`POST /admin/bootstrap`, see docs/deploy.md).

use crate::dispatch::{self, BootstrapError, CompanyBootstrap, Deps};
use crate::harness;
use cratefield_core::{Config, NoopDefer, constant_time_eq};
use cratefield_runtime_cloudflare::{Cloudflare, serve};
use serde_json::json;
use std::sync::{Arc, OnceLock};
use worker::{Context, Env, Method, Request, Response, console_error, event};

/// The bootstrap route. Enabled only while the `BOOTSTRAP_TOKEN` secret is set
/// and no agent key exists yet.
pub const BOOTSTRAP_PATH: &str = "/admin/bootstrap";

/// Global harness, built once at first request
static HARNESS: OnceLock<(cratefield_core::Harness, Cloudflare)> = OnceLock::new();

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

    if req.method() == Method::Post && req.path() == BOOTSTRAP_PATH {
        return bootstrap_route(req, &env, runtime).await;
    }

    // Call the runtime's serve function to handle the request
    serve(harness_instance, runtime, req, env, ctx).await
}

/// `POST /admin/bootstrap` with `Authorization: Bearer <BOOTSTRAP_TOKEN>` and a
/// JSON body `{"company": "...", "currency": "EUR"}`. Creates the company and
/// returns one full-tier key, once. The response is the only place the key
/// appears; nothing here logs it.
async fn bootstrap_route(
    mut req: Request,
    env: &Env,
    runtime: &Cloudflare,
) -> worker::Result<Response> {
    let expected = match env.var("BOOTSTRAP_TOKEN") {
        Ok(token) if !token.to_string().is_empty() => token.to_string(),
        _ => return Response::error("Not found", 404),
    };
    let presented = req
        .headers()
        .get("authorization")?
        .and_then(|value| value.strip_prefix("Bearer ").map(str::to_owned))
        .unwrap_or_default();
    if !constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
        return Response::error("Unauthorized", 401);
    }

    let input: CompanyBootstrap = match req.json().await {
        Ok(input) => input,
        Err(_) => {
            return Response::error(
                "Send JSON: {\"company\": \"Demo GmbH\", \"currency\": \"EUR\"}",
                400,
            );
        }
    };
    let company_id = env
        .var("LEDGERS_COMPANY_ID")
        .map(|value| value.to_string())
        .unwrap_or_default();
    let ports = runtime.ports(env, Arc::new(NoopDefer));
    let Some(deps) = Deps::from_ports(&ports) else {
        return Response::error("Storage is not bound", 503);
    };

    let mut response = match dispatch::bootstrap_company(&deps, &company_id, &input).await {
        Ok(key) => Response::from_json(&json!({
            "company_id": company_id,
            "company": input.company.trim(),
            "currency": input.currency,
            "api_key": key,
        }))?
        .with_status(201),
        Err(BootstrapError::Invalid(message)) => Response::error(message, 400)?,
        Err(BootstrapError::AlreadyBootstrapped) => Response::error(
            "This environment is already bootstrapped; no key was issued",
            409,
        )?,
        Err(BootstrapError::Failed(step)) => {
            console_error!("bootstrap failed: {}", step);
            Response::error("Bootstrap failed; see the Worker logs", 500)?
        }
    };
    response.headers_mut().set("cache-control", "no-store")?;
    Ok(response)
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
