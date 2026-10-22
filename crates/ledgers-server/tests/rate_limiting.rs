//! Agent-key rate limiting: the limiter throttles agent keys, fails closed,
//! and never throttles a person.

use cratefield_core::{Decision, RateLimitError, RateLimiter};
use ledgers_commands::{Limits, Principal, Tier};
use ledgers_server::rate_limiting::enforce_agent_limit;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Allows the first `limit` requests, then denies the rest.
struct FakeRateLimiter {
    limit: usize,
    calls: AtomicUsize,
}

impl FakeRateLimiter {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl RateLimiter for FakeRateLimiter {
    async fn limit(&self, _key: &str) -> Result<Decision, RateLimitError> {
        let count = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Decision {
            ok: count < self.limit,
            retry_after: Some(Duration::from_secs(60)),
            quota: None,
        })
    }
}

/// A limiter whose backend is down.
struct BrokenRateLimiter;

#[async_trait::async_trait]
impl RateLimiter for BrokenRateLimiter {
    async fn limit(&self, _key: &str) -> Result<Decision, RateLimitError> {
        Err(RateLimitError::Transport("backend unavailable".to_string()))
    }
}

fn agent() -> Principal {
    Principal::Agent {
        key_id: "key_1".to_string(),
        tier: Tier::Read,
        limits: Limits::new(),
    }
}

fn person() -> Principal {
    Principal::Person {
        person_id: "user_1".to_string(),
    }
}

#[tokio::test]
async fn agent_key_is_limited_after_its_allowance() {
    let limiter = FakeRateLimiter::new(2);
    let dyn_limiter: Arc<dyn RateLimiter> = limiter.clone();
    let limiter_ref = Some(&dyn_limiter);

    assert!(enforce_agent_limit(limiter_ref, &agent()).await.is_ok());
    assert!(enforce_agent_limit(limiter_ref, &agent()).await.is_ok());
    let err = enforce_agent_limit(limiter_ref, &agent())
        .await
        .expect_err("third request is over the limit");
    assert_eq!(err.status.as_u16(), 429);
    assert_eq!(limiter.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn person_principal_is_never_limited() {
    let limiter = FakeRateLimiter::new(0);
    let dyn_limiter: Arc<dyn RateLimiter> = limiter.clone();
    assert!(
        enforce_agent_limit(Some(&dyn_limiter), &person())
            .await
            .is_ok()
    );
    assert_eq!(limiter.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn limiter_failure_denies_an_agent_key() {
    let dyn_limiter: Arc<dyn RateLimiter> = Arc::new(BrokenRateLimiter);
    let err = enforce_agent_limit(Some(&dyn_limiter), &agent())
        .await
        .expect_err("a failing limiter fails closed");
    assert_eq!(err.status.as_u16(), 429);
}
