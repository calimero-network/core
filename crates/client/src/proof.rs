//! Signing one request, so a caller with no session can still be identified.
//!
//! A bearer token says "someone authenticated once". A request-carried proof
//! says "the holder of this device key is making *this* call" — it commits to
//! the method, the path and the body, so it cannot be lifted off one request
//! and replayed on another. That is what lets a client talk to a relay it has
//! no account on: nothing was ever issued to it.
//!
//! This is the client half of what `calimero-server` verifies. The types are
//! `calimero-account`'s own, so unlike the JS implementation there is no
//! encoding to get right here — only the decision of what to sign and when.

use calimero_account::{AccountProof, CallerProof, DeviceCert, LoginStatement, RequestSig};
use calimero_primitives::identity::PrivateKey;
use eyre::Result as EyreResult;

/// How long a signature stays valid, in seconds.
///
/// Short on purpose: the expiry is what bounds replay and nothing else does. It
/// only has to outlive the flight time, and the node allows a small clock skew
/// on top.
const DEFAULT_TTL_SECS: u64 = 120;

/// The keys and certificates a caller signs requests with.
///
/// Built once and reused: the certificates are fixed for the life of the
/// device, and only the per-request signature changes.
///
/// Deliberately neither `Clone` nor `Debug`-derived. `PrivateKey` is not
/// `Clone` on purpose — a signing key should have one home — so a connection
/// holds this behind an `Arc` rather than copying it, and `Debug` is written by
/// hand so a key cannot reach a log through a struct someone printed while
/// debugging something else.
pub struct RequestProofSigner {
    account_proof: AccountProof<DeviceCert>,
    session: Option<LoginStatement>,
    signer: PrivateKey,
    ttl_secs: u64,
}

impl std::fmt::Debug for RequestProofSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestProofSigner")
            .field("account", &self.account_proof.genesis.account_id())
            .field("session", &self.session.is_some())
            .field("ttl_secs", &self.ttl_secs)
            .finish_non_exhaustive()
    }
}

impl RequestProofSigner {
    /// A three-link chain: the device certified a session key, and that key
    /// signs each request.
    ///
    /// Prefer this where a session key exists. It keeps the device key out of
    /// the signing path, so a leaked session cannot be escalated into use of
    /// the device itself.
    #[must_use]
    pub const fn with_session(
        account_proof: AccountProof<DeviceCert>,
        session: LoginStatement,
        session_sk: PrivateKey,
    ) -> Self {
        Self {
            account_proof,
            session: Some(session),
            signer: session_sk,
            ttl_secs: DEFAULT_TTL_SECS,
        }
    }

    /// A two-link chain: the device key signs each request directly.
    ///
    /// This is `meroctl`'s shape. A CLI run by the person holding the device
    /// has nowhere better to put a session key than the same process, so the
    /// extra link would add a second secret without adding a boundary.
    #[must_use]
    pub const fn with_device_key(
        account_proof: AccountProof<DeviceCert>,
        device_sk: PrivateKey,
    ) -> Self {
        Self {
            account_proof,
            session: None,
            signer: device_sk,
            ttl_secs: DEFAULT_TTL_SECS,
        }
    }

    /// Override how long each signature is valid for.
    #[must_use]
    pub const fn with_ttl_secs(mut self, ttl_secs: u64) -> Self {
        self.ttl_secs = ttl_secs;
        self
    }

    /// Sign one request and return the `X-Calimero-Proof` value.
    ///
    /// `path` must be what the node will see and must carry no query string:
    /// the signature covers the path alone, and a mismatch reads as a bad
    /// signature rather than as a bad path.
    ///
    /// `body` must be the exact bytes that will be sent. Serializing again
    /// between here and the wire would produce a second spelling — a signature
    /// over bytes that never travelled.
    pub fn sign(&self, method: &str, path: &str, body: &[u8], now: u64) -> EyreResult<String> {
        let proof = CallerProof {
            account_proof: self.account_proof.clone(),
            session: self.session.clone(),
            request: RequestSig::sign(
                &self.signer,
                method,
                path,
                body,
                now,
                now.saturating_add(self.ttl_secs),
            )?,
        };
        Ok(hex::encode(borsh::to_vec(&proof)?))
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{AccountGenesis, Audience, KemPublicKey};
    use calimero_primitives::identity::DeviceId;

    use super::*;

    const METHOD: &str = "GET";
    const PATH: &str = "/admin-api/namespaces";
    const NOW: u64 = 1_700_000_000;

    /// The same chain `calimero-server`'s `proof_auth.rs` tests build, and the
    /// same one `mero-js` pins its encoder against.
    ///
    /// One byte string for three implementations: the verifier, the JS client
    /// and this one. If any of them drifts on the preimage or the field order,
    /// it stops matching the other two instead of failing only in production
    /// against a node nobody ran locally.
    fn key(seed: u8) -> PrivateKey {
        PrivateKey::from([seed; 32])
    }

    fn signer_for(with_session: bool) -> RequestProofSigner {
        let root = key(1);
        let genesis = AccountGenesis::new(root.public_key());
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [0x22; 16]);
        let device_sk = key(51);
        let session_sk = key(101);

        let cert = DeviceCert::sign(
            &root,
            account,
            device,
            &device_sk.public_key(),
            &KemPublicKey::from([9_u8; 32]),
            0,
            0,
        )
        .expect("cert");

        let account_proof = AccountProof {
            genesis,
            chain: vec![],
            statement: cert,
        };

        if with_session {
            let session = LoginStatement::sign(
                &device_sk,
                key(7).public_key(),
                Audience::Cli,
                [0x77; 32],
                session_sk.public_key(),
                NOW,
                NOW + 3600,
            )
            .expect("statement");
            RequestProofSigner::with_session(account_proof, session, session_sk).with_ttl_secs(300)
        } else {
            RequestProofSigner::with_device_key(account_proof, session_sk).with_ttl_secs(300)
        }
    }

    #[test]
    fn a_three_link_proof_matches_the_bytes_the_node_verifies() {
        assert_eq!(
            signer_for(true).sign(METHOD, PATH, b"", NOW).unwrap(),
            "028a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c0000000004cfa21629a77f8cd8ddd3f821ed514009a9f572b2ce8e0a11f5cbb5e25340b09aeef190d5865e90861a94ec2e0b28de56ff7412f13806ff78322eb2d7a7d71d17cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b85e18080ce09090909090909090909090909090909090909090909090909090909090909090000000000000000ffb06b89ad31e732ed1bbec65b9d9c0ba1aead6faa37850432387b56dd69278670f6bb57b945ebe1a83997761ede1faed02bd0e37c7a7f3fbbdf934c14bbbb0e01ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c027777777777777777777777777777777777777777777777777777777777777777d62f016a1efd1e4fdf793eb42cd84471e1ba9f0cf04d1287b5cc71f616287cb817cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b85e18080ce00f153650000000010ff53650000000056554af3cbf04380c22898aa2b15397f526d9dec50ab792b76fd45ca01668488be57b317015d9e4aada80a8dda0367f7390071051e2b3855c5e7eeaf0119520c03000000474554150000002f61646d696e2d6170692f6e616d65737061636573ea5bb92ca52d40a333e56e353225c6a569e2afbcb7a9152aa9727bed602872f900f15365000000002cf2536500000000299cd5a4a5566805c03ff461f68b9609d09ca0c82185fe488b6816a8cc1544e1e863d1a7b62274c7c0a4b30de72ad92e74ad1ebd9350976c6b3961430dfd2e02"
        );
    }

    /// meroctl's shape. Not the three-link value with a field removed — borsh
    /// writes the absent `Option` as a tag byte, so it is a different encoding
    /// and the node reads that tag before anything else.
    #[test]
    fn a_two_link_proof_is_a_different_encoding_not_a_shorter_one() {
        let two = signer_for(false).sign(METHOD, PATH, b"", NOW).unwrap();
        assert_eq!(two, "028a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c0000000004cfa21629a77f8cd8ddd3f821ed514009a9f572b2ce8e0a11f5cbb5e25340b09aeef190d5865e90861a94ec2e0b28de56ff7412f13806ff78322eb2d7a7d71d17cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b85e18080ce09090909090909090909090909090909090909090909090909090909090909090000000000000000ffb06b89ad31e732ed1bbec65b9d9c0ba1aead6faa37850432387b56dd69278670f6bb57b945ebe1a83997761ede1faed02bd0e37c7a7f3fbbdf934c14bbbb0e0003000000474554150000002f61646d696e2d6170692f6e616d65737061636573ea5bb92ca52d40a333e56e353225c6a569e2afbcb7a9152aa9727bed602872f900f15365000000002cf2536500000000299cd5a4a5566805c03ff461f68b9609d09ca0c82185fe488b6816a8cc1544e1e863d1a7b62274c7c0a4b30de72ad92e74ad1ebd9350976c6b3961430dfd2e02");
        assert_ne!(two, "028a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c0000000004cfa21629a77f8cd8ddd3f821ed514009a9f572b2ce8e0a11f5cbb5e25340b09aeef190d5865e90861a94ec2e0b28de56ff7412f13806ff78322eb2d7a7d71d17cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b85e18080ce09090909090909090909090909090909090909090909090909090909090909090000000000000000ffb06b89ad31e732ed1bbec65b9d9c0ba1aead6faa37850432387b56dd69278670f6bb57b945ebe1a83997761ede1faed02bd0e37c7a7f3fbbdf934c14bbbb0e01ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c027777777777777777777777777777777777777777777777777777777777777777d62f016a1efd1e4fdf793eb42cd84471e1ba9f0cf04d1287b5cc71f616287cb817cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b85e18080ce00f153650000000010ff53650000000056554af3cbf04380c22898aa2b15397f526d9dec50ab792b76fd45ca01668488be57b317015d9e4aada80a8dda0367f7390071051e2b3855c5e7eeaf0119520c03000000474554150000002f61646d696e2d6170692f6e616d65737061636573ea5bb92ca52d40a333e56e353225c6a569e2afbcb7a9152aa9727bed602872f900f15365000000002cf2536500000000299cd5a4a5566805c03ff461f68b9609d09ca0c82185fe488b6816a8cc1544e1e863d1a7b62274c7c0a4b30de72ad92e74ad1ebd9350976c6b3961430dfd2e02");
    }

    /// The body is committed to, so a different body is a different signature.
    /// Worth pinning because an empty body is the easy case to get right by
    /// accident and the one most requests use.
    #[test]
    fn the_body_changes_the_signature() {
        let signer = signer_for(true);
        let empty = signer.sign(METHOD, PATH, b"", NOW).unwrap();
        let full = signer.sign(METHOD, PATH, b"{}", NOW).unwrap();
        assert_ne!(empty, full);
    }

    /// A key never reaches a log through a struct someone printed.
    #[test]
    fn debug_does_not_render_the_signing_key() {
        let rendered = format!("{:?}", signer_for(true));
        assert!(!rendered.contains("65656565"), "{rendered}");
        assert!(rendered.contains("ttl_secs"), "{rendered}");
    }
}
