//! An anonymous visitor through a public link: the credential a request carries to call an
//! instance's `#[app::public]` methods (`Caller::Visitor { link, key }` on the node), made of
//! the link's token and the visitor's own Ed25519 key, signed fresh for each request.

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

/// What a public link token's key is derived under (the node's `proof::PUBLIC_LINK_DOMAIN`).
const PUBLIC_LINK_DOMAIN: &[u8] = b"calimero.public-link.v1\0";
/// What a visitor's proof signs before its fields (the node's `proof::VISITOR_DOMAIN`).
const VISITOR_DOMAIN: &[u8] = b"calimero.visitor.v1\0";

/// A visitor: the public link's token and the visitor's Ed25519 key (its 32-byte seed).
#[derive(Clone)]
pub struct Visitor {
    token: String,
    key: SigningKey,
}

impl std::fmt::Debug for Visitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Visitor")
            .field("key", &self.public_key())
            .finish_non_exhaustive()
    }
}

impl Visitor {
    /// A visitor of the link whose token is `token`, with the key whose seed is `seed`. Keep
    /// the seed to stay the same visitor.
    #[must_use]
    pub fn new(token: &str, seed: [u8; 32]) -> Self {
        Self {
            token: token.to_owned(),
            key: SigningKey::from_bytes(&seed),
        }
    }

    /// The link's key (hex): how the node and the app name the link.
    #[must_use]
    pub fn link(&self) -> String {
        link_key(&self.token)
    }

    /// The visitor's public key (64 lowercase hex): how the app names the visitor.
    #[must_use]
    pub fn public_key(&self) -> String {
        hex::encode(self.key.verifying_key().to_bytes())
    }

    /// The bearer credential for a request at `at` (Unix seconds):
    /// `vis1.<token>.<key>.<at>.<signature>`, valid within 5 minutes of the node's clock.
    #[must_use]
    pub fn bearer(&self, at: u64) -> String {
        let link = self.link();
        let key = self.public_key();
        let signature = hex::encode(self.key.sign(&message(&link, &key, at)).to_bytes());
        format!("vis1.{}.{key}.{at}.{signature}", self.token)
    }
}

/// A public link token's public key, hex: `SHA-256("calimero.public-link.v1\0" || token)` is
/// its seed.
#[must_use]
pub fn link_key(token: &str) -> String {
    let seed: [u8; 32] = Sha256::new()
        .chain_update(PUBLIC_LINK_DOMAIN)
        .chain_update(token.as_bytes())
        .finalize()
        .into();
    hex::encode(SigningKey::from_bytes(&seed).verifying_key().to_bytes())
}

/// The bytes a visitor signs: `"calimero.visitor.v1\0" || link || 0 || key || 0 || at`.
fn message(link: &str, key: &str, at: u64) -> Vec<u8> {
    let mut m = VISITOR_DOMAIN.to_vec();
    m.extend_from_slice(link.as_bytes());
    m.push(0);
    m.extend_from_slice(key.as_bytes());
    m.push(0);
    m.extend_from_slice(at.to_string().as_bytes());
    m
}

#[cfg(test)]
mod tests {
    use super::Visitor;

    /// The credential's shape: the token, the key, the time and a signature that verifies
    /// under the key over the message the node rebuilds.
    #[test]
    fn a_visitors_bearer_names_its_token_key_and_time() {
        use ed25519_dalek::Verifier;
        let v = Visitor::new("pl_token", [9; 32]);
        let bearer = v.bearer(1_000);
        let parts: Vec<&str> = bearer.split('.').collect();
        assert_eq!(parts[0], "vis1");
        assert_eq!(parts[1], "pl_token");
        assert_eq!(parts[2], v.public_key());
        assert_eq!(parts[3], "1000");
        let sig = ed25519_dalek::Signature::from_slice(&hex::decode(parts[4]).unwrap()).unwrap();
        let public = ed25519_dalek::VerifyingKey::from_bytes(
            &hex::decode(v.public_key()).unwrap().try_into().unwrap(),
        )
        .unwrap();
        assert!(public
            .verify(&super::message(&v.link(), &v.public_key(), 1_000), &sig)
            .is_ok());
        assert_ne!(v.link(), Visitor::new("pl_other", [9; 32]).link());
    }

    /// The vector mero-js checks too (`src/client/visitor.test.ts`): the same link key,
    /// visitor key and credential (Ed25519 signs deterministically).
    #[test]
    fn the_shared_visitor_vector_holds() {
        let v = Visitor::new("pl_token", [9; 32]);
        assert_eq!(
            v.link(),
            "d4205f1ac7bb3fe43278131b6e621ebf952f0a5f51a38505327572d863c46c42"
        );
        assert_eq!(
            v.public_key(),
            "fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618"
        );
        assert_eq!(v.bearer(1_000), VECTOR_BEARER);
    }

    const VECTOR_BEARER: &str = "vis1.pl_token.fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618.1000.9b5205e4deb4b534a612c3b03be773b60aea16192f2fb6d99496d194f2dd259173173731e56a46ec7b37cc3d2d2482fe7be5d70c0fadda54c22f5ea2b8fa4e09";
}
