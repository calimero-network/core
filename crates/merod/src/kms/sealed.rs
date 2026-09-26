//! Sealed key release: the key the KMS releases is encrypted to this TD.
//!
//! The key used to cross the wire as plain hex inside the TLS session, and TLS
//! ended wherever `kms-phala-url` pointed. That URL comes from instance metadata,
//! which the operator writes. So an operator could stand an HTTPS proxy with any
//! public-CA certificate in front of the genuine KMS and forward every request
//! untouched. The node's quote was genuine, the KMS's quote was genuine, and every
//! check passed — while the proxy read the storage key as it went by.
//!
//! Sealing closes that without trusting the transport at all:
//!
//! * The KMS holds a long-lived X25519 **transport key**, derived inside its TD
//!   (so every replica of one KMS app holds the same one). `/attest` returns its
//!   public half and commits to it in the quote's report data
//!   ([`attest_binding`]), so a key the node accepts is one only a genuine KMS
//!   holds.
//! * For each release the node makes a one-time X25519 key and commits to its
//!   public half in ITS quote ([`request_binding`]). The KMS encrypts the storage
//!   key to that key ([`EphemeralSealKey::open`] is the node's side). Nobody but
//!   this TD can open the result, and nobody but the attested KMS can have made
//!   it: a proxy that swaps in its own one-time key would need a fresh quote, and
//!   one that answers with a key of its own choosing cannot produce a ciphertext
//!   that authenticates under the attested transport key.
//!
//! Wire format, which mero-kms implements identically:
//!
//! ```text
//! attest report_data  = nonce || SHA256(ATTEST_DOMAIN || binding || kms_pk)
//! get-key report_data = challenge_nonce || SHA256(REQUEST_DOMAIN || seal_to || peer_id)
//! aead key            = HKDF-SHA256(salt = challenge_nonce,
//!                                   ikm  = X25519(kms_sk, seal_to),
//!                                   info = SEAL_DOMAIN || kms_pk || seal_to || peer_id)
//! sealed key          = AES-256-GCM(aead key, 12-byte nonce, aad = peer_id, key hex)
//! ```

use curve25519_dalek::montgomery::MontgomeryPoint;
use eyre::{bail, Result};
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::hkdf::{Salt, HKDF_SHA256};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Domain for the KMS's commitment to its transport key in `/attest`.
const ATTEST_DOMAIN: &[u8] = b"mero-kms/attest-transport-key/v1";
/// Domain for the node's commitment to its one-time key in the get-key quote.
const REQUEST_DOMAIN: &[u8] = b"mero-kms/sealed-get-key/v1";
/// HKDF info domain for the key that seals the released key.
const SEAL_DOMAIN: &[u8] = b"mero-kms/sealed-key/v1";

/// The 32 bytes `/attest` puts after the nonce when it reports a transport key.
pub(super) fn attest_binding(binding: &[u8; 32], kms_public: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ATTEST_DOMAIN);
    hasher.update(binding);
    hasher.update(kms_public);
    hasher.finalize().into()
}

/// The 32 bytes the node's get-key quote puts after the challenge nonce.
pub(super) fn request_binding(seal_to: &[u8; 32], peer_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(REQUEST_DOMAIN);
    hasher.update(seal_to);
    hasher.update(peer_id.as_bytes());
    hasher.finalize().into()
}

/// The one-time key a single release is sealed to.
pub(super) struct EphemeralSealKey {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl EphemeralSealKey {
    pub(super) fn generate() -> Self {
        let mut secret = Zeroizing::new([0u8; 32]);
        UnwrapErr(SysRng).fill_bytes(secret.as_mut());
        let public = MontgomeryPoint::mul_base_clamped(*secret).0;
        Self { secret, public }
    }

    pub(super) fn public(&self) -> &[u8; 32] {
        &self.public
    }

    /// Open a key the KMS sealed to this key, authenticating it as the work of
    /// the holder of `kms_public`.
    pub(super) fn open(
        &self,
        kms_public: &[u8; 32],
        challenge_nonce: &[u8; 32],
        peer_id: &str,
        seal_nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>> {
        let Ok(seal_nonce) = <[u8; NONCE_LEN]>::try_from(seal_nonce) else {
            bail!("sealed key nonce must be {NONCE_LEN} bytes");
        };
        let shared = Zeroizing::new(MontgomeryPoint(*kms_public).mul_clamped(*self.secret).0);
        let key = seal_key(&shared, challenge_nonce, kms_public, &self.public, peer_id)?;
        let mut buffer = Zeroizing::new(ciphertext.to_vec());
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(seal_nonce),
                Aad::from(peer_id.as_bytes()),
                buffer.as_mut(),
            )
            .map_err(|_| {
                eyre::eyre!(
                    "the sealed key did not open: it was not sealed to this request by the \
                     attested KMS"
                )
            })?;
        Ok(Zeroizing::new(plaintext.to_vec()))
    }
}

fn seal_key(
    shared: &[u8; 32],
    challenge_nonce: &[u8; 32],
    kms_public: &[u8; 32],
    seal_to: &[u8; 32],
    peer_id: &str,
) -> Result<LessSafeKey> {
    // A low-order point yields an all-zero secret that anybody can compute.
    if shared.iter().all(|byte| *byte == 0) {
        bail!("the KMS transport key is a low-order point");
    }
    let info: [&[u8]; 4] = [SEAL_DOMAIN, kms_public, seal_to, peer_id.as_bytes()];
    let prk = Salt::new(HKDF_SHA256, challenge_nonce).extract(shared);
    let okm = prk
        .expand(&info, &AES_256_GCM)
        .map_err(|_| eyre::eyre!("HKDF expansion failed"))?;
    Ok(LessSafeKey::new(UnboundKey::from(okm)))
}

/// The KMS's side of the seal, for the tests' stand-in KMS.
#[cfg(test)]
pub(super) fn seal_for_test(
    kms_secret: &[u8; 32],
    seal_to: &[u8; 32],
    challenge_nonce: &[u8; 32],
    peer_id: &str,
    seal_nonce: [u8; NONCE_LEN],
    key_hex: &str,
) -> Vec<u8> {
    let kms_public = MontgomeryPoint::mul_base_clamped(*kms_secret).0;
    let shared = MontgomeryPoint(*seal_to).mul_clamped(*kms_secret).0;
    let key = seal_key(&shared, challenge_nonce, &kms_public, seal_to, peer_id).unwrap();
    let mut buffer = key_hex.as_bytes().to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(seal_nonce),
        Aad::from(peer_id.as_bytes()),
        &mut buffer,
    )
    .unwrap();
    buffer
}

#[cfg(test)]
pub(super) fn public_for_test(secret: &[u8; 32]) -> [u8; 32] {
    MontgomeryPoint::mul_base_clamped(*secret).0
}

#[cfg(test)]
impl EphemeralSealKey {
    pub(super) fn from_secret_for_test(secret: [u8; 32]) -> Self {
        let public = MontgomeryPoint::mul_base_clamped(secret).0;
        Self {
            secret: Zeroizing::new(secret),
            public,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: &str = "12D3KooWPeer";
    const NONCE: [u8; 32] = [0x11; 32];
    const KMS_SECRET: [u8; 32] = [0x22; 32];
    const NODE_SECRET: [u8; 32] = [0x33; 32];
    const SEAL_NONCE: [u8; NONCE_LEN] = [0x44; NONCE_LEN];
    const KEY_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn a_key_sealed_to_this_request_opens() {
        let node = EphemeralSealKey::from_secret_for_test(NODE_SECRET);
        let kms_public = public_for_test(&KMS_SECRET);
        let sealed = seal_for_test(
            &KMS_SECRET,
            node.public(),
            &NONCE,
            PEER,
            SEAL_NONCE,
            KEY_HEX,
        );
        let opened = node
            .open(&kms_public, &NONCE, PEER, &SEAL_NONCE, &sealed)
            .unwrap();
        assert_eq!(opened.as_slice(), KEY_HEX.as_bytes());
    }

    /// A proxy that answers get-key itself knows the node's one-time public key
    /// (it is in the request) but not the attested KMS's secret, so whatever it
    /// seals does not open against the transport key the node attested.
    #[test]
    fn a_key_sealed_by_anyone_but_the_attested_kms_does_not_open() {
        let node = EphemeralSealKey::from_secret_for_test(NODE_SECRET);
        let attested_kms = public_for_test(&KMS_SECRET);
        let forged = seal_for_test(
            &[0x55; 32],
            node.public(),
            &NONCE,
            PEER,
            SEAL_NONCE,
            KEY_HEX,
        );
        assert!(node
            .open(&attested_kms, &NONCE, PEER, &SEAL_NONCE, &forged)
            .is_err());
    }

    #[test]
    fn a_seal_bound_to_another_request_does_not_open() {
        let node = EphemeralSealKey::from_secret_for_test(NODE_SECRET);
        let kms_public = public_for_test(&KMS_SECRET);
        let sealed = seal_for_test(
            &KMS_SECRET,
            node.public(),
            &NONCE,
            PEER,
            SEAL_NONCE,
            KEY_HEX,
        );
        assert!(node
            .open(&kms_public, &[0x12; 32], PEER, &SEAL_NONCE, &sealed)
            .is_err());
        assert!(node
            .open(&kms_public, &NONCE, "12D3KooWOther", &SEAL_NONCE, &sealed)
            .is_err());
        let other = EphemeralSealKey::from_secret_for_test([0x66; 32]);
        assert!(other
            .open(&kms_public, &NONCE, PEER, &SEAL_NONCE, &sealed)
            .is_err());
    }

    #[test]
    fn a_low_order_transport_key_is_refused() {
        let node = EphemeralSealKey::from_secret_for_test(NODE_SECRET);
        let err = node
            .open(&[0u8; 32], &NONCE, PEER, &SEAL_NONCE, &[0u8; 80])
            .unwrap_err();
        assert!(err.to_string().contains("low-order"), "{err}");
    }

    /// Fixed vectors, repeated verbatim in mero-kms's tests: the two sides are
    /// separate implementations of one wire format, and this is what keeps them
    /// the same format.
    #[test]
    fn the_wire_format_matches_the_published_vectors() {
        let kms_public = public_for_test(&KMS_SECRET);
        let node = EphemeralSealKey::from_secret_for_test(NODE_SECRET);
        let actual = [
            hex::encode(attest_binding(&[0x77; 32], &kms_public)),
            hex::encode(request_binding(node.public(), PEER)),
            hex::encode(seal_for_test(
                &KMS_SECRET,
                node.public(),
                &NONCE,
                PEER,
                SEAL_NONCE,
                KEY_HEX,
            )),
        ];
        assert_eq!(
            actual,
            [
                ATTEST_BINDING_VECTOR,
                REQUEST_BINDING_VECTOR,
                SEALED_KEY_VECTOR
            ]
        );
    }

    // Also reproduced independently with Python's `cryptography` (X25519, HKDF,
    // AESGCM), so the format is standard and not merely self-consistent.
    const ATTEST_BINDING_VECTOR: &str =
        "9554b7960a1e1e3493ba075885680dc2c8c17f3c0caf7c5fc97a80400703acf7";
    const REQUEST_BINDING_VECTOR: &str =
        "9ef1ee8c46f5fa7b8ed4bcec0255cd88ec847674a6c6aca13eb5076c85af9f17";
    const SEALED_KEY_VECTOR: &str = "ebb35e8aa078b188ac8118d8a5c57f63ebe9431c41d887a1df61425b18d8f6b1\
        7287f6c7e3d81ea1af1ce06ce3cdc9ccce726b637498bf43c290a1dce1d9231a201a46995aa2dcc60f1224f553c29730";
}
