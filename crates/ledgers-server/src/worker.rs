#![forbid(unsafe_code)]
//! Ledgers server: Cloudflare Workers entry point.
//!
//! Runs the Ledgers harness on the Cloudflare runtime with D1 and R2 storage.
//! One route sits in front of the harness: the one-shot deployment bootstrap
//! (`POST /admin/bootstrap`, see docs/deploy.md).

use crate::Composed;
use crate::compose;
use crate::demo::{self, SeedGate};
use crate::dispatch::{self, BootstrapError, CompanyBootstrap, Deps};
use crate::members;
use crate::people::{allowed_origin, credentialed_cors_headers};
use crate::timing;
use cratefield_core::{
    BoundedHttpClient, Clock, Config, Database, HttpClient, NoopDefer, constant_time_eq,
};
use cratefield_runtime_cloudflare::{Cloudflare, D1Database, FetchClient, WorkersClock, serve};
use serde_json::json;
use std::sync::Arc;
use worker::{
    Context, Env, Method, Request, Response, console_error, console_log, console_warn, event,
};

/// The bootstrap route. Enabled only while the `BOOTSTRAP_TOKEN` secret is set
/// and no agent key exists yet.
pub const BOOTSTRAP_PATH: &str = "/admin/bootstrap";

/// The demo seed route (issue #51). Enabled only while the `SEED_TOKEN` secret
/// is set and `LEDGERS_ALLOW_DEMO_SEED` is `"1"`; see docs/deploy.md.
pub const SEED_DEMO_PATH: &str = "/admin/seed-demo";

/// The composed Ledgers app (the harness and its runtime), built once per
/// isolate at the first request and shared by every request after it. A build
/// that fails on required configuration is kept as its problem list, so every
/// request gets the same readable 503 instead of a crashed isolate.
static APP: Composed<Result<(cratefield_core::Harness, Cloudflare), String>> = Composed::new();

/// Cloudflare Workers fetch event handler
#[event(fetch)]
pub async fn fetch(req: Request, env: Env, ctx: Context) -> worker::Result<Response> {
    let composed = APP.get_or_build(|| {
        // Create configuration from environment
        let cfg = WorkerConfig { env: env.clone() };

        // The organization module's Auth port reads the session row, so it holds
        // the same D1 binding the runtime resolves per request. Invitation mail
        // goes out over the Workers fetch.
        let Ok(binding) = env.d1("DB") else {
            return Err("the DB binding is missing".to_owned());
        };
        let clock: Arc<dyn Clock> = Arc::new(WorkersClock);
        let db: Arc<dyn Database> = Arc::new(D1Database(binding));
        let http: Arc<dyn HttpClient> = Arc::new(BoundedHttpClient::new(
            Arc::new(FetchClient),
            Arc::clone(&clock),
        ));

        // Set up the Cloudflare runtime with D1, R2, and rate limiting bindings.
        // The limiter binding (wrangler.toml) is resolved per request in serve().
        let runtime = Cloudflare::new()
            .db("DB")
            .blob("DOCUMENTS")
            .rate_limiter("RATE_LIMITER")
            .auth_arc(Arc::new(members::SessionIdentity::new(
                Arc::clone(&db),
                Arc::clone(&clock),
            )))
            .mailer(members::invitation_mailer(&cfg, http, clock));

        // Build the harness with the runtime (so it knows which ports are provided).
        // Optional features that are off are logged and degrade on their own routes.
        let registry = crate::dispatch::registry();
        match compose(&registry, &cfg, runtime.clone()) {
            Ok(composition) => {
                for warning in composition.warnings() {
                    console_warn!("{warning}");
                }
                Ok((composition.harness, runtime))
            }
            Err(err) => {
                let problems = err.problems.join("; ");
                console_error!("Ledgers is not configured: {}", problems);
                Err(problems)
            }
        }
    });
    let (harness_instance, runtime) = match composed {
        Ok((harness_instance, runtime)) => (harness_instance, runtime),
        Err(problems) => {
            return Response::error(
                format!(
                    "Ledgers is not configured: {problems}. Set the variables named here, then redeploy."
                ),
                503,
            );
        }
    };

    if req.method() == Method::Post && req.path() == BOOTSTRAP_PATH {
        return bootstrap_route(req, &env, runtime).await;
    }
    if req.method() == Method::Post && req.path() == SEED_DEMO_PATH {
        return seed_demo_route(req, &env, runtime).await;
    }

    // Credentialed CORS for the person sign-in and session calls. The harness's
    // CORS layer answers without `Allow-Credentials` and cannot be configured
    // here, so the Worker answers the preflight and marks the responses itself.
    // Only the configured app origins get the header, and only on `/v1`.
    let credentialed = req.path().starts_with("/v1/");
    let allowed = crate::app_origins(&WorkerConfig { env: env.clone() });
    let origin = origin_header_worker(&req)
        .filter(|origin| credentialed && allowed_origin(&allowed, origin).is_some());
    if let Some(origin) = origin.as_deref()
        && req.method() == Method::Options
        && req
            .headers()
            .get("access-control-request-method")?
            .is_some()
    {
        let mut response = Response::empty()?.with_status(204);
        for (name, value) in credentialed_cors_headers(origin) {
            response.headers_mut().set(name, &value)?;
        }
        response
            .headers_mut()
            .set("access-control-allow-methods", "GET, POST, OPTIONS")?;
        response.headers_mut().set(
            "access-control-allow-headers",
            "Content-Type, Authorization",
        )?;
        response
            .headers_mut()
            .set("access-control-max-age", "600")?;
        return Ok(response);
    }

    // Call the runtime's serve function to handle the request
    let started = timing::now_ms();
    let mut response = serve(harness_instance, runtime, req, env.clone(), ctx).await?;
    if let Some(origin) = origin.as_deref() {
        for (name, value) in credentialed_cors_headers(origin) {
            response.headers_mut().set(name, &value)?;
        }
    }
    if timing::enabled(&WorkerConfig { env }) {
        let total = format!("total;dur={:.2}", timing::now_ms() - started);
        // The routes add their database counters to the same header; the
        // total goes first.
        let joined = match response.headers().get("server-timing")? {
            Some(existing) => format!("{total}, {existing}"),
            None => total,
        };
        response.headers_mut().set("server-timing", &joined)?;
    }
    Ok(response)
}

/// The request's `Origin` header, if it sent one.
fn origin_header_worker(req: &Request) -> Option<String> {
    req.headers().get("origin").ok().flatten()
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

/// `POST /admin/seed-demo` with `Authorization: Bearer <SEED_TOKEN>`. Loads the
/// Halden Works demo book into the configured company, posted as the internal
/// agent and the "Demo owner" person through the executor. Idempotent by
/// `external_id`: a second run posts nothing. The gate is checked first, so a
/// refused request reads and writes nothing. The response and the log carry
/// counts only.
async fn seed_demo_route(
    req: Request,
    env: &Env,
    runtime: &Cloudflare,
) -> worker::Result<Response> {
    let secret = env.var("SEED_TOKEN").ok().map(|value| value.to_string());
    let allow = env
        .var("LEDGERS_ALLOW_DEMO_SEED")
        .ok()
        .map(|value| value.to_string());
    let presented = req
        .headers()
        .get("authorization")?
        .and_then(|value| value.strip_prefix("Bearer ").map(str::to_owned))
        .unwrap_or_default();
    let gate = demo::seed_demo_gate(secret.as_deref(), &presented, allow.as_deref());
    if let Some(status) = gate.status() {
        let message = match gate {
            SeedGate::NotFound => "Not found",
            SeedGate::Unauthorized => "Unauthorized",
            _ => "Demo seeding is not enabled in this environment",
        };
        return Response::error(message, status);
    }

    // `?demo=halden` (the default) loads Halden Works into LEDGERS_COMPANY_ID;
    // `?demo=factory-zero` loads the Factory Zero book into its own company,
    // LEDGERS_DEMO_FACTORY_ZERO_COMPANY_ID, which is created on first use.
    let demo_name = req
        .url()?
        .query_pairs()
        .find(|(key, _)| key == "demo")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| "halden".to_owned());
    let (company_var, factory_zero) = match demo_name.as_str() {
        "halden" => ("LEDGERS_COMPANY_ID", false),
        "factory-zero" => ("LEDGERS_DEMO_FACTORY_ZERO_COMPANY_ID", true),
        _ => return Response::error("demo must be halden or factory-zero", 400),
    };
    let company_id = env
        .var(company_var)
        .map(|value| value.to_string())
        .unwrap_or_default();
    if company_id.is_empty() {
        return Response::error(format!("{company_var} is not set"), 500);
    }
    let ports = runtime.ports(env, Arc::new(NoopDefer));
    let Some(deps) = Deps::from_ports(&ports) else {
        return Response::error("Storage is not bound", 503);
    };

    let year = deps.clock.now().year();
    let result = if factory_zero {
        let book = ledgers_demo::factory_zero::book(year);
        demo::seed_company_book(&deps, &company_id, &book).await
    } else {
        let book = ledgers_demo::halden::book(year);
        demo::seed_internal(&deps, &company_id, &book).await
    };
    let mut response = match result {
        Ok(report) => {
            console_log!(
                "demo seed: posted {}, reversed {}, already present {}, dimensions created {}",
                report.posted,
                report.reversed,
                report.skipped,
                report.dimensions_created
            );
            Response::from_json(&json!({
                "posted": report.posted,
                "reversed": report.reversed,
                "already_present": report.skipped,
                "dimensions_created": report.dimensions_created,
            }))?
        }
        Err(_) => {
            // The error text names an entry; the log keeps to counts.
            console_error!("demo seed failed; the book is partly loaded and a re-run continues it");
            Response::error("Demo seeding failed; see the Worker logs", 500)?
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
