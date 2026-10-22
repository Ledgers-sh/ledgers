//! MCP HTTP endpoint: `/v1/mcp` POST with auth and scope filtering.
//! RFC 9728 protected-resource metadata at `/.well-known/oauth-protected-resource`.

use crate::dispatch::{DISPATCH_PORTS, Deps};
use crate::routes::{Dispatch, Presented};
use axum::extract::Query;
use axum::http::header;
use cratefield_core::axum::{
    self,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext};
use ledgers_commands::{Principal, Registry, Tier, mcp};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Shared MCP module state.
#[derive(Clone)]
struct McpState {
    registry: Arc<Registry>,
    dispatch: Arc<dyn Dispatch>,
    public_url: String,
    origin: String,
}

pub struct McpModule {
    state: McpState,
}

impl McpModule {
    pub fn new(registry: Arc<Registry>, dispatch: Arc<dyn Dispatch>, public_url: String) -> Self {
        let origin = extract_origin(&public_url);
        Self {
            state: McpState {
                registry,
                dispatch,
                public_url,
                origin,
            },
        }
    }
}

/// Extract the origin (`scheme://host[:port]`) from a URL.
fn extract_origin(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if let Some(scheme_end) = trimmed.find("://")
        && let Some(path_start) = trimmed[scheme_end + 3..].find('/')
    {
        return trimmed[..scheme_end + 3 + path_start].to_string();
    }
    trimmed.to_string()
}

/// Query parameters for MCP POST request.
#[derive(Deserialize, Default)]
#[serde(default)]
struct McpQuery {
    read_only: Option<String>,
}

impl Module for McpModule {
    fn name(&self) -> &'static str {
        "mcp"
    }
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
    fn requires(&self) -> &'static [cratefield_core::Port] {
        DISPATCH_PORTS
    }

    fn optional(&self) -> &'static [cratefield_core::Port] {
        // The agent-key limiter, when the runtime provides one.
        &[cratefield_core::Port::RateLimiter]
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        let state = self.state.clone();
        let deps = Arc::new(
            Deps::from_ports(&ctx.ports)
                .expect("the dispatch ports (db, clock, id_gen) are declared in requires()"),
        );

        axum::Router::new().route(
            "/",
            post({
                let state = state.clone();
                let deps = Arc::clone(&deps);
                move |headers: HeaderMap, Query(query): Query<McpQuery>, body: String| {
                    let state = state.clone();
                    let deps = Arc::clone(&deps);
                    async move { mcp_post(state, headers, query, body, deps).await }
                }
            })
            .get(move || async { method_not_allowed() }),
        )
    }

    fn well_known(&self) -> Option<axum::Router> {
        Some(axum::Router::new().route(
            "/oauth-protected-resource",
            get({
                let public_url = self.state.public_url.clone();
                move || {
                    let url = public_url.clone();
                    async move {
                        let public_url = url.as_str();
                        let public_url = public_url.trim_end_matches('/');
                        (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, "application/json")],
                            json!({
                                "resource": format!("{}/v1/mcp", public_url),
                                "authorization_servers": [],
                                "bearer_methods_supported": ["header"]
                            })
                            .to_string(),
                        )
                    }
                }
            }),
        ))
    }

    fn migrations(&self) -> Migrations {
        Migrations::default()
    }
    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }
}

/// Extract bearer token from Authorization header
fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(ToString::to_string))
}

/// Create WWW-Authenticate header with `resource_metadata`
fn www_authenticate(public_url: &str) -> axum::http::HeaderValue {
    format!("Bearer resource_metadata=\"{public_url}/.well-known/oauth-protected-resource\"")
        .parse()
        .unwrap()
}

/// Create unauthorized response with authentication headers
fn unauthorized_response(public_url: &str) -> (StatusCode, HeaderMap, String) {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse().unwrap());
    headers.insert(header::WWW_AUTHENTICATE, www_authenticate(public_url));
    (StatusCode::UNAUTHORIZED, headers, String::new())
}

/// Create scope from principal and query/header parameters
fn scope_for(principal: &Principal, query: &McpQuery, headers: &HeaderMap) -> mcp::ToolScope {
    let read_only_query = query.read_only.as_deref().is_some_and(|v| v == "true");
    let read_only_header = headers
        .get("ledgers-read-only")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.to_lowercase() == "true");

    let (tier, read_only) = match principal {
        Principal::Person { .. } => (Tier::Read, true),
        Principal::Agent { tier, .. } => (*tier, read_only_query || read_only_header),
    };
    mcp::ToolScope { tier, read_only }
}

/// POST /mcp: JSON-RPC 2.0 handler with authentication and authorization
async fn mcp_post(
    state: McpState,
    headers: HeaderMap,
    query: McpQuery,
    body: String,
    deps: Arc<Deps>,
) -> (StatusCode, HeaderMap, String) {
    // (a) Check Origin header if present
    if let Some(origin) = headers.get("origin")
        && let Ok(origin_str) = origin.to_str()
        && origin_str != state.origin
    {
        let mut result_headers = HeaderMap::new();
        result_headers.insert("content-type", "application/json".parse().unwrap());
        result_headers.insert(
            header::WWW_AUTHENTICATE,
            www_authenticate(&state.public_url),
        );
        return (StatusCode::FORBIDDEN, result_headers, String::new());
    }

    // (b) Extract bearer token
    let Some(bearer) = extract_bearer(&headers) else {
        return unauthorized_response(&state.public_url);
    };

    // (c) The company the agent key belongs to serves the request: the key is
    // verified against that company's keys, never against the deployment's.
    let dispatch = match state.dispatch.key_company(&deps, &bearer).await {
        Some(company) => Arc::clone(&state.dispatch).scoped(&company),
        None => Arc::clone(&state.dispatch),
    };

    // (c) Authenticate the bearer token
    let Ok(principal) = dispatch.principal(&deps, &bearer).await else {
        return unauthorized_response(&state.public_url);
    };

    // (d) Parse JSON body
    let Ok(message) = serde_json::from_str::<Value>(&body) else {
        let mut error_headers = HeaderMap::new();
        error_headers.insert("content-type", "application/json".parse().unwrap());
        return (
            StatusCode::BAD_REQUEST,
            error_headers,
            json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": -32700,
                    "message": "Parse error"
                }
            })
            .to_string(),
        );
    };

    let scope = scope_for(&principal, &query, &headers);

    // Generate request ID using id_gen
    let request_id = format!("mcp-{}", deps.id_gen.ulid());

    // Build a ToolCaller that uses dispatch
    let caller = ServerToolCaller {
        dispatch,
        deps: Arc::clone(&deps),
        credential: Some(bearer),
        request_id,
    };

    // (e) Call the MCP handler
    let (status, body) = match mcp::handle(&state.registry, scope, &caller, message).await {
        Some(response) => (StatusCode::OK, response.to_string()),
        None => (StatusCode::ACCEPTED, String::new()),
    };

    let mut result_headers = HeaderMap::new();
    result_headers.insert("content-type", "application/json".parse().unwrap());

    (status, result_headers, body)
}

/// Implements `ToolCaller` by calling the dispatch trait.
struct ServerToolCaller {
    dispatch: Arc<dyn Dispatch>,
    deps: Arc<Deps>,
    credential: Option<String>,
    request_id: String,
}

#[async_trait::async_trait]
impl mcp::ToolCaller for ServerToolCaller {
    async fn call(
        &self,
        descriptor: &ledgers_commands::CommandDescriptor,
        input: Value,
        _dry_run: bool,
    ) -> mcp::ToolOutcome {
        let presented = self.credential.clone().map(Presented::Bearer);
        if let Err(limited) =
            crate::rate_limiting::admit(self.dispatch.as_ref(), &self.deps, presented.as_ref())
                .await
        {
            return mcp::ToolOutcome::Failed(limited.problem().to_string());
        }
        match self
            .dispatch
            .dispatch(
                &self.deps,
                &descriptor.name,
                input,
                &self.request_id,
                presented,
            )
            .await
        {
            Ok(value) => {
                // Check if response indicates needs-approval
                if let Some(obj) = value.as_object()
                    && obj.get("status").and_then(|s| s.as_str()) == Some("needs-approval")
                    && let Ok(mcp::ToolOutcome::NeedsApproval { card }) =
                        mcp::ToolOutcome::from_dispatch(Ok(value.clone()))
                {
                    return mcp::ToolOutcome::NeedsApproval { card };
                }
                mcp::ToolOutcome::Done(value)
            }
            Err(problem) => {
                let msg = problem.to_string();
                // Check if it's a policy denial (status 403)
                let is_denied = problem.status == StatusCode::FORBIDDEN;
                if is_denied {
                    mcp::ToolOutcome::Denied(msg)
                } else {
                    mcp::ToolOutcome::Failed(msg)
                }
            }
        }
    }
}

/// GET /v1/mcp: Return 405 Method Not Allowed
fn method_not_allowed() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::CONTENT_TYPE, "application/json")],
    )
}
