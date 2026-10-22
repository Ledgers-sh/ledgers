//! Short-lived signed download links. The link carries the company id, the
//! document id, the expiry (Unix seconds) and an HMAC-SHA256 over all of them
//! and the content hash, so a link cannot be moved to another document, another
//! company or extended.

use sha2::{Digest, Sha256};

/// How long a download link stays valid: five minutes.
pub const LINK_TTL_SECONDS: i64 = 300;

/// Why a link was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The link's expiry has passed.
    Expired,
    /// The signature does not match.
    BadSignature,
}

/// HMAC-SHA256 (RFC 2104) over `message` with `key`.
#[must_use]
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block_key = [0u8; BLOCK];
    if key.len() > BLOCK {
        block_key[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    let ipad: Vec<u8> = block_key.iter().map(|b| b ^ 0x36).collect();
    inner.update(&ipad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    let opad: Vec<u8> = block_key.iter().map(|b| b ^ 0x5c).collect();
    outer.update(&opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn payload(company_id: &str, document_id: &str, sha256: &str, expires_at: i64) -> String {
    format!("document\n{company_id}\n{document_id}\n{sha256}\n{expires_at}")
}

/// The hex signature for a document download of `company_id` valid until
/// `expires_at`. The company is signed too: a link made for one company never
/// verifies as a link to another.
#[must_use]
pub fn sign(
    key: &[u8],
    company_id: &str,
    document_id: &str,
    sha256: &str,
    expires_at: i64,
) -> String {
    hex::encode(hmac_sha256(
        key,
        payload(company_id, document_id, sha256, expires_at).as_bytes(),
    ))
}

/// Checks a link: the signature first (constant time), then the expiry.
///
/// # Errors
/// [`LinkError::BadSignature`] for a forged or altered link,
/// [`LinkError::Expired`] once `now` is past `expires_at`.
pub fn verify(
    key: &[u8],
    company_id: &str,
    document_id: &str,
    sha256: &str,
    expires_at: i64,
    signature: &str,
    now: i64,
) -> Result<(), LinkError> {
    let expected = sign(key, company_id, document_id, sha256, expires_at);
    if !constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
        return Err(LinkError::BadSignature);
    }
    if now > expires_at {
        return Err(LinkError::Expired);
    }
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"test-signing-key";

    #[test]
    fn hmac_matches_the_rfc_4231_vector() {
        // RFC 4231 test case 2.
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn a_fresh_link_verifies_until_it_expires() {
        let sig = sign(KEY, "co1", "doc1", "abc", 1_000);
        assert_eq!(
            verify(KEY, "co1", "doc1", "abc", 1_000, &sig, 1_000),
            Ok(())
        );
        assert_eq!(
            verify(KEY, "co1", "doc1", "abc", 1_000, &sig, 1_001),
            Err(LinkError::Expired)
        );
    }

    #[test]
    fn an_extended_or_moved_link_is_refused() {
        let sig = sign(KEY, "co1", "doc1", "abc", 1_000);
        // Extending the expiry changes the signed payload.
        assert_eq!(
            verify(KEY, "co1", "doc1", "abc", 9_000, &sig, 2_000),
            Err(LinkError::BadSignature)
        );
        // The same signature names another document.
        assert_eq!(
            verify(KEY, "co1", "doc2", "abc", 1_000, &sig, 500),
            Err(LinkError::BadSignature)
        );
        // Another key cannot mint the link.
        assert_eq!(
            verify(b"other", "co1", "doc1", "abc", 1_000, &sig, 500),
            Err(LinkError::BadSignature)
        );
    }

    #[test]
    fn a_link_is_bound_to_its_company() {
        let sig = sign(KEY, "co1", "doc1", "abc", 1_000);
        // The same document id and hash under another company: refused.
        assert_eq!(
            verify(KEY, "co2", "doc1", "abc", 1_000, &sig, 500),
            Err(LinkError::BadSignature)
        );
        // A link made for company 2 never verifies for company 1 either.
        let other = sign(KEY, "co2", "doc1", "abc", 1_000);
        assert_eq!(
            verify(KEY, "co1", "doc1", "abc", 1_000, &other, 500),
            Err(LinkError::BadSignature)
        );
    }
}
