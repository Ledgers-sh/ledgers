//! HTTP routes from the command registry.
//!
//! Routes are organized by domain: each unique domain gets a harness Module
//! that mounts routes from the registry, dispatching through [`Dispatch`].

use crate::dispatch::{DISPATCH_PORTS, Deps};
use crate::people::presented_from;
use crate::query_input::get_input;
use crate::rate_limiting::admit;
use crate::timing::{self, CountingDb, DbTiming};
use cratefield_core::axum::response::IntoResponse;
use cratefield_core::axum::{
    self,
    extract::{Json, Path, Query},
    routing::{get, post},
};
use cratefield_core::{
    Config, ConfigError, Migrations, Module, ModuleContext, Port, Problem, ProblemDef, Scope,
};
use ledgers_commands::{Principal, Registry};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
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

    /// The agent key id a bearer credential verifies to: the rate-limit bucket
    /// admission charges. Admission needs no more than this, so it reads no
    /// policy row. `None` for a credential that does not verify.
    async fn verified_key_id(&self, deps: &Deps, credential: &str) -> Option<String> {
        match self.principal(deps, credential).await.ok()? {
            Principal::Agent { key_id, .. } => Some(key_id),
            Principal::Person { .. } => None,
        }
    }

    /// This dispatch serving `company_id` instead. A request names its company
    /// with the `X-Ledgers-Company` header (docs/permissions.md, "Companies"); the membership
    /// or agent key check then runs against that company, so a person or key
    /// of another company is refused there.
    fn scoped(self: Arc<Self>, company_id: &str) -> Arc<dyn Dispatch>;
}

/// The header that names the company a request acts for.
pub const COMPANY_HEADER: &str = "x-ledgers-company";

/// The dispatch a request runs on: the company its `X-Ledgers-Company` header
/// names, or `base` (the deployment's company) when the header is absent.
///
/// # Errors
/// A 400 problem when the header is not a company id (ASCII letters and digits,
/// at most 64 characters), so an unusable value never reaches a query.
pub fn company_dispatch(
    base: &Arc<dyn Dispatch>,
    headers: &axum::http::HeaderMap,
) -> Result<Arc<dyn Dispatch>, Problem> {
    let Some(value) = headers.get(COMPANY_HEADER) else {
        return Ok(Arc::clone(base));
    };
    let company = value
        .to_str()
        .ok()
        .filter(|id| {
            !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric())
        })
        .ok_or_else(|| Problem::new(&INVALID_COMPANY))?;
    Ok(Arc::clone(base).scoped(company))
}

/// 400: the `X-Ledgers-Company` header is not a company id.
static INVALID_COMPANY: ProblemDef = ProblemDef {
    slug: "invalid-company",
    status: axum::http::StatusCode::BAD_REQUEST,
    title: "Invalid company",
    description: "The X-Ledgers-Company header must be a company id: letters and digits, at most 64 characters.",
};

/// One command route of a domain: the HTTP method, the path relative to the
/// domain, the command, and the command's input schema (which types the
/// parameters of a GET route).
#[derive(Clone, Debug)]
pub struct CommandRoute {
    /// `GET` or `POST`.
    pub method: String,
    /// The path under the domain, such as `/{number}` or `/`.
    pub path: String,
    /// The command's name, such as `accounts.get`.
    pub command: String,
    /// The command's input JSON schema.
    pub input_schema: Arc<Value>,
}

/// 422: a path or query parameter does not fit the type its command field has.
static INVALID_PARAMETER: ProblemDef = ProblemDef {
    slug: "invalid-parameter",
    status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
    title: "Invalid request parameter",
    description: "A path or query parameter does not match the type of its command input field.",
};

/// A domain module from the registry.
pub struct DomainModule {
    name: &'static str,
    dispatch: Arc<dyn Dispatch>,
    routes: Vec<CommandRoute>,
}

impl DomainModule {
    pub(crate) fn new(
        name: &'static str,
        dispatch: Arc<dyn Dispatch>,
        routes: Vec<CommandRoute>,
    ) -> Self {
        Self {
            name,
            dispatch,
            routes,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn mount(&self, deps: &Arc<Deps>, timing: Option<&Arc<DbTiming>>) -> axum::Router {
        let mut router = axum::Router::new();

        for route in &self.routes {
            let dispatch = Arc::clone(&self.dispatch);
            let path_pattern = route.path.as_str();
            let cmd = route.command.clone();
            let schema = Arc::clone(&route.input_schema);
            let deps = Arc::clone(deps);
            let timing = timing.cloned();

            match route.method.as_str() {
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
                                let timing = timing.clone();

                                async move {
                                    let request_id = scope.request_id.clone();
                                    let presented = presented_from(&headers, false);
                                    let dispatch = match company_dispatch(&dispatch, &headers) {
                                        Ok(scoped) => scoped,
                                        Err(problem) => return problem.into_response(),
                                    };

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
                                        timing.as_deref(),
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
                                  params: Option<Path<BTreeMap<String, String>>>,
                                  Query(query): Query<Vec<(String, String)>>| {
                                let dispatch = Arc::clone(&dispatch);
                                let deps = Arc::clone(&deps);
                                let cmd = cmd.clone();
                                let schema = Arc::clone(&schema);
                                let timing = timing.clone();

                                async move {
                                    let request_id = scope.request_id.clone();
                                    let presented = presented_from(&headers, true);
                                    let dispatch = match company_dispatch(&dispatch, &headers) {
                                        Ok(scoped) => scoped,
                                        Err(problem) => return problem.into_response(),
                                    };

                                    if let Err(limited) =
                                        admit(dispatch.as_ref(), &deps, presented.as_ref()).await
                                    {
                                        return limited.into_response();
                                    }

                                    // Path and query parameters are the command's input,
                                    // each converted to the type its schema field names.
                                    let path_params = params.map(|Path(p)| p).unwrap_or_default();
                                    let input = match get_input(&schema, &query, &path_params) {
                                        Ok(input) => input,
                                        Err(err) => {
                                            return Problem::new(&INVALID_PARAMETER)
                                                .with_detail(err.to_string())
                                                .into_response();
                                        }
                                    };
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
                                        timing.as_deref(),
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
/// the problem document. With request timing on, the response carries this
/// request's database counters in `Server-Timing` (see [`timing`]).
fn respond(result: Result<Value, Problem>, timing: Option<&DbTiming>) -> axum::response::Response {
    let mut response = match result {
        Ok(body) => (axum::http::StatusCode::OK, Json(body)).into_response(),
        Err(problem) => problem.into_response(),
    };
    if let Some(timing) = timing {
        // The value is ASCII built from our own counters, so it always parses.
        if let Ok(value) = axum::http::HeaderValue::from_str(&timing.server_timing()) {
            response.headers_mut().insert(
                axum::http::header::HeaderName::from_static("server-timing"),
                value,
            );
        }
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

    fn optional(&self) -> &'static [Port] {
        // The agent-key limiter, when the runtime provides one.
        &[Port::RateLimiter]
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        let mut deps = Deps::from_ports(&ctx.ports)
            .expect("the dispatch ports (db, clock, id_gen) are declared in requires()");
        // One counter set per router: the runtime builds the router per
        // request, so the counters are per request too (see `timing`).
        let timing = timing::enabled(ctx.config.as_ref()).then(|| {
            let timing = Arc::new(DbTiming::default());
            deps.db = Arc::new(CountingDb::new(Arc::clone(&deps.db), Arc::clone(&timing)));
            timing
        });
        self.mount(&Arc::new(deps), timing.as_ref())
    }

    fn migrations(&self) -> Migrations {
        Migrations::default()
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }
}

/// The command routes of one registry domain (`documents` for
/// `/v1/documents/...`). A module that owns the domain mounts these itself,
/// next to its own routes.
#[must_use]
pub fn domain_routes(registry: &Registry, domain: &str) -> Vec<CommandRoute> {
    registry
        .iter()
        .filter_map(|descriptor| {
            let after_v1 = descriptor.http_path.strip_prefix("/v1/")?;
            let first = after_v1.split('/').next().unwrap_or("");
            (first == domain).then(|| {
                let relative = after_v1.strip_prefix(&format!("{domain}/")).unwrap_or("");
                CommandRoute {
                    method: descriptor.http_method.clone(),
                    path: format!("/{relative}"),
                    command: descriptor.name.clone(),
                    input_schema: Arc::new(descriptor.input_schema.clone()),
                }
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

    let mut domains: HashMap<String, Vec<CommandRoute>> = HashMap::new();

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

            domains.entry(domain).or_default().push(CommandRoute {
                method: descriptor.http_method.clone(),
                path: relative_path,
                command: descriptor.name.clone(),
                input_schema: Arc::new(descriptor.input_schema.clone()),
            });
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
