//! HTTP routes from the command registry.
//!
//! Routes are organized by domain: each unique domain gets a harness Module
//! that mounts routes from the registry, dispatching through async Dispatch.

use cratefield_core::axum::http::header;
use cratefield_core::axum::response::IntoResponse;
use cratefield_core::axum::{
    self,
    extract::{Json, Path},
    routing::{get, post},
};
use cratefield_core::{
    Config, ConfigError, Migrations, Module, ModuleContext, Port, Problem, Scope,
};
use ledgers_commands::Registry;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Dispatches commands: async handler.
#[async_trait::async_trait]
pub trait Dispatch: Send + Sync {
    async fn dispatch(
        &self,
        command: &str,
        input: Value,
        request_id: &str,
        credential: Option<String>,
    ) -> Result<Value, Problem>;
}

/// Default Dispatch: 501 Not Implemented.
pub struct Unwired;

#[async_trait::async_trait]
impl Dispatch for Unwired {
    async fn dispatch(
        &self,
        command: &str,
        _input: Value,
        request_id: &str,
        _credential: Option<String>,
    ) -> Result<Value, Problem> {
        Err(Problem::internal()
            .with_detail(format!("command '{command}' not yet implemented"))
            .instance(request_id))
    }
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

    fn router(&self) -> axum::Router {
        let mut router = axum::Router::new();

        for (method, path_pattern, command_name) in &self.routes {
            let dispatch = Arc::clone(&self.dispatch);
            let cmd = command_name.clone();

            let axum_path = path_pattern
                .replace("{id}", ":id")
                .replace("{target}", ":target");

            match method.as_str() {
                "POST" => {
                    router = router.route(
                        &axum_path,
                        post(
                            move |scope: Scope,
                                  headers: axum::http::HeaderMap,
                                  Path(params): Path<HashMap<String, String>>,
                                  Json(mut input): Json<Value>| {
                                let dispatch = Arc::clone(&dispatch);
                                let cmd = cmd.clone();

                                async move {
                                    let request_id = scope.request_id.clone();
                                    let credential = extract_bearer(&headers);

                                    if let Value::Object(ref mut obj) = input {
                                        for (key, value) in params {
                                            obj.insert(key, json!(value));
                                        }
                                    }

                                    match dispatch
                                        .dispatch(&cmd, input, &request_id, credential)
                                        .await
                                    {
                                        Ok(body) => {
                                            (axum::http::StatusCode::OK, Json(body)).into_response()
                                        }
                                        Err(problem) => problem.into_response(),
                                    }
                                }
                            },
                        ),
                    );
                }
                "GET" => {
                    router = router.route(
                        &axum_path,
                        get(
                            move |scope: Scope,
                                  headers: axum::http::HeaderMap,
                                  Path(params): Path<HashMap<String, String>>| {
                                let dispatch = Arc::clone(&dispatch);
                                let cmd = cmd.clone();

                                async move {
                                    let request_id = scope.request_id.clone();
                                    let credential = extract_bearer(&headers);
                                    let mut input = json!({});

                                    for (key, value) in params {
                                        input[&key] = json!(value);
                                    }

                                    match dispatch.dispatch(&cmd, input, &request_id, credential).await {
                                        Ok(body) => {
                                            (axum::http::StatusCode::OK, Json(body)).into_response()
                                        }
                                        Err(problem) => problem.into_response(),
                                    }
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

impl Module for DomainModule {
    fn name(&self) -> &'static str {
        self.name
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn router(&self, _ctx: ModuleContext) -> axum::Router {
        self.router()
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
