//! HTTP session: sends a planned request and maps the response to an outcome.
//!
//! Response mapping (the contract agents rely on):
//! - 2xx with `{"status":"needs-approval","approval":{...}}` → [`CallOutcome::NeedsApproval`] (exit 2)
//! - 2xx with `dry_run` requested → [`CallOutcome::DryRun`] (exit 0)
//! - 2xx otherwise → [`CallOutcome::Done`] (exit 0)
//! - 403 → [`CallOutcome::Denied`] (exit 3)
//! - any other status → [`CliError`] (exit 1)

use crate::error::{CliError, Result};
use crate::request::PlannedRequest;
use reqwest::StatusCode;
use serde_json::Value;
use std::time::Duration;

/// The outcome of a command call.
#[derive(Clone, Debug, PartialEq)]
pub enum CallOutcome {
    /// The command ran; the body is its output.
    Done(Value),
    /// A dry run; the body is the preview. Nothing was written.
    DryRun(Value),
    /// Policy needs a human approval first.
    NeedsApproval {
        /// The approval request id.
        approval_id: String,
        /// Where a person approves the request.
        url: String,
        /// Why approval is needed.
        reason: String,
        /// The journal preview the approval covers.
        preview: Value,
    },
    /// Policy denied the command; the string is the server's reason.
    Denied(String),
}

/// An HTTP session: a server URL and the API key.
pub struct Session {
    /// The server base URL, e.g. `http://localhost:3000`.
    pub url: String,
    /// The API key, sent as a Bearer token.
    pub key: String,
    client: reqwest::Client,
}

impl Session {
    /// Creates a session.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(url: String, key: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| CliError::Network(format!("cannot create HTTP client: {e}")))?;
        Ok(Session { url, key, client })
    }

    /// Sends a planned request and maps the response to a [`CallOutcome`].
    ///
    /// # Errors
    /// Returns an error for network failures, unparseable responses, and
    /// non-403 error statuses.
    pub async fn execute(&self, plan: &PlannedRequest, dry_run: bool) -> Result<CallOutcome> {
        let url = format!("{}{}", self.url.trim_end_matches('/'), plan.path);
        let query: Vec<(&str, &str)> = plan
            .query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let mut request = if plan.method == "GET" {
            self.client.get(&url)
        } else {
            self.client.post(&url)
        };
        request = request.query(&query).bearer_auth(&self.key);
        if let Some(body) = &plan.body {
            request = request.json(body);
        }

        let response = request
            .send()
            .await
            .map_err(|e| CliError::Network(e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| CliError::Network(format!("cannot read response body: {e}")))?;

        let body: Value = if text.trim().is_empty() {
            Value::Null
        } else {
            match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(_) if status.is_success() => {
                    return Err(CliError::Json(format!(
                        "response is not JSON (HTTP {status})"
                    )));
                }
                Err(_) => {
                    return Err(CliError::Http {
                        status: status.as_u16(),
                        body: text,
                    });
                }
            }
        };

        map_response(status, body, dry_run)
    }
}

/// Maps an HTTP status and parsed body to a [`CallOutcome`]. Pure, so it can be tested.
///
/// # Errors
/// Returns an error for statuses other than 2xx and 403, and for a malformed
/// approval object.
pub fn map_response(status: StatusCode, body: Value, dry_run: bool) -> Result<CallOutcome> {
    if status.is_success() {
        if body.get("status").and_then(Value::as_str) == Some("needs-approval") {
            return approval_outcome(&body);
        }
        return Ok(if dry_run {
            CallOutcome::DryRun(body)
        } else {
            CallOutcome::Done(body)
        });
    }

    if status == StatusCode::FORBIDDEN {
        let reason = problem_detail(&body).unwrap_or_else(|| "policy denied".to_string());
        return Ok(CallOutcome::Denied(reason));
    }

    match problem_slug(&body) {
        Some(slug) => Err(CliError::Problem {
            slug,
            detail: problem_detail(&body).unwrap_or_else(|| "(no detail)".to_string()),
            status: status.as_u16(),
        }),
        None => Err(CliError::Http {
            status: status.as_u16(),
            body: body.to_string(),
        }),
    }
}

fn approval_outcome(body: &Value) -> Result<CallOutcome> {
    let approval = body
        .get("approval")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::Json("needs-approval response has no approval object".into()))?;
    let text = |key: &str| {
        approval
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let approval_id =
        text("approval_id").ok_or_else(|| CliError::Json("approval has no approval_id".into()))?;
    let url = text("url").ok_or_else(|| CliError::Json("approval has no url".into()))?;
    Ok(CallOutcome::NeedsApproval {
        approval_id,
        url,
        reason: text("reason").unwrap_or_default(),
        preview: approval.get("preview").cloned().unwrap_or(Value::Null),
    })
}

/// The last path segment of an RFC 9457 `type` URI: `.../problems/policy-denied`
/// becomes `policy-denied`.
fn problem_slug(body: &Value) -> Option<String> {
    let problem_type = body.get("type")?.as_str()?;
    let slug = problem_type.rsplit('/').next()?;
    (!slug.is_empty()).then(|| slug.to_string())
}

fn problem_detail(body: &Value) -> Option<String> {
    body.get("detail")
        .and_then(Value::as_str)
        .or_else(|| body.get("title").and_then(Value::as_str))
        .map(str::to_string)
}
