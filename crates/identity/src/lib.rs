//! Identity for Calimero: people sign in with WebAuthn passkeys,
//! machines with scoped API keys, and both become a short session verified by
//! any node of the cluster.
//!
//! This crate is pure logic. Accounts, credentials and revocations are stored
//! by the caller (the node keeps them in the system group).

use ring::digest::{digest, SHA256};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};

pub mod webauthn;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use webauthn::{
    claimed_origin, verify_assertion, verify_consent, verify_registration, Assertion, Credential,
    PublicKey, Registration, RelyingParty,
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
    ring::rand::generate(&SystemRandom::new())
        .expect("system randomness is available")
        .expose()
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
    /// The passkey the session was opened with (its credential id): the
    /// device, for whoever asks which one this is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passkey: Option<String>,
    /// The package whose frontend origin opened the session: it reaches only
    /// that package's apps. `None` for the cluster's own origins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// The node that issued and signed the token.
    pub node: u64,
    /// That node's signing key (Ed25519, base64url): the key its attestation
    /// binds. A verifier that attested another key for the node attests it
    /// again (the node restarted).
    pub key: String,
}

/// What a token's signature covers before its claims.
const SESSION_DOMAIN: &[u8] = b"calimero.session.v2\0";

/// Mint a session token: `v2.<claims>.<sig>`, signed with the Ed25519 key
/// whose seed is `seed` (the issuing node's attested key) over
/// `"calimero.session.v2\0" || claims`. `claims.node` and `claims.key` name
/// the issuer.
#[must_use]
pub fn mint_session(seed: &[u8; 32], claims: &SessionClaims) -> String {
    let body = b64url(&serde_json::to_vec(claims).expect("claims serialize"));
    // SAFETY: any 32 bytes are an Ed25519 seed.
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).expect("a 32-byte seed");
    let signature = pair.sign(&[SESSION_DOMAIN, body.as_bytes()].concat());
    format!("v2.{body}.{}", b64url(signature.as_ref()))
}

/// The Ed25519 public key of `seed`, as a token's `key` names it.
#[must_use]
pub fn session_public(seed: &[u8; 32]) -> [u8; 32] {
    // SAFETY: any 32 bytes are an Ed25519 seed, whose public key is 32 bytes.
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).expect("a 32-byte seed");
    pair.public_key()
        .as_ref()
        .try_into()
        .expect("an Ed25519 public key is 32 bytes")
}

/// Read a token's claims without checking it, to find which issuer to check
/// it with.
pub fn peek_session(token: &str) -> Result<SessionClaims, Error> {
    let mut parts = token.split('.');
    let (Some("v2"), Some(body), Some(_), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Error::Malformed("session token"));
    };
    serde_json::from_slice(&from_b64url(body)?).map_err(|_| Error::Malformed("session claims"))
}

/// Check a token's signature under `public` (the issuer's attested key) and
/// its expiry against `now`. Which node issued it, and whether that node may
/// vouch for the account, is the caller's to judge.
pub fn verify_session(public: &[u8; 32], token: &str, now: u64) -> Result<SessionClaims, Error> {
    let mut parts = token.split('.');
    let (Some("v2"), Some(body), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Error::Malformed("session token"));
    };
    UnparsedPublicKey::new(&ED25519, public)
        .verify(
            &[SESSION_DOMAIN, body.as_bytes()].concat(),
            &from_b64url(signature)?,
        )
        .map_err(|_| Error::BadSignature)?;
    let claims: SessionClaims = serde_json::from_slice(&from_b64url(body)?)
        .map_err(|_| Error::Malformed("session claims"))?;
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
    fn a_saved_passkey_keeps_signing_and_counting() {
        let mut auth = SoftAuthenticator::new("localhost", "http://localhost:3000");
        let challenge = random::<32>();
        let mut cred = verify_registration(&rp(), &challenge, &auth.register(&challenge)).unwrap();
        cred.sign_count = verify_assertion(&rp(), &[6; 32], &cred, &auth.assert(&[6; 32])).unwrap();
        let mut loaded = SoftAuthenticator::from_json(&auth.to_json()).unwrap();
        assert_eq!(loaded.credential_id(), auth.credential_id());
        assert!(verify_assertion(&rp(), &[7; 32], &cred, &loaded.assert(&[7; 32])).is_ok());
        assert!(SoftAuthenticator::from_json("{}").is_err());
    }

    #[test]
    fn a_consent_verifies_from_public_data_only() {
        let mut auth = SoftAuthenticator::new("localhost", "http://localhost:3000");
        let challenge = random::<32>();
        let cred = verify_registration(&rp(), &challenge, &auth.register(&challenge)).unwrap();
        let a = auth.assert(&[8; 32]);
        assert_eq!(verify_consent(&[8; 32], &cred, &a), Ok(()));
        // Again: apply leaves the counter to the node that took it.
        assert_eq!(verify_consent(&[8; 32], &cred, &a), Ok(()));
        assert_eq!(
            verify_consent(&[9; 32], &cred, &a),
            Err(Error::Mismatch("challenge"))
        );
        let mut other = SoftAuthenticator::new("localhost", "http://localhost:3000");
        let c2 = random::<32>();
        let other_cred = verify_registration(&rp(), &c2, &other.register(&c2)).unwrap();
        let forged = Assertion {
            credential_id: cred.id.clone(),
            ..other.assert(&[8; 32])
        };
        assert_eq!(
            verify_consent(&[8; 32], &cred, &forged),
            Err(Error::BadSignature)
        );
        assert_eq!(
            verify_consent(&[8; 32], &other_cred, &a),
            Err(Error::Mismatch("credential"))
        );
    }

    #[test]
    fn node_signed_session_tokens() {
        let seed = random::<32>();
        let public = session_public(&seed);
        let claims = SessionClaims {
            account: "acc".into(),
            session: "s1".into(),
            expires_at: 1000,
            passkey: Some("cred".into()),
            app: None,
            node: 3,
            key: b64url(&public),
        };
        let token = mint_session(&seed, &claims);
        assert_eq!(verify_session(&public, &token, 999).unwrap(), claims);
        assert_eq!(peek_session(&token).unwrap(), claims);
        assert_eq!(verify_session(&public, &token, 1000), Err(Error::Expired));
        let other = session_public(&random::<32>());
        assert_eq!(verify_session(&other, &token, 1), Err(Error::BadSignature));
        // Claims swapped under a valid signature do not verify.
        let mut parts: Vec<&str> = token.split('.').collect();
        let forged = b64url(
            &serde_json::to_vec(&SessionClaims {
                account: "someone-else".into(),
                ..claims
            })
            .unwrap(),
        );
        parts[1] = &forged;
        assert_eq!(
            verify_session(&public, &parts.join("."), 1),
            Err(Error::BadSignature)
        );
    }

    #[test]
    fn api_keys_and_confirm_challenges() {
        let (key, hash) = new_api_key();
        assert!(key.starts_with("cal_"));
        assert_eq!(api_key_hash(&key), hash);
        let nonce = random::<16>();
        let a = confirm_challenge("app", "transfer", b"{\"to\":1}", &nonce);
        let b = confirm_challenge("app", "transfer", b"{\"to\":2}", &nonce);
        assert_ne!(a, b);
    }
}
