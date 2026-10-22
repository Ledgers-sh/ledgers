//! Rate limiting for agent keys.
//!
//! Every HTTP route and MCP tool call from an agent key is checked against
//! the harness `RateLimiter` before it dispatches ([`admit`]). The limiter is
//! read from the request's [`Deps`] (the ports the module context resolved),
//! not captured at harness build time, because the Cloudflare runtime
//! resolves its rate-limiting binding per request. Person principals pass
//! through unchanged.
//!
//! The check runs in the transport layer rather than inside `Dispatch`, so a
//! refusal carries its `Retry-After` value to the HTTP response: a
//! [`Limited`] is built where the limiter's decision is known.
//!
//! The native runtime has no Redis limiter by default, so it uses
//! [`InProcessRateLimiter`] (`LEDGERS_RATE_LIMIT_PER_MIN`); see
//! docs/threat-model.md.

#[cfg(not(target_arch = "wasm32"))]
use async_trait::async_trait;
#[cfg(not(target_arch = "wasm32"))]
use cratefield_core::RateLimitError;
use cratefield_core::axum::http::{HeaderValue, header};
use cratefield_core::axum::response::{IntoResponse, Response};
use cratefield_core::{Decision, Problem, RateLimiter};
use ledgers_commands::{CommandError, Principal};
use std::sync::Arc;

use crate::dispatch::Deps;
use crate::routes::{Dispatch, Presented};

/// The `Retry-After` value, in seconds, when the limiter answers without a
/// pause: the Workers Rate Limiting binding exposes none, and a failing
/// limiter has none to give. It is the period `wrangler.toml` configures for
/// the binding (`simple = { limit = 100, period = 60 }`).
pub const FALLBACK_RETRY_AFTER_SECS: u64 = 60;

/// A refused agent-key request: the limit is spent until `retry_after_secs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limited {
    /// Seconds until the caller may try again (at least 1).
    pub retry_after_secs: u64,
}

impl Limited {
    /// The RFC 9457 `rate-limited` problem, carrying `retryAfterSecs`.
    #[must_use]
    pub fn problem(&self) -> Problem {
        CommandError::RateLimited {
            retry_after_secs: Some(self.retry_after_secs),
        }
        .to_problem()
    }

    /// The 429 response: the problem document with `Retry-After` set to the
    /// limiter's value.
    #[must_use]
    pub fn into_response(self) -> Response {
        let mut response = self.problem().into_response();
        if let Ok(value) = HeaderValue::from_str(&self.retry_after_secs.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

/// Converts a limiter pause to whole seconds, rounding up, never below 1.
#[must_use]
pub fn retry_after_secs(pause: std::time::Duration) -> u64 {
    let rounded = pause.as_secs() + u64::from(pause.subsec_nanos() > 0);
    rounded.max(1)
}

/// The refusal a limiter decision implies. `Some(pause)` is the limiter's
/// own value; `None` (no pause known) falls back to
/// [`FALLBACK_RETRY_AFTER_SECS`].
#[must_use]
pub fn limited_from_decision(decision: &Decision) -> Limited {
    Limited {
        retry_after_secs: decision
            .retry_after
            .map_or(FALLBACK_RETRY_AFTER_SECS, retry_after_secs),
    }
}

/// Applies the agent-key limit to `principal`.
///
/// Person principals are never limited. With no limiter configured the call
/// is not throttled. A limiter that refuses the request returns its own pause;
/// a limiter that fails denies the request, with [`FALLBACK_RETRY_AFTER_SECS`].
///
/// # Errors
/// [`Limited`] (HTTP 429) when the key is over its limit or the limiter
/// cannot answer.
pub async fn enforce_agent_limit(
    limiter: Option<&Arc<dyn RateLimiter>>,
    principal: &Principal,
) -> Result<(), Limited> {
    let Principal::Agent { key_id, .. } = principal else {
        return Ok(());
    };
    enforce_key_limit(limiter, key_id).await
}

/// Charges one request to the bucket of agent key `key_id`, when a limiter is
/// configured. The key id, never the secret, names the bucket.
///
/// # Errors
/// [`Limited`] when the key is over its limit, or the limiter fails.
pub async fn enforce_key_limit(
    limiter: Option<&Arc<dyn RateLimiter>>,
    key_id: &str,
) -> Result<(), Limited> {
    let Some(limiter) = limiter else {
        return Ok(());
    };
    let rate_limit_key = format!("agent-key:{key_id}");
    match limiter.limit(&rate_limit_key).await {
        Ok(decision) if decision.ok => Ok(()),
        Ok(decision) => Err(limited_from_decision(&decision)),
        Err(_) => Err(Limited {
            retry_after_secs: FALLBACK_RETRY_AFTER_SECS,
        }),
    }
}

/// Admits or refuses one request before it dispatches.
///
/// Only a bearer agent key is limited. A session cookie is a person and
/// passes through. A credential that does not resolve is admitted here and
/// answered as unauthenticated by the dispatch itself.
///
/// # Errors
/// [`Limited`] when the agent key is over its limit.
pub async fn admit(
    dispatch: &dyn Dispatch,
    deps: &Deps,
    presented: Option<&Presented>,
) -> Result<(), Limited> {
    if let Some(Presented::Bearer(token)) = presented
        && let Some(key_id) = dispatch.verified_key_id(deps, token).await
    {
        enforce_key_limit(deps.rate_limiter.as_ref(), &key_id).await?;
    }
    Ok(())
}

/// A fixed-window limiter in this process, for the native runtime.
///
/// Each key gets `limit` requests per `window`, counted from its first request
/// in the window. A refusal's pause is the time left in that window, so the
/// `Retry-After` it produces is exact. The state is per process: a native
/// deployment running several processes needs a shared limiter (Redis) to hold
/// the limit across them.
#[cfg(not(target_arch = "wasm32"))]
pub struct InProcessRateLimiter {
    limit: u64,
    window: std::time::Duration,
    windows: std::sync::Mutex<std::collections::HashMap<String, FixedWindow>>,
}

/// Idle windows are dropped once the table grows past this many keys.
#[cfg(not(target_arch = "wasm32"))]
const PRUNE_ABOVE_KEYS: usize = 4096;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct FixedWindow {
    started: std::time::Instant,
    count: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl InProcessRateLimiter {
    /// `limit` requests per key in each `window`.
    #[must_use]
    pub fn new(limit: u64, window: std::time::Duration) -> Self {
        Self {
            limit,
            window,
            windows: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The per-minute limiter `LEDGERS_RATE_LIMIT_PER_MIN` configures.
    #[must_use]
    pub fn per_minute(limit: u64) -> Self {
        Self::new(limit, std::time::Duration::from_secs(60))
    }

    /// The decision for one request of `key` at `now`. Pure apart from the
    /// table it updates, so the window arithmetic is testable with fixed times.
    fn decide(&self, key: &str, now: std::time::Instant) -> Result<Decision, RateLimitError> {
        let mut windows = self
            .windows
            .lock()
            .map_err(|_| RateLimitError::Transport("rate limiter state is poisoned".to_owned()))?;
        if windows.len() > PRUNE_ABOVE_KEYS {
            let window = self.window;
            windows.retain(|_, w| now.saturating_duration_since(w.started) < window);
        }
        let slot = windows.entry(key.to_owned()).or_insert(FixedWindow {
            started: now,
            count: 0,
        });
        if now.saturating_duration_since(slot.started) >= self.window {
            *slot = FixedWindow {
                started: now,
                count: 0,
            };
        }
        let left = self
            .window
            .saturating_sub(now.saturating_duration_since(slot.started));
        if slot.count < self.limit {
            slot.count += 1;
            Ok(Decision {
                ok: true,
                retry_after: None,
                quota: None,
            })
        } else {
            Ok(Decision {
                ok: false,
                retry_after: Some(left),
                quota: None,
            })
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl RateLimiter for InProcessRateLimiter {
    async fn limit(&self, key: &str) -> Result<Decision, RateLimitError> {
        self.decide(key, std::time::Instant::now())
    }
}

/// The `LEDGERS_RATE_LIMIT_PER_MIN` setting: a positive integer, default 100.
///
/// # Errors
/// A message when the value is set but is not a positive integer.
pub fn per_minute_from_setting(raw: Option<&str>) -> Result<u64, String> {
    match raw.map(str::trim) {
        None | Some("") => Ok(DEFAULT_PER_MINUTE),
        Some(text) => match text.parse::<u64>() {
            Ok(limit) if limit > 0 => Ok(limit),
            _ => Err(format!(
                "LEDGERS_RATE_LIMIT_PER_MIN must be a positive integer, got {text:?}"
            )),
        },
    }
}

/// The per-key requests a minute when `LEDGERS_RATE_LIMIT_PER_MIN` is unset.
pub const DEFAULT_PER_MINUTE: u64 = 100;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn retry_after_rounds_up_and_never_zero() {
        assert_eq!(retry_after_secs(Duration::from_millis(1)), 1);
        assert_eq!(retry_after_secs(Duration::from_secs(7)), 7);
        assert_eq!(retry_after_secs(Duration::from_millis(7_001)), 8);
        assert_eq!(retry_after_secs(Duration::ZERO), 1);
    }

    #[test]
    fn a_decision_without_a_pause_falls_back_to_the_binding_period() {
        let decision = Decision {
            ok: false,
            retry_after: None,
            quota: None,
        };
        assert_eq!(
            limited_from_decision(&decision).retry_after_secs,
            FALLBACK_RETRY_AFTER_SECS
        );
    }

    #[test]
    fn a_decision_pause_is_used_as_is() {
        let decision = Decision {
            ok: false,
            retry_after: Some(Duration::from_secs(13)),
            quota: None,
        };
        assert_eq!(limited_from_decision(&decision).retry_after_secs, 13);
    }

    #[test]
    fn fixed_window_admits_the_limit_then_refuses_with_the_time_left() {
        let limiter = InProcessRateLimiter::new(2, Duration::from_secs(60));
        let start = Instant::now();
        assert!(limiter.decide("k", start).expect("decides").ok);
        assert!(
            limiter
                .decide("k", start + Duration::from_secs(10))
                .expect("decides")
                .ok
        );
        let refused = limiter
            .decide("k", start + Duration::from_secs(20))
            .expect("decides");
        assert!(!refused.ok);
        assert_eq!(refused.retry_after, Some(Duration::from_secs(40)));
        assert_eq!(
            limited_from_decision(&refused).retry_after_secs,
            40,
            "the native pause reaches Retry-After exactly"
        );
    }

    #[test]
    fn fixed_window_resets_when_the_window_turns_over() {
        let limiter = InProcessRateLimiter::new(1, Duration::from_secs(60));
        let start = Instant::now();
        assert!(limiter.decide("k", start).expect("decides").ok);
        assert!(
            !limiter
                .decide("k", start + Duration::from_secs(59))
                .expect("decides")
                .ok
        );
        assert!(
            limiter
                .decide("k", start + Duration::from_secs(60))
                .expect("decides")
                .ok
        );
    }

    #[test]
    fn keys_have_separate_budgets() {
        let limiter = InProcessRateLimiter::new(1, Duration::from_secs(60));
        let start = Instant::now();
        assert!(limiter.decide("a", start).expect("decides").ok);
        assert!(limiter.decide("b", start).expect("decides").ok);
        assert!(!limiter.decide("a", start).expect("decides").ok);
    }

    #[test]
    fn the_setting_defaults_to_100_and_rejects_non_positive_values() {
        assert_eq!(per_minute_from_setting(None), Ok(100));
        assert_eq!(per_minute_from_setting(Some(" 25 ")), Ok(25));
        assert!(per_minute_from_setting(Some("0")).is_err());
        assert!(per_minute_from_setting(Some("-3")).is_err());
        assert!(per_minute_from_setting(Some("many")).is_err());
    }
}
