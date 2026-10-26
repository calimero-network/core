//! Identity for the Calimero cloud: people sign in with WebAuthn passkeys,
//! machines with scoped API keys, and both become a short session verified by
//! any node of the cluster.
//!
//! This crate is pure logic. Accounts, credentials and revocations are stored
//! by the caller (the cloud node keeps them in the system group).

use ring::digest::{digest, SHA256};
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

pub mod webauthn;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use webauthn::{
    verify_assertion, verify_registration, Assertion, Credential, PublicKey, Registration,
    RelyingParty,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("malformed {0}")]
    Malformed(&'static str),
    #[error("unsupported {0}")]
    Unsupported(&'static str),
    #[error("{0} does not match")]
    Mismatch(&'static str),
    #[error("signature does not verify")]
    BadSignature,
    #[error("token expired")]
    Expired,
}

/// `n` random bytes.
#[must_use]
pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0; N];
    SystemRandom::new()
        .fill(&mut out)
        .expect("system randomness is available");
    out
}

/// base64url without padding.
#[must_use]
pub fn b64url(bytes: &[u8]) -> String {
    webauthn::enc(bytes)
}

/// Decode base64url (padding tolerated).
pub fn from_b64url(s: &str) -> Result<Vec<u8>, Error> {
    webauthn::b64(s)
}

/// SHA-256.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(digest(&SHA256, data).as_ref());
    out
}

/// What a session token says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionClaims {
    /// The account, base64url.
    pub account: String,
    /// The session, for revocation and refresh.
    pub session: String,
    /// Unix seconds.
    pub expires_at: u64,
    /// Which K_s version signed it.
    pub key_version: u32,
}

/// Mint a session token: `v1.<claims>.<mac>`, MAC = HMAC-SHA256 under `key`.
#[must_use]
pub fn mint_session(key: &[u8; 32], claims: &SessionClaims) -> String {
    let body = b64url(&serde_json::to_vec(claims).expect("claims serialize"));
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), body.as_bytes());
    format!("v1.{body}.{}", b64url(tag.as_ref()))
}

/// Read a token's claims without checking it, to find which key to check with.
pub fn peek_session(token: &str) -> Result<SessionClaims, Error> {
    let mut parts = token.split('.');
    let (Some("v1"), Some(body), Some(_), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(Error::Malformed("session token"));
    };
    serde_json::from_slice(&from_b64url(body)?).map_err(|_| Error::Malformed("session claims"))
}

/// Check a session token's MAC under `key` and its expiry against `now`.
pub fn verify_session(key: &[u8; 32], token: &str, now: u64) -> Result<SessionClaims, Error> {
    let mut parts = token.split('.');
    let (Some("v1"), Some(body), Some(tag), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(Error::Malformed("session token"));
    };
    hmac::verify(
        &hmac::Key::new(hmac::HMAC_SHA256, key),
        body.as_bytes(),
        &from_b64url(tag)?,
    )
    .map_err(|_| Error::BadSignature)?;
    let claims: SessionClaims =
        serde_json::from_slice(&from_b64url(body)?).map_err(|_| Error::Malformed("session claims"))?;
    if claims.expires_at <= now {
        return Err(Error::Expired);
    }
    Ok(claims)
}

/// A new API key and the hash to store for it. The key is shown once.
#[must_use]
pub fn new_api_key() -> (String, String) {
    let key = format!("cal_{}", b64url(&random::<32>()));
    let hash = api_key_hash(&key);
    (key, hash)
}

/// The stored form of an API key.
#[must_use]
pub fn api_key_hash(key: &str) -> String {
    b64url(&sha256(key.as_bytes()))
}

/// The challenge a passkey signs to consent to one call of a confirm method:
/// `SHA-256("calimero.confirm.v1" | app | 0 | method | 0 | SHA-256(args) | nonce)`.
#[must_use]
pub fn confirm_challenge(app: &str, method: &str, args: &[u8], nonce: &[u8]) -> [u8; 32] {
    let mut buf = b"calimero.confirm.v1".to_vec();
    buf.extend_from_slice(app.as_bytes());
    buf.push(0);
    buf.extend_from_slice(method.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&sha256(args));
    buf.extend_from_slice(nonce);
    sha256(&buf)
}

#[cfg(test)]
mod tests {
    use super::testing::SoftAuthenticator;
    use super::*;

    fn rp() -> RelyingParty {
        RelyingParty {
            id: "localhost".into(),
            origins: vec!["http://localhost:3000".into()],
            require_uv: false,
        }
    }

    #[test]
    fn registers_and_asserts_es256() {
        let mut auth = SoftAuthenticator::new("localhost", "http://localhost:3000");
        let challenge = random::<32>();
        let reg = auth.register(&challenge);
        let cred = verify_registration(&rp(), &challenge, &reg).unwrap();
        assert_eq!(cred.id, auth.credential_id());

        let challenge = random::<32>();
        let a = auth.assert(&challenge);
        let count = verify_assertion(&rp(), &challenge, &cred, &a).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn refuses_wrong_challenge_origin_and_tampering() {
        let mut auth = SoftAuthenticator::new("localhost", "http://localhost:3000");
        let challenge = random::<32>();
        let cred = verify_registration(&rp(), &challenge, &auth.register(&challenge)).unwrap();

        let a = auth.assert(&[1; 32]);
        assert_eq!(
            verify_assertion(&rp(), &[2; 32], &cred, &a),
            Err(Error::Mismatch("challenge"))
        );

        let mut evil = SoftAuthenticator::with_key(&auth, "localhost", "https://evil.example");
        let a = evil.assert(&[3; 32]);
        assert_eq!(
            verify_assertion(&rp(), &[3; 32], &cred, &a),
            Err(Error::Mismatch("origin"))
        );

        let mut a = auth.assert(&[4; 32]);
        let mut sig = from_b64url(&a.signature).unwrap();
        let last = sig.len() - 1;
        sig[last] ^= 1;
        a.signature = b64url(&sig);
        assert!(verify_assertion(&rp(), &[4; 32], &cred, &a).is_err());
    }

    #[test]
    fn refuses_a_replayed_counter() {
        let mut auth = SoftAuthenticator::new("localhost", "http://localhost:3000");
        let challenge = random::<32>();
        let mut cred = verify_registration(&rp(), &challenge, &auth.register(&challenge)).unwrap();
        let a = auth.assert(&[5; 32]);
        cred.sign_count = verify_assertion(&rp(), &[5; 32], &cred, &a).unwrap();
        assert_eq!(
            verify_assertion(&rp(), &[5; 32], &cred, &a),
            Err(Error::Mismatch("signature counter"))
        );
    }

    #[test]
    fn session_tokens() {
        let key = random::<32>();
        let claims = SessionClaims {
            account: "acc".into(),
            session: "s1".into(),
            expires_at: 1000,
            key_version: 1,
        };
        let token = mint_session(&key, &claims);
        assert_eq!(verify_session(&key, &token, 999).unwrap(), claims);
        assert_eq!(peek_session(&token).unwrap(), claims);
        assert_eq!(verify_session(&key, &token, 1000), Err(Error::Expired));
        assert_eq!(
            verify_session(&random::<32>(), &token, 1),
            Err(Error::BadSignature)
        );
    }

    #[test]
    fn api_keys_and_confirm_challenges() {
        let (key, hash) = new_api_key();
        assert!(key.starts_with("cal_"));
        assert_eq!(api_key_hash(&key), hash);
        let a = confirm_challenge("app", "transfer", b"{\"to\":1}", b"n");
        let b = confirm_challenge("app", "transfer", b"{\"to\":2}", b"n");
        assert_ne!(a, b);
    }
}
