//! `OpenID` Connect pieces of the people sign-in (issue #12).
//!
//! The Ledgers API is a confidential-free (public, PKCE) OIDC client of a
//! per-app Cratefield auth instance. This module holds everything that is
//! pure: discovery parsing, the authorization URL, PKCE, the signed
//! login-state cookie, JWKS parsing and ID-token verification. Time,
//! randomness and HTTP are supplied by the caller, so the crate stays
//! wasm-clean and every rule here is unit-testable with a generated key.
//!
//! The access-token verifier in `cratefield-auth-client` is deliberately not
//! used for the callback: it accepts only `typ: at+jwt` tokens and has no
//! nonce check, and the callback must verify an ID token's nonce.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use p256::EncodedPoint;
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
use serde_json::{Value, json};

/// The only signature algorithm the sign-in accepts.
pub const ID_TOKEN_ALG: &str = "ES256";

/// Clock skew tolerated on `exp` and `iat`, in seconds.
pub const CLOCK_SKEW_SECS: i64 = 60;

/// Why an OIDC step failed. Logged, never shown to the browser in detail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OidcError {
    #[error("token is not three base64url segments")]
    Malformed,
    #[error("token header is not JSON, or omits alg/kid")]
    Header,
    #[error("algorithm {0} is not accepted; only ES256 is")]
    Algorithm(String),
    #[error("token type {0:?} is not an ID token")]
    Type(String),
    #[error("no published key matches the token's kid")]
    UnknownKey,
    #[error("signature does not verify")]
    Signature,
    #[error("payload is not the expected JSON")]
    Payload,
    #[error("issuer is {got:?}, expected {want:?}")]
    Issuer { got: String, want: String },
    #[error("audience does not name this client")]
    Audience,
    #[error("authorized party (azp) is not this client")]
    Azp,
    #[error("token expired")]
    Expired,
    #[error("token is not valid yet")]
    NotYetValid,
    #[error("nonce does not match the one this sign-in started with")]
    Nonce,
    #[error("token has no subject")]
    MissingSubject,
    #[error("discovery document: {0}")]
    Discovery(String),
    #[error("key set: {0}")]
    Jwks(String),
}

/// The endpoints the issuer's discovery document publishes, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

#[derive(Deserialize)]
struct RawDiscovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

/// The `scheme://host[:port]` part of an absolute URL, lowercased, or `None`.
#[must_use]
pub fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(match parsed.port() {
        Some(port) => format!("{}://{host}:{port}", parsed.scheme()),
        None => format!("{}://{host}", parsed.scheme()),
    })
}

/// Parses the discovery document and checks it: the issuer must be the one
/// configured, and every endpoint must live on the issuer's origin. A
/// document that points the token exchange or the key set elsewhere is
/// refused rather than followed.
///
/// # Errors
/// [`OidcError::Discovery`] naming the failed check.
pub fn parse_discovery(body: &[u8], configured_issuer: &str) -> Result<Discovery, OidcError> {
    let raw: RawDiscovery =
        serde_json::from_slice(body).map_err(|err| OidcError::Discovery(err.to_string()))?;
    if raw.issuer != configured_issuer {
        return Err(OidcError::Discovery(format!(
            "issuer is {:?}, expected {configured_issuer:?}",
            raw.issuer
        )));
    }
    let want = origin_of(configured_issuer)
        .ok_or_else(|| OidcError::Discovery("configured issuer is not an http(s) URL".into()))?;
    for (name, endpoint) in [
        ("authorization_endpoint", &raw.authorization_endpoint),
        ("token_endpoint", &raw.token_endpoint),
        ("jwks_uri", &raw.jwks_uri),
    ] {
        if origin_of(endpoint).as_deref() != Some(want.as_str()) {
            return Err(OidcError::Discovery(format!(
                "{name} is not on the issuer's origin"
            )));
        }
    }
    Ok(Discovery {
        issuer: raw.issuer,
        authorization_endpoint: raw.authorization_endpoint,
        token_endpoint: raw.token_endpoint,
        jwks_uri: raw.jwks_uri,
    })
}

/// The base64url (no padding) encoding of `bytes`.
#[must_use]
pub fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The PKCE S256 challenge for a code verifier (RFC 7636).
#[must_use]
pub fn pkce_challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

/// The authorization request URL the browser is redirected to.
///
/// # Panics
/// Never for a discovery document that passed [`parse_discovery`]: the
/// authorization endpoint is an absolute URL.
#[must_use]
pub fn authorize_url(
    discovery: &Discovery,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    challenge: &str,
) -> String {
    authorize_url_with(
        discovery,
        client_id,
        redirect_uri,
        state,
        nonce,
        challenge,
        false,
    )
}

/// The authorization URL. With `reauthenticate`, the issuer must sign the
/// person in again (`prompt=login`) and may not reuse an earlier login
/// (`max_age=0`), so the ID token carries a fresh `auth_time`.
///
/// # Panics
/// If the discovery's authorization endpoint is not an absolute URL; discovery
/// documents are checked before they reach this function.
#[must_use]
pub fn authorize_url_with(
    discovery: &Discovery,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    challenge: &str,
    reauthenticate: bool,
) -> String {
    let mut url = url::Url::parse(&discovery.authorization_endpoint)
        .expect("discovery endpoints are absolute URLs");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", "openid email")
        .append_pair("state", state)
        .append_pair("nonce", nonce)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    if reauthenticate {
        url.query_pairs_mut()
            .append_pair("prompt", "login")
            .append_pair("max_age", "0");
    }
    url.to_string()
}

/// One published signing key, by `kid`.
pub type PublishedKey = (String, VerifyingKey);

#[derive(Deserialize)]
struct RawJwks {
    keys: Vec<RawJwk>,
}

#[derive(Deserialize)]
struct RawJwk {
    kty: String,
    #[serde(default)]
    crv: Option<String>,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default)]
    r#use: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
}

/// Parses a JWKS into the P-256 signing keys it publishes. Keys of another
/// type, curve, use or algorithm are skipped; a key that claims to be P-256
/// but does not decode is an error, because it is a broken publication.
///
/// # Errors
/// [`OidcError::Jwks`] when the document is not a JWKS or holds no usable key.
pub fn parse_jwks(body: &[u8]) -> Result<Vec<PublishedKey>, OidcError> {
    let raw: RawJwks =
        serde_json::from_slice(body).map_err(|err| OidcError::Jwks(err.to_string()))?;
    let mut keys = Vec::new();
    for jwk in raw.keys {
        if jwk.kty != "EC"
            || jwk.crv.as_deref() != Some("P-256")
            || jwk.r#use.as_deref().is_some_and(|u| u != "sig")
            || jwk.alg.as_deref().is_some_and(|a| a != ID_TOKEN_ALG)
        {
            continue;
        }
        let Some(kid) = jwk.kid.filter(|k| !k.is_empty()) else {
            continue;
        };
        let x = jwk
            .x
            .and_then(|v| decode_coordinate(&v))
            .ok_or_else(|| OidcError::Jwks(format!("key {kid} has a bad x coordinate")))?;
        let y = jwk
            .y
            .and_then(|v| decode_coordinate(&v))
            .ok_or_else(|| OidcError::Jwks(format!("key {kid} has a bad y coordinate")))?;
        let point = EncodedPoint::from_affine_coordinates(&x.into(), &y.into(), false);
        let key = VerifyingKey::from_encoded_point(&point)
            .map_err(|_| OidcError::Jwks(format!("key {kid} is not a point on P-256")))?;
        keys.push((kid, key));
    }
    if keys.is_empty() {
        return Err(OidcError::Jwks("no usable ES256 signing key".into()));
    }
    Ok(keys)
}

fn decode_coordinate(text: &str) -> Option<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

/// The `aud` claim: one client id, or several.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Audience {
    One(String),
    Many(Vec<String>),
    #[default]
    None,
}

impl Audience {
    fn names(&self, client_id: &str) -> bool {
        match self {
            Audience::One(one) => one == client_id,
            Audience::Many(many) => many.iter().any(|a| a == client_id),
            Audience::None => false,
        }
    }

    fn len(&self) -> usize {
        match self {
            Audience::One(_) => 1,
            Audience::Many(many) => many.len(),
            Audience::None => 0,
        }
    }
}

/// The claims of a verified ID token that the sign-in uses.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct IdClaims {
    pub iss: String,
    pub sub: String,
    #[serde(default)]
    pub aud: Audience,
    #[serde(default)]
    pub azp: Option<String>,
    pub exp: i64,
    #[serde(default)]
    pub iat: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    /// When the person authenticated at the issuer, epoch seconds.
    #[serde(default)]
    pub auth_time: Option<i64>,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    typ: Option<String>,
}

/// Verifies an ID token against the published keys and the sign-in's own
/// expectations. Order: shape, algorithm (before any key is looked up),
/// token type, key by `kid`, signature, then the claims. Nothing in the
/// payload is trusted until the signature verifies.
///
/// # Errors
/// [`OidcError`] naming the first failed check.
pub fn verify_id_token(
    token: &str,
    keys: &[PublishedKey],
    issuer: &str,
    client_id: &str,
    expected_nonce: &str,
    now: i64,
) -> Result<IdClaims, OidcError> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(OidcError::Malformed);
    };
    let header_bytes = URL_SAFE_NO_PAD
        .decode(header_b64)
        .map_err(|_| OidcError::Malformed)?;
    let header: Header = serde_json::from_slice(&header_bytes).map_err(|_| OidcError::Header)?;
    if header.alg != ID_TOKEN_ALG {
        return Err(OidcError::Algorithm(header.alg));
    }
    // An access token has the same signature and often the same audience. The
    // callback verifies an ID token, so an access token is refused by type.
    if let Some(typ) = header.typ.as_deref()
        && typ.eq_ignore_ascii_case("at+jwt")
    {
        return Err(OidcError::Type(typ.to_owned()));
    }
    let kid = header.kid.ok_or(OidcError::Header)?;
    let key = keys
        .iter()
        .find(|(k, _)| *k == kid)
        .map(|(_, key)| key)
        .ok_or(OidcError::UnknownKey)?;

    let signature_bytes = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|_| OidcError::Malformed)?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|_| OidcError::Signature)?;
    let signing_input = format!("{header_b64}.{payload_b64}");
    key.verify(signing_input.as_bytes(), &signature)
        .map_err(|_| OidcError::Signature)?;

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| OidcError::Malformed)?;
    let claims: IdClaims =
        serde_json::from_slice(&payload_bytes).map_err(|_| OidcError::Payload)?;

    if claims.iss != issuer {
        return Err(OidcError::Issuer {
            got: claims.iss,
            want: issuer.to_owned(),
        });
    }
    if !claims.aud.names(client_id) {
        return Err(OidcError::Audience);
    }
    // With several audiences the OIDC rule applies: azp names this client.
    if claims.aud.len() > 1 && claims.azp.as_deref() != Some(client_id) {
        return Err(OidcError::Azp);
    }
    if claims.azp.as_deref().is_some_and(|azp| azp != client_id) {
        return Err(OidcError::Azp);
    }
    if now - CLOCK_SKEW_SECS >= claims.exp {
        return Err(OidcError::Expired);
    }
    if claims.iat.is_some_and(|iat| iat - CLOCK_SKEW_SECS > now) {
        return Err(OidcError::NotYetValid);
    }
    let nonce = claims.nonce.as_deref().unwrap_or("");
    if expected_nonce.is_empty() || !ct_eq(nonce.as_bytes(), expected_nonce.as_bytes()) {
        return Err(OidcError::Nonce);
    }
    if claims.sub.is_empty() {
        return Err(OidcError::MissingSubject);
    }
    Ok(claims)
}

/// The short-lived sign-in transaction the login cookie carries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoginState {
    pub state: String,
    pub nonce: String,
    pub verifier: String,
    /// Expiry, epoch seconds.
    pub exp: i64,
    /// Where the app wants the person sent after sign-in (an app path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_to: Option<String>,
}

type HmacSha256 = Hmac<Sha256>;

/// Seals `payload` with HMAC-SHA256 under `key`: `base64url(payload).base64url(mac)`.
///
/// # Panics
/// Never: HMAC accepts a key of any length.
#[must_use]
pub fn seal(key: &[u8], payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

/// Opens a value made by [`seal`]. `None` for a malformed value or a MAC that
/// does not verify (checked in constant time).
#[must_use]
pub fn open(key: &[u8], value: &str) -> Option<Vec<u8>> {
    let (payload_b64, mac_b64) = value.split_once('.')?;
    if mac_b64.contains('.') {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let mac_bytes = URL_SAFE_NO_PAD.decode(mac_b64).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(&payload);
    mac.verify_slice(&mac_bytes).ok()?;
    Some(payload)
}

/// A purpose-bound key derived from a server secret: HMAC-SHA256 keyed by
/// `secret` over `label`.
///
/// # Panics
/// Never: HMAC accepts a key of any length.
#[must_use]
pub fn derive_key(secret: &[u8], label: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(label.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Constant-time equality for secrets of any length (length is not secret).
#[must_use]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    cratefield_core::constant_time_eq(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::{SigningKey, signature::Signer};

    const ISSUER: &str = "https://auth-staging.ledgers.sh";
    const CLIENT: &str = "ledgers-api-staging";
    const NOW: i64 = 1_800_000_000;

    fn key() -> SigningKey {
        SigningKey::from_slice(&[3u8; 32]).expect("valid scalar")
    }

    fn keys_for(signing: &SigningKey, kid: &str) -> Vec<PublishedKey> {
        let point = VerifyingKey::from(signing).to_encoded_point(false);
        let jwks = json!({"keys": [{
            "kty": "EC", "crv": "P-256", "kid": kid, "alg": "ES256", "use": "sig",
            "x": b64url(point.x().unwrap()), "y": b64url(point.y().unwrap()),
        }]});
        parse_jwks(jwks.to_string().as_bytes()).expect("a usable key")
    }

    fn jwt(signing: &SigningKey, kid: &str, typ: Option<&str>, claims: &Value) -> String {
        let mut header = json!({"alg": "ES256", "kid": kid});
        if let Some(typ) = typ {
            header["typ"] = json!(typ);
        }
        let input = format!(
            "{}.{}",
            b64url(header.to_string().as_bytes()),
            b64url(claims.to_string().as_bytes())
        );
        let sig: p256::ecdsa::Signature = signing.sign(input.as_bytes());
        format!("{input}.{}", b64url(&sig.to_bytes()))
    }

    fn claims() -> Value {
        json!({
            "iss": ISSUER, "sub": "person-1", "aud": CLIENT, "azp": CLIENT,
            "exp": NOW + 300, "iat": NOW, "nonce": "n-1", "email": "a@b.test",
            "email_verified": true, "roles": ["owner"],
        })
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636_appendix_b() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn discovery_must_name_the_configured_issuer_on_its_own_origin() {
        let good = json!({
            "issuer": ISSUER,
            "authorization_endpoint": format!("{ISSUER}/authorize"),
            "token_endpoint": format!("{ISSUER}/token"),
            "jwks_uri": format!("{ISSUER}/jwks.json"),
        });
        assert!(parse_discovery(good.to_string().as_bytes(), ISSUER).is_ok());

        let other_issuer = good.clone();
        assert!(
            parse_discovery(other_issuer.to_string().as_bytes(), "https://other.test").is_err()
        );

        let mut elsewhere = good.clone();
        elsewhere["token_endpoint"] = json!("https://attacker.test/token");
        assert!(parse_discovery(elsewhere.to_string().as_bytes(), ISSUER).is_err());
    }

    #[test]
    fn authorization_url_carries_pkce_state_and_nonce() {
        let discovery = Discovery {
            issuer: ISSUER.into(),
            authorization_endpoint: format!("{ISSUER}/authorize"),
            token_endpoint: format!("{ISSUER}/token"),
            jwks_uri: format!("{ISSUER}/jwks.json"),
        };
        let url = authorize_url(
            &discovery,
            CLIENT,
            "https://api.test/v1/auth/callback",
            "st",
            "no",
            "ch",
        );
        let parsed = url::Url::parse(&url).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge"], "ch");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], "st");
        assert_eq!(q["nonce"], "no");
        assert_eq!(q["redirect_uri"], "https://api.test/v1/auth/callback");
    }

    #[test]
    fn a_valid_id_token_verifies_and_yields_its_claims() {
        let signing = key();
        let keys = keys_for(&signing, "k1");
        let token = jwt(&signing, "k1", Some("JWT"), &claims());
        let verified = verify_id_token(&token, &keys, ISSUER, CLIENT, "n-1", NOW).unwrap();
        assert_eq!(verified.sub, "person-1");
        assert_eq!(verified.roles, vec!["owner".to_owned()]);
    }

    #[test]
    fn each_claim_check_refuses_on_its_own() {
        let signing = key();
        let keys = keys_for(&signing, "k1");
        let check = |claims: Value, nonce: &str| {
            let token = jwt(&signing, "k1", Some("JWT"), &claims);
            verify_id_token(&token, &keys, ISSUER, CLIENT, nonce, NOW)
        };
        let mut c = claims();
        c["iss"] = json!("https://evil.test");
        assert!(matches!(check(c, "n-1"), Err(OidcError::Issuer { .. })));

        let mut c = claims();
        c["aud"] = json!("another-client");
        assert_eq!(check(c, "n-1"), Err(OidcError::Audience));

        let mut c = claims();
        c["aud"] = json!([CLIENT, "another-client"]);
        c["azp"] = json!("another-client");
        assert_eq!(check(c, "n-1"), Err(OidcError::Azp));

        let mut c = claims();
        c["exp"] = json!(NOW - 61);
        assert_eq!(check(c, "n-1"), Err(OidcError::Expired));

        assert_eq!(check(claims(), "other"), Err(OidcError::Nonce));
        assert_eq!(check(claims(), ""), Err(OidcError::Nonce));

        let mut c = claims();
        c["sub"] = json!("");
        assert_eq!(check(c, "n-1"), Err(OidcError::MissingSubject));
    }

    #[test]
    fn signature_algorithm_and_key_checks_come_before_the_claims() {
        let signing = key();
        let keys = keys_for(&signing, "k1");
        let rogue = SigningKey::from_slice(&[4u8; 32]).unwrap();
        let rogue_token = jwt(&rogue, "k1", Some("JWT"), &claims());
        assert_eq!(
            verify_id_token(&rogue_token, &keys, ISSUER, CLIENT, "n-1", NOW),
            Err(OidcError::Signature)
        );
        let unknown = jwt(&signing, "k9", Some("JWT"), &claims());
        assert_eq!(
            verify_id_token(&unknown, &keys, ISSUER, CLIENT, "n-1", NOW),
            Err(OidcError::UnknownKey)
        );
        let hs = format!(
            "{}.{}.x",
            b64url(br#"{"alg":"HS256","kid":"k1"}"#),
            b64url(claims().to_string().as_bytes())
        );
        assert_eq!(
            verify_id_token(&hs, &keys, ISSUER, CLIENT, "n-1", NOW),
            Err(OidcError::Algorithm("HS256".into()))
        );
        let at = jwt(&signing, "k1", Some("at+jwt"), &claims());
        assert_eq!(
            verify_id_token(&at, &keys, ISSUER, CLIENT, "n-1", NOW),
            Err(OidcError::Type("at+jwt".into()))
        );
    }

    #[test]
    fn jwks_skips_other_key_types_and_refuses_a_key_with_no_usable_point() {
        assert!(parse_jwks(br#"{"keys":[{"kty":"RSA","kid":"r"}]}"#).is_err());
        let bad_point = json!({"keys": [{"kty": "EC", "crv": "P-256", "kid": "x",
            "x": b64url(&[1u8; 32]), "y": b64url(&[2u8; 32])}]});
        assert!(parse_jwks(bad_point.to_string().as_bytes()).is_err());
    }

    #[test]
    fn sealed_values_open_only_when_untampered() {
        let key = derive_key(b"secret", "label");
        let sealed = seal(&key, b"payload");
        assert_eq!(open(&key, &sealed).as_deref(), Some(&b"payload"[..]));
        assert_eq!(open(&derive_key(b"other", "label"), &sealed), None);
        let forged = sealed.replacen("cGF5", "ZmFr", 1);
        assert_eq!(open(&key, &forged), None);
        assert_eq!(open(&key, "no-dot-here"), None);
    }
}

#[cfg(test)]
mod reauth_url_tests {
    use super::{Discovery, authorize_url_with};

    #[test]
    fn reauthentication_asks_the_issuer_for_a_fresh_login() {
        let discovery = Discovery {
            issuer: "https://issuer.example".to_owned(),
            authorization_endpoint: "https://issuer.example/authorize".to_owned(),
            token_endpoint: "https://issuer.example/token".to_owned(),
            jwks_uri: "https://issuer.example/keys".to_owned(),
        };
        let fresh =
            authorize_url_with(&discovery, "c", "https://r.example/cb", "s", "n", "x", true);
        assert!(fresh.contains("prompt=login"));
        assert!(fresh.contains("max_age=0"));
        let normal = authorize_url_with(
            &discovery,
            "c",
            "https://r.example/cb",
            "s",
            "n",
            "x",
            false,
        );
        assert!(!normal.contains("prompt="));
    }
}
