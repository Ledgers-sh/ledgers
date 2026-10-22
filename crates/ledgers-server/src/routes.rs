//! HTTP routes from the command registry.
//!
//! Routes are organized by domain: each unique domain gets a harness Module
//! that mounts routes from the registry, dispatching through [`Dispatch`].

use crate::dispatch::{DISPATCH_PORTS, Deps};
use crate::people::presented_from;
use crate::rate_limiting::admit;
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

/// How a request presents its caller: an agent key, or a person's session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Presented {
    /// `Authorization: Bearer <agent key>`.
    Bearer(String),
    /// The person's session cookie, with what the request is checked against.
    Session(SessionPresented),
    /// A principal the server vouches for in-process, such as the internal
    /// agent the demo seed posts as. No request can present one: the HTTP
    /// adapters only build `Bearer` and `Session`.
    Principal(Principal),
}

/// A session cookie as the request presented it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPresented {
    /// The opaque session id from the cookie.
    pub id: String,
    /// The request's `Origin`, when it sent one.
    pub origin: Option<String>,
    /// Whether the method is safe (GET, HEAD, OPTIONS). State-changing methods
    /// need an allowed origin.
    pub safe: bool,
}

/// Dispatches commands: the registry's commands run against the harness
/// database, behind authentication, policy and audit.
#[async_trait::async_trait]
pub trait Dispatch: Send + Sync {
    /// Runs `command` with `input` as the caller `presented` (or anonymous).
    async fn dispatch(
        &self,
        deps: &Deps,
        command: &str,
        input: Value,
        request_id: &str,
        presented: Option<Presented>,
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
    pub(crate) fn new(
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
                                    let presented = presented_from(&headers, false);

                                    if let (Value::Object(obj), Some(Path(params))) =
                                        (&mut input, params)
                                    {
                                        for (key, value) in params {
                                            obj.insert(key, json!(value));
                                        }
                                    }

                                    if let Err(limited) =
                                        admit(dispatch.as_ref(), &deps, presented.as_ref()).await
                                    {
                                        return limited.into_response();
                                    }
                                    respond(
                                        dispatch
                                            .dispatch(&deps, &cmd, input, &request_id, presented)
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
                                    let presented = presented_from(&headers, true);

                                    // Path and query parameters are the command's input:
                                    // every value arrives as a string.
                                    let mut input = serde_json::Map::new();
                                    let path_params = params.map(|Path(p)| p).unwrap_or_default();
                                    for (key, value) in query.into_iter().chain(path_params) {
                                        input.insert(key, json!(value));
                                    }

                                    if let Err(limited) =
                                        admit(dispatch.as_ref(), &deps, presented.as_ref()).await
                                    {
                                        return limited.into_response();
                                    }
                                    respond(
                                        dispatch
                                            .dispatch(
                                                &deps,
                                                &cmd,
                                                Value::Object(input),
                                                &request_id,
                                                presented,
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
        Err(problem) => problem.into_response(),
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
        DISPATCH_PORTS
    }

    fn optional(&self) -> &'static [Port] {
        // The agent-key limiter, when the runtime provides one.
        &[Port::RateLimiter]
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

/// The command routes of one registry domain (`documents` for
/// `/v1/documents/...`): `(method, relative path, command name)`. A module
/// that owns the domain mounts these itself, next to its own routes.
#[must_use]
pub fn domain_routes(registry: &Registry, domain: &str) -> Vec<(String, String, String)> {
    registry
        .iter()
        .filter_map(|descriptor| {
            let after_v1 = descriptor.http_path.strip_prefix("/v1/")?;
            let first = after_v1.split('/').next().unwrap_or("");
            (first == domain).then(|| {
                let relative = after_v1.strip_prefix(&format!("{domain}/")).unwrap_or("");
                (
                    descriptor.http_method.clone(),
                    format!("/{relative}"),
                    descriptor.name.clone(),
                )
            })
        })
        .collect()
}

/// Domains whose routes a module of the same name mounts itself, with the
/// command routes of the domain (see [`domain_routes`]).
const MODULE_OWNED: &[&str] = &["documents"];

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
    domains.retain(|domain, _| !MODULE_OWNED.contains(&domain.as_str()));

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
