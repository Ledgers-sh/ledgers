//! Approval service configuration.

use std::time::Duration;

/// Configuration for the approvals service.
#[derive(Clone, Debug)]
pub struct ApprovalConfig {
    /// How long an approval request remains pending before expiring.
    /// Default: 24 hours.
    pub ttl: Duration,
    /// How long a nonce remains valid for step-up (e.g., 2 minutes).
    /// Default: 2 minutes.
    pub nonce_ttl: Duration,
    /// The `WebAuthn` RP ID (Relying Party ID, e.g., <https://ledgers.example.com>).
    pub rp_id: String,
    /// The `WebAuthn` origin (e.g., <https://ledgers.example.com>).
    pub origin: String,
    /// The base URL for approval links (e.g., <https://ledgers.example.com>).
    pub base_url: String,
}

impl ApprovalConfig {
    /// Creates a new config with custom values.
    #[must_use]
    pub fn new(rp_id: String, origin: String, base_url: String) -> Self {
        ApprovalConfig {
            ttl: Duration::from_hours(24),
            nonce_ttl: Duration::from_mins(2),
            rp_id,
            origin,
            base_url,
        }
    }

    /// Sets the request TTL (time-to-live before expiry).
    #[must_use]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Sets the nonce TTL (time-to-live before the nonce expires).
    #[must_use]
    pub fn with_nonce_ttl(mut self, nonce_ttl: Duration) -> Self {
        self.nonce_ttl = nonce_ttl;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = ApprovalConfig::new(
            "example.com".to_string(),
            "https://example.com".to_string(),
            "https://example.com".to_string(),
        );
        assert_eq!(config.rp_id, "example.com");
        assert_eq!(config.origin, "https://example.com");
        assert_eq!(config.ttl.as_secs(), 24 * 60 * 60);
        assert_eq!(config.nonce_ttl.as_secs(), 2 * 60);
    }

    #[test]
    fn config_custom_ttl() {
        let config = ApprovalConfig::new(
            "example.com".to_string(),
            "https://example.com".to_string(),
            "https://example.com".to_string(),
        )
        .with_ttl(Duration::from_secs(3600));

        assert_eq!(config.ttl.as_secs(), 3600);
    }
}
