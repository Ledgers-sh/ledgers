//! Rate limiting test: agent keys are rejected when rate limited.

use cratefield_core::{Decision, Problem, RateLimitError, RateLimiter};
use ledgers_commands::{Limits, Principal, Tier};
use ledgers_server::Dispatch;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Fake rate limiter that allows N requests, then denies the rest.
struct FakeRateLimiter {
    limit: usize,
    call_count: AtomicUsize,
}

impl FakeRateLimiter {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(FakeRateLimiter {
            limit,
            call_count: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl RateLimiter for FakeRateLimiter {
    async fn limit(&self, _key: &str) -> Result<Decision, RateLimitError> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(Decision {
            ok: count < self.limit,
            retry_after: Some(Duration::from_secs(60)),
            quota: None,
        })
    }
}

struct TestDispatch;

#[async_trait::async_trait]
impl Dispatch for TestDispatch {
    async fn dispatch(
        &self,
        _: &str,
        _: Value,
        _: &str,
        _: Option<String>,
    ) -> Result<Value, Problem> {
        Ok(serde_json::json!({"ok": true}))
    }

    async fn principal(&self, credential: &str) -> Result<Principal, Problem> {
        match credential {
            "agent_key" => Ok(Principal::Agent {
                key_id: "key_1".to_string(),
                tier: Tier::Read,
                limits: Limits::new(),
            }),
            "person_key" => Ok(Principal::Person {
                person_id: "user_1".to_string(),
            }),
            _ => Err(Problem::validation_failed("unauthorized")),
        }
    }
}

#[tokio::test]
async fn agent_key_rate_limited_after_limit() {
    let wrapped = Arc::new(TestDispatch);
    let limiter = FakeRateLimiter::new(2);
    let dispatch = ledgers_server::rate_limiting::RateLimitingDispatch::new(
        wrapped.clone(),
        Some(limiter.clone() as Arc<dyn RateLimiter>),
    );

    // Requests 1-2: should succeed
    for i in 1..=2 {
        let result = dispatch
            .dispatch(
                "test",
                serde_json::json!({}),
                &format!("req_{i}"),
                Some("agent_key".to_string()),
            )
            .await;
        assert!(result.is_ok(), "request {i} should succeed");
    }

    // Request 3: should be rate limited (429)
    let result = dispatch
        .dispatch(
            "test",
            serde_json::json!({}),
            "req_3",
            Some("agent_key".to_string()),
        )
        .await;
    assert!(result.is_err(), "request 3 should be rate limited");
    let err = result.unwrap_err();
    assert_eq!(err.status.as_u16(), 429, "should be 429 Too Many Requests");
    assert!(
        err.title.to_lowercase().contains("rate"),
        "title should mention rate"
    );

    // Verify rate limiter was called exactly 3 times (not called for person)
    assert_eq!(limiter.call_count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn person_principal_bypasses_rate_limiting() {
    let wrapped = Arc::new(TestDispatch);
    let limiter = FakeRateLimiter::new(0); // Deny all
    let dispatch = ledgers_server::rate_limiting::RateLimitingDispatch::new(
        wrapped.clone(),
        Some(limiter.clone() as Arc<dyn RateLimiter>),
    );

    // Person request: should succeed even though limiter denies all
    let result = dispatch
        .dispatch(
            "test",
            serde_json::json!({}),
            "req_1",
            Some("person_key".to_string()),
        )
        .await;
    assert!(
        result.is_ok(),
        "person principal should not be rate limited"
    );
    // Limiter should not be called for person
    assert_eq!(limiter.call_count.load(Ordering::SeqCst), 0);
}
