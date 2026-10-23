//! Credentials at rest (issue #142): AES-256-GCM under a key derived from the
//! server secret `LEDGERS_CONNECT_KEY`, bound to the connection id as
//! associated data, so a sealed token copied to another connection does not
//! open. Credentials are write-only: commands store the sealed form, read it
//! only to call the vendor, and never return it. The key and the plaintext
//! never reach `Debug`, logs or errors.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// The environment variable that holds the server secret.
pub const KEY_ENV: &str = "LEDGERS_CONNECT_KEY";

/// The minimum length of the server secret, in bytes.
pub const MIN_SECRET_BYTES: usize = 32;

/// Why a credential could not be sealed or opened. Carries no secret material.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// `LEDGERS_CONNECT_KEY` is missing or too short.
    #[error(
        "{KEY_ENV} must be set to at least {MIN_SECRET_BYTES} bytes before connections can hold credentials"
    )]
    KeyMissing,
    /// The sealed value is not in the `v1.<nonce>.<ciphertext>` form.
    #[error("the sealed credential is malformed")]
    Malformed,
    /// The value does not open under this key and connection id.
    #[error("the sealed credential does not open for this connection")]
    Refused,
    /// The operating system could not supply randomness.
    #[error("no randomness for a credential nonce")]
    Entropy,
}

/// The sealing key. `Debug` shows only that a key is present.
#[derive(Clone)]
pub struct ConnectKey {
    key: [u8; 32],
}

impl std::fmt::Debug for ConnectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectKey(<redacted>)")
    }
}

impl ConnectKey {
    /// Derives the sealing key from the server secret.
    ///
    /// # Errors
    /// [`SecretError::KeyMissing`] when the secret is shorter than
    /// [`MIN_SECRET_BYTES`].
    pub fn from_secret(secret: &str) -> Result<Self, SecretError> {
        if secret.len() < MIN_SECRET_BYTES {
            return Err(SecretError::KeyMissing);
        }
        let mut hasher = Sha256::new();
        hasher.update(b"ledgers-connect-credentials-v1:");
        hasher.update(secret.as_bytes());
        Ok(Self {
            key: hasher.finalize().into(),
        })
    }

    /// Seals `plaintext` for the connection `aad` (its id).
    ///
    /// # Errors
    /// [`SecretError::Entropy`] when no nonce can be drawn.
    pub fn seal(&self, plaintext: &str, aad: &str) -> Result<String, SecretError> {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).map_err(|_| SecretError::Entropy)?;
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| SecretError::Malformed)?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| SecretError::Refused)?;
        Ok(format!(
            "v1.{}.{}",
            URL_SAFE_NO_PAD.encode(nonce),
            URL_SAFE_NO_PAD.encode(ciphertext)
        ))
    }

    /// Opens a value made by [`ConnectKey::seal`] for the same connection id.
    ///
    /// # Errors
    /// [`SecretError::Malformed`] for a bad shape, [`SecretError::Refused`]
    /// when the key, the connection id or the bytes do not match.
    pub fn open(&self, sealed: &str, aad: &str) -> Result<String, SecretError> {
        let mut parts = sealed.split('.');
        if parts.next() != Some("v1") {
            return Err(SecretError::Malformed);
        }
        let nonce = URL_SAFE_NO_PAD
            .decode(parts.next().ok_or(SecretError::Malformed)?)
            .map_err(|_| SecretError::Malformed)?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(parts.next().ok_or(SecretError::Malformed)?)
            .map_err(|_| SecretError::Malformed)?;
        if parts.next().is_some() || nonce.len() != 12 {
            return Err(SecretError::Malformed);
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| SecretError::Malformed)?;
        let plain = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| SecretError::Refused)?;
        String::from_utf8(plain).map_err(|_| SecretError::Refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef-test-only";

    fn key() -> ConnectKey {
        ConnectKey::from_secret(SECRET).expect("long enough")
    }

    #[test]
    fn round_trips_for_the_same_connection() {
        let sealed = key().seal("cfat_example_token", "conn-1").unwrap();
        assert!(sealed.starts_with("v1."));
        assert!(!sealed.contains("cfat_example_token"));
        assert_eq!(key().open(&sealed, "conn-1").unwrap(), "cfat_example_token");
    }

    #[test]
    fn a_sealed_value_does_not_open_for_another_connection() {
        let sealed = key().seal("token", "conn-1").unwrap();
        assert_eq!(key().open(&sealed, "conn-2"), Err(SecretError::Refused));
    }

    #[test]
    fn a_different_server_secret_refuses() {
        let sealed = key().seal("token", "conn-1").unwrap();
        let other = ConnectKey::from_secret("another-server-secret-that-is-long-enough").unwrap();
        assert_eq!(other.open(&sealed, "conn-1"), Err(SecretError::Refused));
    }

    #[test]
    fn a_tampered_value_refuses() {
        let sealed = key().seal("token", "conn-1").unwrap();
        let mut bytes = sealed.into_bytes();
        let last = bytes.len() - 2;
        bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(bytes).unwrap();
        assert_eq!(key().open(&tampered, "conn-1"), Err(SecretError::Refused));
    }

    #[test]
    fn short_secrets_are_refused() {
        assert!(matches!(
            ConnectKey::from_secret("short"),
            Err(SecretError::KeyMissing)
        ));
    }

    #[test]
    fn each_seal_uses_a_fresh_nonce() {
        let a = key().seal("token", "conn-1").unwrap();
        let b = key().seal("token", "conn-1").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn debug_never_shows_the_key() {
        let shown = format!("{:?}", key());
        assert!(!shown.contains(SECRET));
        assert!(shown.contains("redacted"));
    }
}
