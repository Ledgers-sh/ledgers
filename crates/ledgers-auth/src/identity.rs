//! Identity provider trait for person authentication.
//!
//! [`IdentityProvider`] is the extension point for the cloud to plug in
//! custom person authentication (SSO, etc.). The open build will verify a
//! passkey session (via `cratefield-auth-passkeys` wiring per ADR 0005);
//! the cloud implements custom SSO here.

use async_trait::async_trait;
use ledgers_commands::{CommandError, Principal};

/// A person session assertion or credential for SSO/custom auth.
///
/// Implementations define what form this takes (JWT, SAML token, etc.).
pub type SessionAssertion = String;

/// The identity provider trait: maps an authenticated person session to a principal.
///
/// Implementations verify a person session assertion and return [`Principal::Person`].
/// The open build receives a passkey session assertion (via `cratefield-auth-passkeys`
/// wiring per ADR 0005) and verifies it. The cloud plugs SSO in here.
#[async_trait(?Send)]
pub trait IdentityProvider: Send + Sync {
    /// Authenticates a person session assertion, returning a person principal
    /// or an unauthenticated error.
    async fn authenticate(&self, assertion: &SessionAssertion) -> Result<Principal, CommandError>;
}
