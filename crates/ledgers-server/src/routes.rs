//! HTTP routes from the command registry.
//!
//! Routes are organized by domain: each unique domain gets a harness Module
//! that mounts routes from the registry, dispatching through [`Dispatch`].

use crate::dispatch::{DISPATCH_PORTS, Deps};
use cratefield_core::axum::http::header;
use cratefield_core::axum::response::IntoResponse;
use cratefield_core::axum::{
    self,
    extract::{Json, Path, Query},
    routing::{get, post},
};
use cratefield_core::{
    Config, ConfigError, Migrations, Module, ModuleContext, Port, Problem, Scope,
};
use ledgers_commands::{Principal, Registry};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Dispatches commands: the registry's commands run against the harness
/// database, behind authentication, policy and audit.
#[async_trait::async_trait]
pub trait Dispatch: Send + Sync {
    /// Runs `command` with `input` for the principal behind `credential`.
    async fn dispatch(
        &self,
        deps: &Deps,
        command: &str,
        input: Value,
        request_id: &str,
        credential: Option<String>,
    ) -> Result<Value, Problem>;

    /// Resolves the principal behind a bearer credential, for surfaces that
    /// need it before dispatching (the MCP tool list and scope).
    async fn principal(&self, deps: &Deps, credential: &str) -> Result<Principal, Problem>;
}

/// A domain module from the registry.
pub struct DomainModule {
    name: &'static str,
    dispatch: Arc<dyn Dispatch>,
    routes: Vec<(String, String, String)>, // (method, path_pattern, command_name)
}

impl DomainModule {
    fn new(
        name: &'static str,
        dispatch: Arc<dyn Dispatch>,
        routes: Vec<(String, String, String)>,
    ) -> Self {
        Self {
            name,
            dispatch,
            routes,
        }
    }

    fn mount(&self, deps: &Arc<Deps>) -> axum::Router {
        let mut router = axum::Router::new();

        for (method, path_pattern, command_name) in &self.routes {
            let dispatch = Arc::clone(&self.dispatch);
            let cmd = command_name.clone();
            let deps = Arc::clone(deps);

            match method.as_str() {
                "POST" => {
                    router = router.route(
                        path_pattern,
                        post(
                            move |scope: Scope,
                                  headers: axum::http::HeaderMap,
                                  params: Option<Path<HashMap<String, String>>>,
                                  Json(mut input): Json<Value>| {
                                let dispatch = Arc::clone(&dispatch);
                                let deps = Arc::clone(&deps);
                                let cmd = cmd.clone();

                                async move {
                                    let request_id = scope.request_id.clone();
                                    let credential = extract_bearer(&headers);

                                    if let (Value::Object(obj), Some(Path(params))) =
                                        (&mut input, params)
                                    {
                                        for (key, value) in params {
                                            obj.insert(key, json!(value));
                                        }
                                    }

                                    respond(
                                        dispatch
                                            .dispatch(&deps, &cmd, input, &request_id, credential)
                                            .await,
                                    )
                                }
                            },
                        ),
                    );
                }
                "GET" => {
                    router = router.route(
                        path_pattern,
                        get(
                            move |scope: Scope,
                                  headers: axum::http::HeaderMap,
                                  params: Option<Path<HashMap<String, String>>>,
                                  Query(query): Query<HashMap<String, String>>| {
                                let dispatch = Arc::clone(&dispatch);
                                let deps = Arc::clone(&deps);
                                let cmd = cmd.clone();

                                async move {
                                    let request_id = scope.request_id.clone();
                                    let credential = extract_bearer(&headers);

                                    // Path and query parameters are the command's input:
                                    // every value arrives as a string.
                                    let mut input = serde_json::Map::new();
                                    let path_params = params.map(|Path(p)| p).unwrap_or_default();
                                    for (key, value) in query.into_iter().chain(path_params) {
                                        input.insert(key, json!(value));
                                    }

                                    respond(
                                        dispatch
                                            .dispatch(
                                                &deps,
                                                &cmd,
                                                Value::Object(input),
                                                &request_id,
                                                credential,
                                            )
                                            .await,
                                    )
                                }
                            },
                        ),
                    );
                }
                _ => {}
            }
        }

        router
    }
}

/// Maps a dispatch result to an HTTP response: 200 with the JSON body, or
/// the problem document.
fn respond(result: Result<Value, Problem>) -> axum::response::Response {
    match result {
        Ok(body) => (axum::http::StatusCode::OK, Json(body)).into_response(),
        Err(problem) => problem_to_response(problem),
    }
}

/// Convert a problem to a response, adding Retry-After for rate-limited errors.
fn problem_to_response(problem: Problem) -> axum::response::Response {
    let mut response = problem.into_response();
    // Add Retry-After for rate-limited responses (429 status).
    if response.status() == axum::http::StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, "60".parse().unwrap());
    }
    response
}

impl Module for DomainModule {
    fn name(&self) -> &'static str {
        self.name
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        DISPATCH_PORTS
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        let deps = Deps::from_ports(&ctx.ports)
            .expect("the dispatch ports (db, clock, id_gen) are declared in requires()");
        self.mount(&Arc::new(deps))
    }

    fn migrations(&self) -> Migrations {
        Migrations::default()
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }
}

fn extract_bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(ToString::to_string))
}

/// Generate domain modules from the registry.
///
/// # Errors
///
/// Returns an error if a domain collides with a reserved module name.
pub fn domain_modules(
    registry: &Registry,
    dispatch: &Arc<dyn Dispatch>,
) -> Result<Vec<Arc<dyn Module>>, String> {
    let reserved = [
        "storage",
        "ledgers-policy",
        "db",
        "ledger",
        "approvals",
        "auth",
        "fx",
        "tax",
        "documents",
        "openapi",
    ];

    let mut domains: HashMap<String, Vec<(String, String, String)>> = HashMap::new();

    for descriptor in registry.iter() {
        let path = &descriptor.http_path;
        if let Some(after_v1) = path.strip_prefix("/v1/")
            && let Some(first_segment) = after_v1.split('/').next()
        {
            let domain = first_segment.to_string();
            let relative_path = format!(
                "/{}",
                after_v1.strip_prefix(&format!("{domain}/")).unwrap_or("")
            );

            domains.entry(domain).or_default().push((
                descriptor.http_method.clone(),
                relative_path,
                descriptor.name.clone(),
            ));
        }
    }

    for domain in domains.keys() {
        if reserved.contains(&domain.as_str()) {
            return Err(format!(
                "domain '{domain}' collides with reserved module name"
            ));
        }
    }

    let mut sorted_domains: Vec<_> = domains.into_iter().collect();
    sorted_domains.sort_by_key(|(name, _)| name.clone());

    let mut modules: Vec<Arc<dyn Module>> = Vec::new();
    for (domain, routes) in sorted_domains {
        let module =
            DomainModule::new(Box::leak(domain.into_boxed_str()), dispatch.clone(), routes);
        modules.push(Arc::new(module));
    }

    Ok(modules)
}
