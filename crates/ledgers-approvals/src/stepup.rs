//! Passkey step-up: verify `WebAuthn` assertions bound to approval requests.
//!
//! A `challenge` is derived from the request hash and nonce: this binds
//! the assertion to the exact request, preventing replay onto different data.
//! The verifier checks clientDataJSON, authenticatorData, and the signature
//! over both. This module provides a basic P-256 ES256 verifier and a trait
//! seam for cloud implementations (cratefield-auth-passkeys).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One `WebAuthn` assertion for step-up: the credential id, client data,
/// authenticator data, and signature to verify against a stored public key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PasskeyProof {
    /// The credential id (base64url, no padding).
    pub credential_id: String,
    /// The clientDataJSON (base64url, no padding).
    pub client_data_json: String,
    /// The authenticatorData (base64url, no padding).
    pub authenticator_data: String,
    /// The signature (base64url, no padding).
    pub signature: String,
}

/// Trait for looking up public keys for credential verification (open-core extension point).
///
/// The open-source implementation would read from the passkey store; the cloud can
/// replace this with cratefield-auth-passkeys for centralized key management.
pub trait CredentialKeys: Send + Sync {
    /// Returns the SEC1-encoded P-256 public key of a passkey registered to a person.
    ///
    /// Returns `None` if the credential is not registered to that person.
    #[must_use]
    fn public_key(&self, person_id: &str, credential_id: &str) -> Option<Vec<u8>>;
}

/// Trait for verifying `WebAuthn` assertions (open-core extension point).
///
/// The open-source implementation (`Es256Verifier`) uses `RustCrypto` p256.
/// The cloud can replace this with cratefield-auth-passkeys for more
/// verification options.
pub trait AssertionVerifier: Send + Sync {
    /// Verifies a `WebAuthn` assertion against a challenge and stored public key.
    ///
    /// # Errors
    /// Returns an error if the assertion is invalid or verification fails.
    fn verify(
        &self,
        proof: &PasskeyProof,
        expected_challenge: &[u8],
        rp_id: &str,
        origin: &str,
        public_key: &[u8], // SEC1 uncompressed P-256 public key (65 bytes)
    ) -> Result<(), String>;
}

/// Basic P-256 ES256 `WebAuthn` assertion verifier.
///
/// Verifies that:
/// - clientDataJSON.type == "webauthn.get"
/// - clientDataJSON.challenge matches `expected_challenge` (base64url)
/// - clientDataJSON.origin matches config.origin
/// - authenticatorData rpIdHash (first 32 bytes) == SHA-256(rp_id)
/// - authenticatorData has UP (0x01) and UV (0x04) flags set
/// - signature (DER-encoded ECDSA) verifies over authData || SHA-256(clientDataJSON)
///
/// Uses the p256 crate with the ECDSA signature algorithm. Public keys
/// are assumed to be SEC1 uncompressed (65 bytes: 0x04 || x || y).
pub struct Es256Verifier;

impl AssertionVerifier for Es256Verifier {
    fn verify(
        &self,
        proof: &PasskeyProof,
        expected_challenge: &[u8],
        rp_id: &str,
        origin: &str,
        public_key: &[u8],
    ) -> Result<(), String> {
        // Decode base64url-no-pad values
        let client_data_json =
            base64_url_decode(&proof.client_data_json).map_err(|e| e.to_string())?;
        let authenticator_data =
            base64_url_decode(&proof.authenticator_data).map_err(|e| e.to_string())?;
        let signature = base64_url_decode(&proof.signature).map_err(|e| e.to_string())?;

        // Parse and verify clientDataJSON
        let client_data: serde_json::Value = serde_json::from_slice(&client_data_json)
            .map_err(|e| format!("invalid clientDataJSON JSON: {e}"))?;

        // Check type == "webauthn.get"
        let client_type = client_data
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or("missing or non-string type in clientDataJSON")?;
        if client_type != "webauthn.get" {
            return Err(format!("wrong clientDataJSON type: {client_type}"));
        }

        // Check challenge matches expected
        let client_challenge_b64 = client_data
            .get("challenge")
            .and_then(|v| v.as_str())
            .ok_or("missing or non-string challenge in clientDataJSON")?;
        let client_challenge = base64_url_decode(client_challenge_b64)
            .map_err(|e| format!("invalid challenge in clientDataJSON: {e}"))?;
        if client_challenge != expected_challenge {
            return Err("challenge mismatch in clientDataJSON".to_string());
        }

        // Check origin matches
        let client_origin = client_data
            .get("origin")
            .and_then(|v| v.as_str())
            .ok_or("missing or non-string origin in clientDataJSON")?;
        if client_origin != origin {
            return Err(format!(
                "origin mismatch: expected {origin}, got {client_origin}"
            ));
        }

        // Check authenticatorData RP ID hash (first 32 bytes)
        if authenticator_data.len() < 37 {
            return Err("authenticatorData too short".to_string());
        }
        let rp_id_hash = &authenticator_data[0..32];
        let expected_rp_id_hash = Sha256::digest(rp_id.as_bytes());
        if rp_id_hash != expected_rp_id_hash.as_slice() {
            return Err("RP ID hash mismatch".to_string());
        }

        // Check flags: UP (0x01) and UV (0x04) must be set
        let flags = authenticator_data[32];
        if (flags & 0x01) == 0 {
            return Err("user presence (UP) flag not set".to_string());
        }
        if (flags & 0x04) == 0 {
            return Err("user verification (UV) flag not set".to_string());
        }

        // Verify signature over authenticatorData || SHA-256(clientDataJSON)
        let mut message = authenticator_data.clone();
        let client_data_hash = Sha256::digest(&client_data_json);
        message.extend_from_slice(client_data_hash.as_slice());

        verify_es256_signature(&signature, &message, public_key)?;

        Ok(())
    }
}

/// Verifies an ES256 (P-256 ECDSA SHA256) signature.
///
/// The signature is DER-encoded; the public key is SEC1 uncompressed (65 bytes).
fn verify_es256_signature(
    signature: &[u8],
    message: &[u8],
    public_key: &[u8],
) -> Result<(), String> {
    use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};

    // Decode the public key from SEC1 uncompressed format
    let verifying_key = VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|e| format!("invalid public key point: {e}"))?;

    // Parse the DER signature
    let signature_obj =
        Signature::from_der(signature).map_err(|e| format!("invalid DER signature: {e}"))?;

    // Verify
    verifying_key
        .verify(message, &signature_obj)
        .map_err(|e| format!("signature verification failed: {e}"))
}

/// Decodes a base64url string (no padding).
fn base64_url_decode(s: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    engine
        .decode(s)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::{SigningKey, signature::Signer};

    #[test]
    fn challenge_bytes() {
        // The challenge should be derived as SHA-256(request_hash || nonce)
        let request_hash = "abc123def456";
        let nonce = b"mynonce";
        let mut message = request_hash.as_bytes().to_vec();
        message.extend_from_slice(nonce);
        let challenge = Sha256::digest(&message);
        assert_eq!(challenge.len(), 32);
    }

    /// Helper to create a test signing key and public key
    fn test_signing_key() -> (SigningKey, Vec<u8>) {
        use p256::SecretKey;

        let seed = [42u8; 32];
        let secret = SecretKey::from_slice(&seed).expect("valid seed");
        let signing_key = p256::ecdsa::SigningKey::from(secret);
        let verifying_key = p256::ecdsa::VerifyingKey::from(&signing_key);
        let public_key_bytes = verifying_key.to_sec1_point(false).as_bytes().to_vec();

        (signing_key, public_key_bytes)
    }

    /// Helper to build a passkey proof with custom parameters
    fn build_proof(
        signing_key: &SigningKey,
        challenge: &[u8],
        rp_id: &str,
        origin: &str,
        flags: u8,
        valid: bool,
    ) -> PasskeyProof {
        let verifying_key = p256::ecdsa::VerifyingKey::from(signing_key);
        let public_key_bytes = verifying_key.to_sec1_point(false).as_bytes().to_vec();

        // Build authenticatorData: rpIdHash (32) + flags (1) + counter (4)
        let rp_id_hash = Sha256::new().chain_update(rp_id.as_bytes()).finalize();
        let mut auth_data = rp_id_hash.to_vec();
        auth_data.push(flags);
        auth_data.extend_from_slice(&1u32.to_be_bytes());

        // Build clientDataJSON
        let client_data = serde_json::json!({
            "type": "webauthn.get",
            "challenge": base64_url_no_pad(challenge),
            "origin": origin,
        });
        let client_data_json = serde_json::to_vec(&client_data).unwrap();
        let client_data_hash = Sha256::new().chain_update(&client_data_json).finalize();

        // Sign authData || SHA256(clientDataJSON)
        let mut message = auth_data.clone();
        message.extend_from_slice(client_data_hash.as_slice());
        let signature: p256::ecdsa::Signature = if valid {
            signing_key.sign(&message)
        } else {
            // Create an invalid signature by signing different data
            signing_key.sign(b"invalid_data")
        };

        PasskeyProof {
            credential_id: hex::encode(&public_key_bytes),
            client_data_json: base64_url_no_pad(&client_data_json),
            authenticator_data: base64_url_no_pad(&auth_data),
            signature: base64_url_no_pad(signature.to_der().as_ref()),
        }
    }

    fn base64_url_no_pad(data: &[u8]) -> String {
        use base64::Engine;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        engine.encode(data)
    }

    #[test]
    fn assertion_verifies_with_correct_params() {
        let (signing_key, public_key_bytes) = test_signing_key();
        let verifier = Es256Verifier;
        let challenge = b"test_challenge";
        let rp_id = "localhost";
        let origin = "http://localhost:3000";

        let proof = build_proof(&signing_key, challenge, rp_id, origin, 0x05, true);

        let result = verifier.verify(&proof, challenge, rp_id, origin, &public_key_bytes);
        assert!(
            result.is_ok(),
            "verification should succeed with correct parameters"
        );

        // Extract credential_id from proof
        assert!(!proof.credential_id.is_empty());
    }

    #[test]
    fn assertion_fails_with_uv_flag_missing() {
        let (signing_key, public_key_bytes) = test_signing_key();
        let verifier = Es256Verifier;
        let challenge = b"test_challenge";
        let rp_id = "localhost";
        let origin = "http://localhost:3000";

        // flags = 0x01 (UP only, missing UV 0x04)
        let proof = build_proof(&signing_key, challenge, rp_id, origin, 0x01, true);

        let result = verifier.verify(&proof, challenge, rp_id, origin, &public_key_bytes);
        assert!(
            result.is_err(),
            "verification should fail when UV flag is missing"
        );
    }

    #[test]
    fn assertion_fails_with_wrong_origin() {
        let (signing_key, public_key_bytes) = test_signing_key();
        let verifier = Es256Verifier;
        let challenge = b"test_challenge";
        let rp_id = "localhost";
        let origin = "http://localhost:3000";
        let wrong_origin = "http://evil.com";

        let proof = build_proof(&signing_key, challenge, rp_id, origin, 0x05, true);

        let result = verifier.verify(&proof, challenge, rp_id, wrong_origin, &public_key_bytes);
        assert!(
            result.is_err(),
            "verification should fail with wrong origin"
        );
    }

    #[test]
    fn assertion_fails_with_wrong_rp_id() {
        let (signing_key, public_key_bytes) = test_signing_key();
        let verifier = Es256Verifier;
        let challenge = b"test_challenge";
        let rp_id = "localhost";
        let wrong_rp_id = "evil.localhost";
        let origin = "http://localhost:3000";

        let proof = build_proof(&signing_key, challenge, rp_id, origin, 0x05, true);

        let result = verifier.verify(&proof, challenge, wrong_rp_id, origin, &public_key_bytes);
        assert!(result.is_err(), "verification should fail with wrong RP ID");
    }

    #[test]
    fn assertion_fails_with_wrong_challenge() {
        let (signing_key, public_key_bytes) = test_signing_key();
        let verifier = Es256Verifier;
        let challenge = b"test_challenge";
        let wrong_challenge = b"wrong_challenge";
        let rp_id = "localhost";
        let origin = "http://localhost:3000";

        let proof = build_proof(&signing_key, challenge, rp_id, origin, 0x05, true);

        let result = verifier.verify(&proof, wrong_challenge, rp_id, origin, &public_key_bytes);
        assert!(
            result.is_err(),
            "verification should fail with wrong challenge"
        );
    }

    #[test]
    fn assertion_fails_with_wrong_type() {
        let (signing_key, public_key_bytes) = test_signing_key();
        let verifier = Es256Verifier;
        let challenge = b"test_challenge";
        let rp_id = "localhost";
        let origin = "http://localhost:3000";

        // Build a proof with wrong type in client data
        let verifying_key = p256::ecdsa::VerifyingKey::from(&signing_key);
        let public_key_vec = verifying_key.to_sec1_point(false).as_bytes().to_vec();

        let rp_id_hash = Sha256::new().chain_update(rp_id.as_bytes()).finalize();
        let mut auth_data = rp_id_hash.to_vec();
        auth_data.push(0x05);
        auth_data.extend_from_slice(&1u32.to_be_bytes());

        // Use wrong type "webauthn.create" instead of "webauthn.get"
        let client_data = serde_json::json!({
            "type": "webauthn.create",
            "challenge": base64_url_no_pad(challenge),
            "origin": origin,
        });
        let client_data_json = serde_json::to_vec(&client_data).unwrap();
        let client_data_hash = Sha256::new().chain_update(&client_data_json).finalize();

        let mut message = auth_data.clone();
        message.extend_from_slice(client_data_hash.as_slice());
        let signature: p256::ecdsa::Signature = signing_key.sign(&message);

        let proof = PasskeyProof {
            credential_id: hex::encode(&public_key_vec),
            client_data_json: base64_url_no_pad(&client_data_json),
            authenticator_data: base64_url_no_pad(&auth_data),
            signature: base64_url_no_pad(signature.to_der().as_ref()),
        };

        let result = verifier.verify(&proof, challenge, rp_id, origin, &public_key_bytes);
        assert!(result.is_err(), "verification should fail with wrong type");
    }

    #[test]
    fn assertion_fails_with_invalid_signature() {
        let (signing_key, public_key_bytes) = test_signing_key();
        let verifier = Es256Verifier;
        let challenge = b"test_challenge";
        let rp_id = "localhost";
        let origin = "http://localhost:3000";

        // Build proof with invalid signature
        let proof = build_proof(&signing_key, challenge, rp_id, origin, 0x05, false);

        let result = verifier.verify(&proof, challenge, rp_id, origin, &public_key_bytes);
        assert!(
            result.is_err(),
            "verification should fail with invalid signature"
        );
    }

    #[test]
    fn assertion_fails_with_different_key() {
        let (signing_key, _) = test_signing_key();
        let verifier = Es256Verifier;
        let challenge = b"test_challenge";
        let rp_id = "localhost";
        let origin = "http://localhost:3000";

        let proof = build_proof(&signing_key, challenge, rp_id, origin, 0x05, true);

        // Create a different public key for verification
        let seed2 = [99u8; 32];
        let secret2 = p256::SecretKey::from_slice(&seed2).expect("valid seed");
        let signing_key2 = p256::ecdsa::SigningKey::from(secret2);
        let verifying_key2 = p256::ecdsa::VerifyingKey::from(&signing_key2);
        let public_key2 = verifying_key2.to_sec1_point(false).as_bytes().to_vec();

        let result = verifier.verify(&proof, challenge, rp_id, origin, &public_key2);
        assert!(
            result.is_err(),
            "verification should fail when public key doesn't match the signature"
        );
    }
}

/// Credential keys for a deployment that registers no passkeys: no
/// credential resolves, so the passkey step-up path refuses every assertion.
/// The server approves high-risk requests by a fresh sign-in instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoCredentialKeys;

impl CredentialKeys for NoCredentialKeys {
    fn public_key(&self, _person_id: &str, _credential_id: &str) -> Option<Vec<u8>> {
        None
    }
}
