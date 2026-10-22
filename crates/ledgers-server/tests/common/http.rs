#![allow(dead_code, clippy::needless_pass_by_value)]
//! Raw HTTP helpers for tests that need headers, cookies or both
//! credential kinds: they drive the fixture's router with `oneshot`.

use super::Fixture;
use cratefield_core::axum::body::{Body, to_bytes};
use cratefield_core::axum::http::{HeaderMap, Method, Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

/// The app origin the test sessions and cookie-authenticated calls use.
pub const APP_ORIGIN: &str = "https://app.test";
/// The session cookie name the server reads.
pub const SESSION_COOKIE: &str = "__Host-ledgers-session";

/// How a request presents its caller.
pub enum Auth<'a> {
    /// `Authorization: Bearer <token>`.
    Bearer(&'a str),
    /// The session cookie with this id.
    Session(&'a str),
    /// No credential.
    Anonymous,
}

/// A response, read to the end.
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub text: String,
}

impl Reply {
    /// The body as JSON (`Null` when it is not JSON).
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    /// The `Retry-After` header as text, when present.
    pub fn retry_after(&self) -> Option<String> {
        self.headers
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }
}

/// Sends one request. `origin` adds the app's `Origin` header.
pub fn call(
    fx: &Fixture,
    method: Method,
    path: &str,
    auth: Auth<'_>,
    origin: bool,
    body: Option<Value>,
) -> Reply {
    let mut builder = Request::builder().method(method).uri(path);
    match auth {
        Auth::Bearer(token) => {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        Auth::Session(id) => {
            builder = builder.header(header::COOKIE, format!("{SESSION_COOKIE}={id}"));
        }
        Auth::Anonymous => {}
    }
    if origin {
        builder = builder.header(header::ORIGIN, APP_ORIGIN);
    }
    let request = match body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request");
    pollster::block_on(async {
        let response = fx
            .router()
            .clone()
            .oneshot(request)
            .await
            .expect("router answers");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        Reply {
            status,
            headers,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        }
    })
}

/// A JSON-RPC message to `/v1/mcp` with the bearer `token`.
pub fn mcp(fx: &Fixture, token: &str, message: Value) -> Reply {
    call(
        fx,
        Method::POST,
        "/v1/mcp",
        Auth::Bearer(token),
        false,
        Some(message),
    )
}
