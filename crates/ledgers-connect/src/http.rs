//! Outbound calls to vendors (issue #142). Every call goes through the
//! harness `HttpClient` port, and this module adds the connector rules:
//!
//! - **Fixed hosts.** A connector names the hosts it may call. A URL is sent
//!   only if it is `https`, has no userinfo or non-default port, and its host
//!   is exactly one of those names. A redirect is never followed; the
//!   harness client does not follow them either.
//! - **Bounded.** Each exchange carries an [`HttpPolicy`] (timeout and body
//!   cap). Only `429` and `5xx` answers and transport failures are retried,
//!   at most [`MAX_ATTEMPTS`] times with exponential backoff.
//! - **Secret-free errors.** Errors name the host and status, never the URL's
//!   query or the credential header.
//!
//! Vendor text is untrusted data: the callers keep it out of prompts and
//! only store it (see [`sanitize_vendor_text`]).

use bytes::Bytes;
use cratefield_core::{HttpClient, HttpPolicy};
use http::header;
use std::sync::Arc;
use std::time::Duration;

/// Attempts per request, the first included.
pub const MAX_ATTEMPTS: u32 = 3;

/// The longest vendor answer read, in bytes.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// The per-request deadline.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// Waits between attempts. The server implements it on the runtime's timer;
/// tests record the delays instead of waiting.
#[async_trait::async_trait]
pub trait Pause: Send + Sync {
    /// Waits for `delay`.
    async fn pause(&self, delay: Duration);
}

/// Why a vendor call failed. Never carries a credential.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VendorError {
    /// The URL is not an allowed `https` URL on one of the connector's hosts.
    #[error("the connector may not call {0}")]
    HostNotAllowed(String),
    /// The vendor refused the credential (401 or 403). The scope named in the
    /// connector's documentation is the usual cause.
    #[error("{host} refused the token ({status}); check the token's scopes")]
    Refused { host: String, status: u16 },
    /// The vendor answered with an unexpected status after the retries.
    #[error("{host} answered {status}")]
    Status { host: String, status: u16 },
    /// The request failed before an answer (timeout, connection, size).
    #[error("{host} could not be reached: {detail}")]
    Transport { host: String, detail: String },
    /// The answer is not the JSON shape the connector reads.
    #[error("the answer from {host} is not as documented: {detail}")]
    Malformed { host: String, detail: String },
    /// The vendor does not expose this data to this account, token or plan.
    #[error("not available for this account: {0}")]
    NotAvailable(String),
}

/// Checks that `url` is an `https` URL on one of `allowed_hosts`, with no
/// userinfo and no port other than 443.
///
/// # Errors
/// [`VendorError::HostNotAllowed`] for anything else.
pub fn check_url(url: &str, allowed_hosts: &[&str]) -> Result<http::Uri, VendorError> {
    let refused = || VendorError::HostNotAllowed(redact(url));
    let uri: http::Uri = url.parse().map_err(|_| refused())?;
    if uri.scheme_str() != Some("https") {
        return Err(refused());
    }
    let authority = uri.authority().ok_or_else(refused)?;
    if authority.as_str().contains('@') {
        return Err(refused());
    }
    match authority.port_u16() {
        None | Some(443) => {}
        Some(_) => return Err(refused()),
    }
    let host = authority.host();
    if !allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        return Err(refused());
    }
    Ok(uri)
}

/// The scheme, host and path of a URL, without its query, for messages.
fn redact(url: &str) -> String {
    match url.parse::<http::Uri>() {
        Ok(uri) => {
            let scheme = uri.scheme_str().unwrap_or("?");
            let host = uri.host().unwrap_or("?");
            format!("{scheme}://{host}{}", uri.path())
        }
        Err(_) => "an unparseable URL".to_owned(),
    }
}

/// The vendor HTTP client: the harness port plus the connector rules.
#[derive(Clone)]
pub struct VendorHttp {
    client: Arc<dyn HttpClient>,
    pause: Arc<dyn Pause>,
}

impl std::fmt::Debug for VendorHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VendorHttp").finish_non_exhaustive()
    }
}

impl VendorHttp {
    /// A client that sends through `client` and waits with `pause`.
    #[must_use]
    pub fn new(client: Arc<dyn HttpClient>, pause: Arc<dyn Pause>) -> Self {
        Self { client, pause }
    }

    /// GETs `url` (on one of `allowed_hosts`) with `headers` and reads the
    /// answer as JSON.
    ///
    /// # Errors
    /// See [`VendorError`]. A 401 or 403 is [`VendorError::Refused`] and is
    /// not retried.
    pub async fn get_json(
        &self,
        allowed_hosts: &[&str],
        url: &str,
        headers: &[(&str, String)],
    ) -> Result<serde_json::Value, VendorError> {
        self.request_json(http::Method::GET, allowed_hosts, url, headers, None)
            .await
    }

    /// POSTs `body` as JSON to `url` (on one of `allowed_hosts`) and reads the
    /// answer as JSON. Used for the GraphQL analytics call.
    ///
    /// # Errors
    /// As [`VendorHttp::get_json`].
    pub async fn post_json(
        &self,
        allowed_hosts: &[&str],
        url: &str,
        headers: &[(&str, String)],
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, VendorError> {
        self.request_json(http::Method::POST, allowed_hosts, url, headers, Some(body))
            .await
    }

    async fn request_json(
        &self,
        method: http::Method,
        allowed_hosts: &[&str],
        url: &str,
        headers: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, VendorError> {
        let uri = check_url(url, allowed_hosts)?;
        let host = uri.host().unwrap_or_default().to_owned();
        let payload =
            match body {
                Some(value) => Bytes::from(serde_json::to_vec(value).map_err(|err| {
                    VendorError::Malformed {
                        host: host.clone(),
                        detail: err.to_string(),
                    }
                })?),
                None => Bytes::new(),
            };
        let mut attempt = 1;
        loop {
            let outcome = self
                .send_once(
                    &method,
                    &uri,
                    &host,
                    headers,
                    payload.clone(),
                    body.is_some(),
                )
                .await;
            match outcome {
                Ok(value) => return Ok(value),
                Err(err) if !retryable(&err) || attempt >= MAX_ATTEMPTS => return Err(err),
                Err(_) => {
                    self.pause.pause(backoff(attempt)).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn send_once(
        &self,
        method: &http::Method,
        uri: &http::Uri,
        host: &str,
        headers: &[(&str, String)],
        payload: Bytes,
        has_body: bool,
    ) -> Result<serde_json::Value, VendorError> {
        let mut builder = http::Request::builder()
            .method(method.clone())
            .uri(uri.clone())
            .header(header::ACCEPT, "application/json")
            .header(header::USER_AGENT, "ledgers-connect/0.1");
        if has_body {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        for (name, value) in headers {
            builder = builder.header(*name, value.as_str());
        }
        let mut request = builder
            .body(payload)
            .map_err(|err| VendorError::Transport {
                host: host.to_owned(),
                detail: err.to_string(),
            })?;
        request.extensions_mut().insert(HttpPolicy {
            max_response_bytes: MAX_BODY_BYTES,
            timeout: TIMEOUT,
        });
        let response = self
            .client
            .send(request)
            .await
            .map_err(|err| VendorError::Transport {
                host: host.to_owned(),
                detail: err.to_string(),
            })?;
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            return Err(VendorError::Refused {
                host: host.to_owned(),
                status,
            });
        }
        if !(200..300).contains(&status) {
            return Err(VendorError::Status {
                host: host.to_owned(),
                status,
            });
        }
        serde_json::from_slice(response.body()).map_err(|err| VendorError::Malformed {
            host: host.to_owned(),
            detail: err.to_string(),
        })
    }
}

fn retryable(err: &VendorError) -> bool {
    match err {
        VendorError::Transport { .. } => true,
        VendorError::Status { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500 * u64::from(2_u32.pow(attempt.saturating_sub(1))))
}

/// Vendor text (descriptions, names, line items) is data. This keeps it
/// printable, drops control and bidirectional-override characters, collapses
/// whitespace and caps the length. It never adds or removes digits, so an
/// amount or an invoice number in the text reads the same afterwards.
#[must_use]
pub fn sanitize_vendor_text(text: &str) -> String {
    const MAX_CHARS: usize = 200;
    let cleaned: String = text
        .chars()
        .filter(|c| !is_bidi_control(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(MAX_CHARS).collect()
}

fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOSTS: &[&str] = &["api.example.test"];

    #[test]
    fn allows_https_on_the_listed_host() {
        assert!(check_url("https://api.example.test/v1/x?y=1", HOSTS).is_ok());
        assert!(check_url("https://API.example.test:443/v1", HOSTS).is_ok());
    }

    #[test]
    fn refuses_other_hosts_schemes_ports_and_userinfo() {
        for bad in [
            "https://evil.example.test/v1",
            "https://api.example.test.evil.test/v1",
            "http://api.example.test/v1",
            "https://api.example.test:8443/v1",
            "https://user:pw@api.example.test/v1",
            "https://169.254.169.254/latest/meta-data",
            "https://localhost/",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(
                matches!(check_url(bad, HOSTS), Err(VendorError::HostNotAllowed(_))),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn refusal_messages_do_not_echo_the_query() {
        let err = check_url("https://evil.test/x?token=SECRET123", HOSTS).unwrap_err();
        assert!(!err.to_string().contains("SECRET123"));
    }

    #[test]
    fn backoff_doubles() {
        assert_eq!(backoff(1), Duration::from_millis(500));
        assert_eq!(backoff(2), Duration::from_millis(1000));
    }

    #[test]
    fn vendor_text_loses_control_and_bidi_characters_but_keeps_digits() {
        let raw = "Workers\u{202E}  Paid\n\tplan 12.50\u{0007}";
        assert_eq!(sanitize_vendor_text(raw), "Workers Paid plan 12.50");
        assert_eq!(sanitize_vendor_text(&"x".repeat(500)).chars().count(), 200);
    }
}
