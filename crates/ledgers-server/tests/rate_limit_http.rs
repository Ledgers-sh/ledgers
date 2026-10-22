//! Agent-key rate limits over HTTP: a refusal is a 429 whose `Retry-After`
//! is the limiter's own pause, a failing limiter fails closed with the
//! documented fallback, and a person's session is never throttled.

mod common;

use common::http::{Auth, call, mcp};
use common::{OWNER_SESSION, fixture_with_options, owner_session};
use cratefield_core::axum::http::Method;
use cratefield_core::{Decision, RateLimitError, RateLimiter};
use ledgers_server::McpModule;
use ledgers_server::dispatch::{self, ServerDispatch};
use ledgers_server::rate_limiting::{FALLBACK_RETRY_AFTER_SECS, InProcessRateLimiter};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Admits the first `allowed` requests; every refusal names `pause`.
struct PausingLimiter {
    allowed: usize,
    pause: Option<Duration>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl RateLimiter for PausingLimiter {
    async fn limit(&self, _key: &str) -> Result<Decision, RateLimitError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Decision {
            ok: call < self.allowed,
            retry_after: self.pause,
            quota: None,
        })
    }
}

/// A limiter whose backend is down.
struct BrokenLimiter;

#[async_trait::async_trait]
impl RateLimiter for BrokenLimiter {
    async fn limit(&self, _key: &str) -> Result<Decision, RateLimitError> {
        Err(RateLimitError::Transport("backend unavailable".to_owned()))
    }
}

fn with_limiter(limiter: Arc<dyn RateLimiter>) -> common::Fixture {
    let mcp_module = McpModule::new(
        Arc::new(dispatch::registry()),
        Arc::new(ServerDispatch::new(common::COMPANY)),
        "https://api.test".to_owned(),
    );
    fixture_with_options(
        vec![Arc::new(mcp_module)],
        Some(limiter),
        vec![common::http::APP_ORIGIN.to_owned()],
    )
}

fn get_accounts(fx: &common::Fixture, token: &str) -> common::http::Reply {
    call(
        fx,
        Method::GET,
        "/v1/accounts",
        Auth::Bearer(token),
        false,
        None,
    )
}

#[test]
fn an_over_limit_agent_gets_429_with_the_limiters_own_retry_after() {
    let fx = with_limiter(Arc::new(PausingLimiter {
        allowed: 2,
        pause: Some(Duration::from_secs(13)),
        calls: AtomicUsize::new(0),
    }));
    let token = fx.keys.full.clone();
    assert_eq!(get_accounts(&fx, &token).status.as_u16(), 200);
    assert_eq!(get_accounts(&fx, &token).status.as_u16(), 200);
    let refused = get_accounts(&fx, &token);
    assert_eq!(refused.status.as_u16(), 429, "{}", refused.text);
    assert_eq!(
        refused.retry_after().as_deref(),
        Some("13"),
        "Retry-After must be the limiter's pause, not the old constant 60"
    );
    let body = refused.json();
    assert_eq!(body["status"], 429);
    assert_eq!(body["retryAfterSecs"], 13);
    assert!(
        !refused.text.contains(&token),
        "the 429 body echoes the token"
    );
}

#[test]
fn the_native_limiter_gives_an_exact_retry_after_for_a_fresh_window() {
    let fx = with_limiter(Arc::new(InProcessRateLimiter::new(
        1,
        Duration::from_secs(60),
    )));
    let token = fx.keys.full.clone();
    assert_eq!(get_accounts(&fx, &token).status.as_u16(), 200);
    let refused = get_accounts(&fx, &token);
    assert_eq!(refused.status.as_u16(), 429, "{}", refused.text);
    // The window started a moment ago: the pause left is the full minute,
    // rounded up to whole seconds.
    assert_eq!(refused.retry_after().as_deref(), Some("60"));
}

#[test]
fn a_limiter_with_no_pause_gives_the_workers_binding_fallback() {
    // The Workers Rate Limiting binding reports no pause at all; the 429 then
    // carries the binding's configured period.
    let fx = with_limiter(Arc::new(PausingLimiter {
        allowed: 0,
        pause: None,
        calls: AtomicUsize::new(0),
    }));
    let refused = get_accounts(&fx, &fx.keys.full.clone());
    assert_eq!(refused.status.as_u16(), 429);
    assert_eq!(
        refused.retry_after(),
        Some(FALLBACK_RETRY_AFTER_SECS.to_string())
    );
}

#[test]
fn a_failing_limiter_fails_closed_with_the_fallback() {
    let fx = with_limiter(Arc::new(BrokenLimiter));
    let refused = get_accounts(&fx, &fx.keys.full.clone());
    assert_eq!(refused.status.as_u16(), 429, "{}", refused.text);
    assert_eq!(
        refused.retry_after(),
        Some(FALLBACK_RETRY_AFTER_SECS.to_string())
    );
}

#[test]
fn a_person_session_is_never_throttled() {
    let fx = with_limiter(Arc::new(PausingLimiter {
        allowed: 0,
        pause: Some(Duration::from_secs(60)),
        calls: AtomicUsize::new(0),
    }));
    owner_session(&fx);
    for _ in 0..3 {
        let reply = call(
            &fx,
            Method::GET,
            "/v1/keys",
            Auth::Session(OWNER_SESSION),
            false,
            None,
        );
        assert_eq!(reply.status.as_u16(), 200, "{}", reply.text);
    }
}

#[test]
fn an_over_limit_mcp_tool_call_is_refused_as_a_failed_tool() {
    let fx = with_limiter(Arc::new(PausingLimiter {
        allowed: 0,
        pause: Some(Duration::from_secs(9)),
        calls: AtomicUsize::new(0),
    }));
    let reply = mcp(
        &fx,
        &fx.keys.full.clone(),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "accounts_list", "arguments": {}}}),
    );
    assert_eq!(reply.status.as_u16(), 200, "{}", reply.text);
    assert!(reply.text.contains("\"isError\":true"), "{}", reply.text);
    assert!(reply.text.contains("429"), "{}", reply.text);
}
