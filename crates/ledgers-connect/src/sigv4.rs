//! AWS Signature Version 4, written by hand so the crate stays wasm-compatible
//! (no `aws-sdk`). Only what Cost Explorer needs: a `POST` with a JSON body,
//! signed over `host`, `x-amz-date`, and the headers the caller passes.
//! Reference: <https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_aws-signing.html>.

use sha2::{Digest, Sha256};

/// An IAM access key pair. Debug is not derived so the secret cannot be
/// printed by accident.
#[derive(Clone)]
pub struct AwsKeys {
    /// The access key id (`AKIA...`).
    pub access_key_id: String,
    /// The secret access key.
    pub secret_access_key: String,
}

impl std::fmt::Debug for AwsKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsKeys").finish_non_exhaustive()
    }
}

/// Where a request goes: the method, the host, and the region and service
/// the credential scope names.
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    /// The HTTP method (`POST`).
    pub method: &'a str,
    /// The region the scope names (`us-east-1`).
    pub region: &'a str,
    /// The service the scope names (`ce`).
    pub service: &'a str,
    /// The host header (`ce.us-east-1.amazonaws.com`).
    pub host: &'a str,
}

/// The headers a signed request must carry, besides the ones it was signed with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// The `Authorization` header value.
    pub authorization: String,
    /// The `X-Amz-Date` header value (`20261024T100000Z`).
    pub amz_date: String,
}

/// Converts an RFC 3339 UTC instant (`2026-10-24T10:00:00Z`) to the `SigV4`
/// timestamp (`20261024T100000Z`). `None` for anything else.
#[must_use]
pub fn amz_date(now: &str) -> Option<String> {
    let bytes = now.as_bytes();
    let well_formed = bytes.len() == 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'Z';
    if !well_formed {
        return None;
    }
    let digits: String = now.chars().filter(char::is_ascii_digit).collect();
    (digits.len() == 14).then(|| format!("{}T{}Z", &digits[..8], &digits[8..]))
}

/// SHA-256 of `data` as lower-case hex.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// HMAC-SHA256 (RFC 2104), written out over SHA-256 so there is no keyed-hash
/// dependency and no panic path.
fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 64;
    let mut block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        block[..digest.len()].copy_from_slice(&digest);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner: Vec<u8> = block
        .iter()
        .map(|b| b ^ 0x36)
        .chain(message.iter().copied())
        .collect();
    let inner_digest = Sha256::digest(&inner);
    let outer: Vec<u8> = block
        .iter()
        .map(|b| b ^ 0x5c)
        .chain(inner_digest.iter().copied())
        .collect();
    Sha256::digest(&outer).to_vec()
}

/// Signs one request.
///
/// `headers` are the request's headers other than `host` and `x-amz-date`,
/// names in any case; they are signed as given. `body` is the exact bytes sent.
#[must_use]
pub fn sign(
    keys: &AwsKeys,
    target: &Target<'_>,
    amz_date: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Signature {
    let Target {
        method,
        region,
        service,
        host,
    } = *target;
    let date = &amz_date[..8];
    let mut canonical_headers: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), trim_value(value)))
        .collect();
    canonical_headers.push(("host".to_owned(), host.to_owned()));
    canonical_headers.push(("x-amz-date".to_owned(), amz_date.to_owned()));
    canonical_headers.sort_by(|a, b| a.0.cmp(&b.0));
    let signed: Vec<&str> = canonical_headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let signed_headers = signed.join(";");
    let mut header_block = String::new();
    for (name, value) in &canonical_headers {
        header_block.push_str(name);
        header_block.push(':');
        header_block.push_str(value);
        header_block.push('\n');
    }
    let canonical_request = format!(
        "{method}\n/\n\n{header_block}\n{signed_headers}\n{}",
        sha256_hex(body)
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let secret = format!("AWS4{}", keys.secret_access_key);
    let k_date = hmac(secret.as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));
    Signature {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            keys.access_key_id
        ),
        amz_date: amz_date.to_owned(),
    }
}

/// Header values are trimmed and inner runs of spaces collapsed, as `SigV4` requires.
fn trim_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The AWS documentation's example key pair (not a real credential).
    fn example_keys() -> AwsKeys {
        AwsKeys {
            access_key_id: "AKIDEXAMPLE".to_owned(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_owned(),
        }
    }

    #[test]
    fn converts_rfc3339_to_amz_date() {
        assert_eq!(
            amz_date("2026-10-24T10:00:00Z").as_deref(),
            Some("20261024T100000Z")
        );
        assert_eq!(amz_date("2026-10-24 10:00:00"), None);
        assert_eq!(amz_date("nope"), None);
    }

    #[test]
    fn matches_the_aws_get_vanilla_test_vector() {
        // The AWS signature test suite's "get-vanilla" case (GET /, host and
        // x-amz-date signed, empty body).
        let keys = example_keys();
        let signature = sign(
            &keys,
            &Target {
                method: "GET",
                region: "us-east-1",
                service: "service",
                host: "example.amazonaws.com",
            },
            "20150830T123600Z",
            &[],
            b"",
        );
        assert_eq!(
            signature.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    fn post() -> Target<'static> {
        Target {
            method: "POST",
            region: "us-east-1",
            service: "ce",
            host: "ce.us-east-1.amazonaws.com",
        }
    }

    #[test]
    fn signature_changes_with_the_body_and_the_headers() {
        let keys = example_keys();
        let base = sign(
            &keys,
            &post(),
            "20261024T100000Z",
            &[("content-type", "application/x-amz-json-1.1")],
            b"{}",
        );
        let other_body = sign(
            &keys,
            &post(),
            "20261024T100000Z",
            &[("content-type", "application/x-amz-json-1.1")],
            b"{\"a\":1}",
        );
        let other_target = sign(
            &keys,
            &post(),
            "20261024T100000Z",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", "AWSInsightsIndexService.GetCostAndUsage"),
            ],
            b"{}",
        );
        assert_ne!(base.authorization, other_body.authorization);
        assert_ne!(base.authorization, other_target.authorization);
        assert!(
            base.authorization
                .contains("SignedHeaders=content-type;host;x-amz-date")
        );
    }

    #[test]
    fn the_secret_is_not_in_debug_output() {
        let text = format!("{:?}", example_keys());
        assert!(!text.contains("wJalrXUtnFEMI"));
    }
}
