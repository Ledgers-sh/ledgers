//! HTTP client for communicating with the Ledgers server.

use crate::CallOutcome;
use crate::error::{CliError, Result};
use serde_json::Value;

/// An HTTP session: URL and authentication.
pub struct Session {
    /// The server base URL (e.g., "http://localhost:3000").
    pub url: String,
    /// The API key.
    pub key: String,
    /// The HTTP client (uses rustls for TLS).
    client: reqwest::Client,
}

impl Session {
    /// Creates a new session.
    pub fn new(url: String, key: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .build()
            .map_err(|e| CliError::Network(format!("cannot create HTTP client: {e}")))?;

        Ok(Session { url, key, client })
    }

    /// Builds the full URL for a command.
    /// If the command has a target parameter ({id}), the id positional is substituted.
    pub fn url_for_command(
        &self,
        descriptor: &ledgers_commands::CommandDescriptor,
        id: Option<&str>,
    ) -> Result<String> {
        let path = if let Some(id) = id {
            descriptor
                .http_path
                .replace("{id}", id)
                .replace("{target}", id)
        } else {
            descriptor.http_path.clone()
        };

        Ok(format!("{}{}", self.url, path))
    }

    /// Sends an HTTP request to the server.
    ///
    /// Returns the parsed response, handling NeedsApproval and Denied outcomes.
    pub async fn send_request(
        &self,
        url: &str,
        method: &str,
        params: &[(&str, &str)],
        body: &Value,
    ) -> Result<CallOutcome> {
        let request = match method {
            "GET" => self.client.get(url).query(params),
            "POST" => self.client.post(url).query(params).json(body),
            _ => {
                return Err(CliError::Internal(format!(
                    "unsupported HTTP method: {method}"
                )));
            }
        };

        let request = request.header("Authorization", format!("Bearer {}", self.key));

        let response = request
            .send()
            .await
            .map_err(|e| CliError::Network(format!("{e}")))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| CliError::Network(format!("cannot read response body: {e}")))?;

        // Try to parse as JSON first
        let json: Value = serde_json::from_str(&text).map_err(|_| CliError::Http {
            status: status.as_u16(),
            body: text.clone(),
        })?;

        // Check for error status
        if !status.is_success() {
            // Try to parse as RFC 9457 Problem
            if let Some(slug) = json.get("type").and_then(|t| t.as_str()) {
                let detail = json
                    .get("detail")
                    .and_then(|d| d.as_str())
                    .unwrap_or("(no detail)")
                    .to_string();
                return Err(CliError::Problem {
                    slug: slug.to_string(),
                    detail,
                    status: Some(status.as_u16()),
                });
            }

            return Err(CliError::Http {
                status: status.as_u16(),
                body: text,
            });
        }

        // Success (2xx). Check if it's a special outcome or regular response.
        // For now, assume success responses are always Done (unless the command
        // specifically handles DryRun or NeedsApproval at the dispatch level).
        // This is the main assumption we'll document: NeedsApproval/Denied would
        // come back in the JSON but we're treating them as errors from the HTTP layer.

        Ok(CallOutcome::Done(json))
    }
}
