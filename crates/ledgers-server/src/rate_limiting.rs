//! Rate limiting for agent keys.
//!
//! [`RateLimitingDispatch`] wraps the server's [`Dispatch`] so every HTTP and
//! MCP command from an agent key is checked against the harness
//! `RateLimiter` before it runs. Person principals pass through unchanged.
//!
//! The limiter is read from the request's [`Deps`] (the ports the module
//! context resolved), not captured at harness build time, because the
//! Cloudflare runtime resolves its rate-limiting binding per request.

use async_trait::async_trait;
use cratefield_core::{Problem, RateLimiter};
use ledgers_commands::{CommandError, Principal};
use serde_json::Value;
use std::sync::Arc;

use crate::dispatch::Deps;
use crate::routes::Dispatch;

/// Dispatch wrapper that enforces the per-agent-key rate limit.
pub struct RateLimitingDispatch {
    wrapped: Arc<dyn Dispatch>,
}

impl RateLimitingDispatch {
    /// Wraps `wrapped` with the agent-key rate limit.
    #[must_use]
    pub fn new(wrapped: Arc<dyn Dispatch>) -> Self {
        Self { wrapped }
    }
}

/// Applies the agent-key limit to `principal`.
///
/// Person principals are never limited. With no limiter configured the call
/// is not throttled. A limiter that refuses the request, or fails, denies
/// it: the failure case returns `RateLimited` with a 60 second retry hint.
///
/// # Errors
/// `RateLimited` (HTTP 429) when the key is over its limit or the limiter
/// cannot answer.
pub async fn enforce_agent_limit(
    limiter: Option<&Arc<dyn RateLimiter>>,
    principal: &Principal,
) -> Result<(), Problem> {
    let (Principal::Agent { key_id, .. }, Some(limiter)) = (principal, limiter) else {
        return Ok(());
    };
    // The key id, never the secret, names the bucket.
    let rate_limit_key = format!("agent-key:{key_id}");
    match limiter.limit(&rate_limit_key).await {
        Ok(decision) if decision.ok => Ok(()),
        Ok(decision) => Err(CommandError::RateLimited {
            retry_after_secs: decision.retry_after.map(|d| d.as_secs()),
        }
        .to_problem()),
        Err(_) => Err(CommandError::RateLimited {
            retry_after_secs: Some(60),
        }
        .to_problem()),
    }
}

#[async_trait]
impl Dispatch for RateLimitingDispatch {
    async fn dispatch(
        &self,
        deps: &Deps,
        command: &str,
        input: Value,
        request_id: &str,
        credential: Option<String>,
    ) -> Result<Value, Problem> {
        // An unresolvable credential is left to the wrapped dispatch, which
        // answers it as unauthenticated.
        if let Some(credential) = credential.as_deref()
            && let Ok(principal) = self.wrapped.principal(deps, credential).await
        {
            enforce_agent_limit(deps.rate_limiter.as_ref(), &principal).await?;
        }
        self.wrapped
            .dispatch(deps, command, input, request_id, credential)
            .await
    }

    async fn principal(&self, deps: &Deps, credential: &str) -> Result<Principal, Problem> {
        self.wrapped.principal(deps, credential).await
    }
}
