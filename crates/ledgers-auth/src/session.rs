//! People session primitives: short-lived, secure cookies.
//!
//! Sessions for authenticated persons use short-lived, httpOnly SameSite=Strict
//! cookies with a `__Host-` prefix for maximum security.

/// Time-to-live for a person session cookie.
///
/// Chosen to be short enough to limit the window of a compromised session,
/// long enough to avoid excessive re-authentication UX. Persons must
/// re-authenticate for high-risk approvals (step-up auth).
pub const SESSION_TTL_SECONDS: u64 = 12 * 3600; // 12 hours

/// Builder for the Set-Cookie header value for person sessions.
///
/// Produces a secure, httpOnly, SameSite=Strict cookie with `__Host-` prefix.
pub struct SessionBuilder {
    session_id: String,
}

impl SessionBuilder {
    /// Creates a new session cookie builder with the given session id.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
        }
    }

    /// Builds the Set-Cookie header value for this session.
    ///
    /// Format: `__Host-ledgers-session=<id>; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=<ttl>`
    #[must_use]
    pub fn build(&self) -> String {
        format!(
            "__Host-ledgers-session={}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={}",
            self.session_id, SESSION_TTL_SECONDS
        )
    }

    /// Builds a clearing cookie (Max-Age=0) to logout.
    #[must_use]
    pub fn clear() -> String {
        "__Host-ledgers-session=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0".to_string()
    }
}
