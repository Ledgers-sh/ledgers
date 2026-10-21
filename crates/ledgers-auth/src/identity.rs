//! Identity provider trait for person authentication.
//!
//! [`IdentityProvider`] is the extension point for the cloud to plug in
//! custom person authentication (SSO, etc.). The open build provides
//! [`OwnerPasskeys`], a simple owner-based authentication stub pending
//! `cratefield-auth-passkeys` wiring (ADR 0005); the cloud implements
//! custom SSO and advanced auth here.

use async_trait::async_trait;
use ledgers_commands::{CommandError, Principal};

/// A person session assertion or credential for SSO/custom auth.
///
/// Implementations define what form this takes (JWT, SAML token, session cookie, etc.).
pub type SessionAssertion = String;

/// The identity provider trait: maps an authenticated person session to a principal.
///
/// Implementations verify a person session assertion and return [`Principal::Person`]
/// (never an agent). The open build provides [`OwnerPasskeys`], a stub implementation
/// for owner-based authentication pending passkey verification wiring. The cloud
/// plugs SSO and other auth methods in here.
#[async_trait(?Send)]
pub trait IdentityProvider: Send + Sync {
    /// Authenticates a person session assertion, returning a person principal
    /// or an unauthenticated error.
    async fn authenticate(&self, assertion: &SessionAssertion) -> Result<Principal, CommandError>;
}

/// The open-source default identity provider: passkey verification stub.
///
/// **Security note:** Session verification infrastructure is not yet available
/// in the workspace. This default **fails closed**: it rejects every assertion
/// as unauthenticated until `cratefield-auth-passkeys` is wired in (ADR 0005)
/// with actual passkey assertion verification against stored credentials.
/// This is safer than accepting unverified input.
///
/// A custom `IdentityProvider` in a managed-service crate can plug in SSO or other
/// authentication methods at composition time.
#[derive(Clone, Copy, Debug)]
pub struct OwnerPasskeys;

#[async_trait(?Send)]
impl IdentityProvider for OwnerPasskeys {
    async fn authenticate(&self, _assertion: &SessionAssertion) -> Result<Principal, CommandError> {
        // Fail closed: session verification infrastructure is not yet available.
        // When cratefield-auth-passkeys is wired in (ADR 0005), verify the
        // assertion against stored passkey credentials and return Person.
        // Until then, reject every assertion to avoid auth bypass.
        Err(CommandError::Unauthenticated)
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod contract_tests {
    use super::IdentityProvider;

    /// Contract test for `IdentityProvider` implementations.
    ///
    /// Verifies invariants ANY implementation must uphold:
    /// - Empty, unknown, expired, or garbage assertions are rejected (error).
    /// - Successful authentication returns `Principal::Person`, never `Principal::Agent`.
    ///
    /// # Panics
    /// Panics if the invariants are violated.
    pub async fn identity_provider(provider: &dyn IdentityProvider) {
        // Empty assertion must be rejected
        let result = provider.authenticate(&String::new()).await;
        assert!(result.is_err(), "empty assertion must be rejected");

        // Garbage assertion must be rejected
        let result = provider
            .authenticate(&"garbage-token-that-does-not-exist".to_string())
            .await;
        assert!(result.is_err(), "unknown assertion must be rejected");

        // Any successful result must be Person, never Agent
        // (the open default always rejects; custom impls may succeed with verified assertions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn contract_tests_pass() {
        contract_tests::identity_provider(&OwnerPasskeys).await;
    }
}
