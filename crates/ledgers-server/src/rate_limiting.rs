//! Rate limiting dispatch wrapper for agent keys.
//!
//! Wraps the command dispatch to check rate limits per agent key before
//! executing commands. Person principals pass through unchanged.

use cratefield_core::Problem;
use ledgers_commands::{CommandError, Principal};
use serde_json::Value;
use std::sync::Arc;

use crate::routes::Dispatch;

/// Dispatch wrapper that enforces rate limiting on agent keys.
///
/// The rate limiter is optional: if None, no rate limiting is applied.
/// Rate limiter errors fail closed for agent keys (deny the request).
pub struct RateLimitingDispatch {
    wrapped: Arc<dyn Dispatch>,
    rate_limiter: Option<Arc<dyn cratefield_core::RateLimiter>>,
}

impl RateLimitingDispatch {
    /// Creates a new rate-limiting dispatch wrapper.
    pub fn new(
        wrapped: Arc<dyn Dispatch>,
        rate_limiter: Option<Arc<dyn cratefield_core::RateLimiter>>,
    ) -> Self {
        Self {
            wrapped,
            rate_limiter,
        }
    }
}

#[async_trait::async_trait]
impl Dispatch for RateLimitingDispatch {
    async fn dispatch(
        &self,
        command: &str,
        input: Value,
        request_id: &str,
        credential: Option<String>,
    ) -> Result<Value, Problem> {
        // Resolve the principal to check if it's an agent key.
        let Ok(principal) = self.principal(credential.as_deref().unwrap_or("")).await else {
            // Unauthenticated: let the wrapped dispatch handle it.
            return self
                .wrapped
                .dispatch(command, input, request_id, credential)
                .await;
        };

        // Check rate limit for agent keys only.
        if let Principal::Agent { key_id, .. } = &principal
            && let Some(limiter) = &self.rate_limiter
        {
            // Fail closed: rate limiter errors deny the request for agent keys.
            let rate_limit_key = format!("agent-key:{key_id}");
            match limiter.limit(&rate_limit_key).await {
                Ok(decision) if !decision.ok => {
                    let retry_secs = decision.retry_after.map(|d| d.as_secs());
                    return Err(CommandError::RateLimited {
                        retry_after_secs: retry_secs,
                    }
                    .to_problem());
                }
                Err(_) => {
                    // Backend unavailable: fail closed (deny).
                    return Err(CommandError::RateLimited {
                        retry_after_secs: Some(60),
                    }
                    .to_problem());
                }
                Ok(_) => {} // ok == true, proceed
            }
        }

        // Proceed with the wrapped dispatch.
        self.wrapped
            .dispatch(command, input, request_id, credential)
            .await
    }

    async fn principal(&self, credential: &str) -> Result<Principal, Problem> {
        self.wrapped.principal(credential).await
    }
}
