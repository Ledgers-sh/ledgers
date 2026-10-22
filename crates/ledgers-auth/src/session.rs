//! People session primitives: opaque server-side sessions in short-lived,
//! httpOnly cookies (issue #12).
//!
//! The cookie carries an opaque random id. The server keeps only its SHA-256
//! (see [`session_key_hash`]), so a leaked table cannot be replayed as
//! cookies. `SameSite=Lax` keeps the cookie on the top-level redirect back
//! from the sign-in, and the API stays unreachable to other sites' scripts
//! because CORS never allows credentials for origins it does not name.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// The session cookie name. The `__Host-` prefix pins it to this exact host,
/// `Path=/`, and `Secure`.
pub const SESSION_COOKIE: &str = "__Host-ledgers-session";

/// The short-lived sign-in transaction cookie (state, nonce, PKCE verifier).
pub const LOGIN_COOKIE: &str = "__Host-ledgers-login";

/// Absolute lifetime of a person session, from sign-in. Sessions end here
/// whatever the activity.
pub const SESSION_TTL_SECONDS: u64 = 12 * 3600;

/// Idle lifetime. Every use slides the expiry forward by this much, never past
/// the absolute lifetime.
pub const IDLE_TTL_SECONDS: i64 = 30 * 60;

/// How long a sign-in may take between starting it and the callback.
pub const LOGIN_TTL_SECONDS: i64 = 10 * 60;

/// The roles that may use the company. Any other role, or none, is refused
/// with 403. The open build has one owner, so `owner` is the normal case;
/// `admin` is accepted so a company can grant access beyond the owner.
pub const COMPANY_ROLES: &[&str] = &["owner", "admin"];

/// Whether an account's roles entitle it to use the company.
#[must_use]
pub fn may_use_company(roles: &[String]) -> bool {
    roles
        .iter()
        .any(|role| COMPANY_ROLES.contains(&role.as_str()))
}

/// The storage key of a session: base64url SHA-256 of the cookie id.
#[must_use]
pub fn session_key_hash(session_id: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(session_id.as_bytes()))
}

/// Builder for the Set-Cookie header values of person sessions.
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
    /// Format: `__Host-ledgers-session=<id>; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=<ttl>`
    #[must_use]
    pub fn build(&self) -> String {
        format!(
            "{SESSION_COOKIE}={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
            self.session_id, SESSION_TTL_SECONDS
        )
    }

    /// Builds a clearing cookie (Max-Age=0) to logout.
    #[must_use]
    pub fn clear() -> String {
        format!("{SESSION_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
    }
}

/// The Set-Cookie value that stores the sign-in transaction.
#[must_use]
pub fn login_cookie(sealed: &str) -> String {
    format!(
        "{LOGIN_COOKIE}={sealed}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={LOGIN_TTL_SECONDS}"
    )
}

/// The Set-Cookie value that clears the sign-in transaction.
#[must_use]
pub fn clear_login_cookie() -> String {
    format!("{LOGIN_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
}
