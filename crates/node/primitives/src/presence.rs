//! What an author signs for one ephemeral-presence update.
//!
//! One format for nodes and accounts. A node signs with its context identity and
//! carries no certificate. An account signs with its device key and carries the
//! `AccountProof<DeviceCert>` that ties that key to the account; a relay only
//! seals and forwards it. The whole update travels inside the AEAD, so nothing a
//! receiver relies on rides in the clear.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::{AccountProof, DeviceCert, DeviceId};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{AccountId, PrivateKey, PublicKey};
use sha2::{Digest, Sha256};

/// Part of the signed bytes; never change it without changing the wire.
pub const PRESENCE_DOMAIN: &[u8; 19] = b"calimero/presence/1";

/// What an author signs: its borsh, signed raw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PresenceStatement {
    pub domain: [u8; 19],
    pub context_id: ContextId,
    /// A node's context identity, or an account's device key.
    pub author: PublicKey,
    /// Per `(context, author)`; a receiver keeps the highest.
    pub seq: u64,
    /// The author's wall clock, checked against the receiver's.
    pub sent_at_ms: u64,
    /// `sha256(borsh(state))`, so a retract (`None`) is signed too.
    pub state_hash: [u8; 32],
}

/// One update as it travels, sealed, inside `BroadcastMessage::Ephemeral`.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct PresenceUpdate {
    pub statement: PresenceStatement,
    /// By `statement.author` over [`PresenceStatement::signing_bytes`].
    pub signature: [u8; 64],
    /// `None` for a node; the device certificate for an account.
    pub certificate: Option<AccountProof<DeviceCert>>,
    /// The slice, or `None` to retract.
    pub state: Option<Vec<u8>>,
}

/// What [`PresenceUpdate::verify`] establishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedPresence {
    pub author: PublicKey,
    /// The account the certificate names, when there is one.
    pub account: Option<AccountId>,
    /// That certificate's device, for the revocation check.
    pub device: Option<DeviceId>,
    pub seq: u64,
    pub sent_at_ms: u64,
}

/// One live presence entry as the node's snapshot reports it:
/// `(author, account, slice, age_ms)`, the age relative to the node's clock.
pub type PresenceSnapshotEntry = (PublicKey, Option<AccountId>, Vec<u8>, u64);

/// Why an update does not verify on its own.
#[derive(Debug, thiserror::Error)]
pub enum PresenceRefusal {
    #[error("not a presence statement")]
    WrongDomain,
    #[error("the statement names another context")]
    ContextMismatch,
    #[error("the state does not match the signed hash")]
    StateHashMismatch,
    #[error("the signature does not verify under the author")]
    BadSignature,
    #[error("the device certificate does not verify: {0}")]
    CertificateInvalid(String),
    #[error("the certificate is for another key than the author")]
    CertificateKeyMismatch,
}

/// Why a relay will not publish an account's update.
///
/// The server maps each to one HTTP status; see `presence-intents`.
#[derive(Clone, Debug, thiserror::Error)]
pub enum DelegatedPresenceError {
    /// The update does not verify on its own.
    #[error("{0}")]
    Refused(String),
    /// Its signed stamp is outside the freshness window.
    #[error("the statement is outside the freshness window")]
    Stale,
    /// It carries no device certificate: a node publishes its own presence.
    #[error("the update carries no device certificate")]
    NotAnAccount,
    /// The account is not a member of the context's group.
    #[error("the account is not a member of this context")]
    NotAMember,
    /// The device was revoked in the context's group.
    #[error("the device is revoked here")]
    DeviceRevoked,
    /// The slice is over `EPHEMERAL_MAX_BYTES`.
    #[error("the presence slice is too large: {0} bytes")]
    TooLarge(usize),
    /// Another update from this device arrived too recently.
    #[error("too many presence updates; slow down")]
    RateLimited,
    /// This relay holds no current key for the context.
    #[error("this relay holds no current key for the context")]
    NoGroupKey,
    /// Anything else: a store or actor failure.
    #[error("{0}")]
    Internal(String),
}

/// `sha256(borsh(state))`: `None` is `[0]`, `Some(v)` is `[1] ‖ u32le(len) ‖ v`.
pub fn state_hash(state: &Option<Vec<u8>>) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(5 + state.as_ref().map_or(0, Vec::len));
    match state {
        None => bytes.push(0),
        Some(slice) => {
            bytes.push(1);
            // A slice is capped far below u32::MAX (EPHEMERAL_MAX_BYTES), as borsh requires.
            bytes.extend_from_slice(&u32::try_from(slice.len()).unwrap_or(u32::MAX).to_le_bytes());
            bytes.extend_from_slice(slice);
        }
    }
    Sha256::digest(bytes).into()
}

impl PresenceStatement {
    pub fn new(
        context_id: ContextId,
        author: PublicKey,
        seq: u64,
        sent_at_ms: u64,
        state: &Option<Vec<u8>>,
    ) -> Self {
        Self {
            domain: *PRESENCE_DOMAIN,
            context_id,
            author,
            seq,
            sent_at_ms,
            state_hash: state_hash(state),
        }
    }

    /// The exact bytes an author signs: the statement's borsh.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(19 + 32 + 32 + 8 + 8 + 32);
        bytes.extend_from_slice(&self.domain);
        bytes.extend_from_slice(self.context_id.as_ref());
        bytes.extend_from_slice(self.author.as_ref());
        bytes.extend_from_slice(&self.seq.to_le_bytes());
        bytes.extend_from_slice(&self.sent_at_ms.to_le_bytes());
        bytes.extend_from_slice(&self.state_hash);
        bytes
    }
}

impl PresenceUpdate {
    /// Sign an update as `sk`. A node passes `certificate: None`.
    pub fn signed(
        sk: &PrivateKey,
        context_id: ContextId,
        seq: u64,
        sent_at_ms: u64,
        state: Option<Vec<u8>>,
        certificate: Option<AccountProof<DeviceCert>>,
    ) -> eyre::Result<Self> {
        let statement =
            PresenceStatement::new(context_id, sk.public_key(), seq, sent_at_ms, &state);
        let signature = sk
            .sign(&statement.signing_bytes())
            .map_err(|err| eyre::eyre!("failed to sign a presence statement: {err}"))?
            .to_bytes();
        Ok(Self {
            statement,
            signature,
            certificate,
            state,
        })
    }

    /// Everything decidable from the update alone. Freshness, membership and
    /// revocation need a clock and a store, and are the caller's.
    pub fn verify(&self, context_id: ContextId) -> Result<VerifiedPresence, PresenceRefusal> {
        let statement = &self.statement;
        if &statement.domain != PRESENCE_DOMAIN {
            return Err(PresenceRefusal::WrongDomain);
        }
        if statement.context_id != context_id {
            return Err(PresenceRefusal::ContextMismatch);
        }
        if statement.state_hash != state_hash(&self.state) {
            return Err(PresenceRefusal::StateHashMismatch);
        }
        statement
            .author
            .verify_raw_signature(&statement.signing_bytes(), &self.signature)
            .map_err(|_| PresenceRefusal::BadSignature)?;

        let (account, device) = match &self.certificate {
            None => (None, None),
            Some(proof) => {
                // Both steps: the proof is the account's, AND it is about this key.
                let cert = proof
                    .verify(proof.statement.account)
                    .map_err(|err| PresenceRefusal::CertificateInvalid(err.to_string()))?;
                if cert.sign_pk != statement.author {
                    return Err(PresenceRefusal::CertificateKeyMismatch);
                }
                (Some(cert.account), Some(cert.device))
            }
        };

        Ok(VerifiedPresence {
            author: statement.author,
            account,
            device,
            seq: statement.seq,
            sent_at_ms: statement.sent_at_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{AccountGenesis, AccountProof, DeviceCert, DeviceId, KemPublicKey};
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    const SENT_AT: u64 = 1_700_000_000_000;

    fn ctx() -> ContextId {
        ContextId::from([0x11u8; 32])
    }

    fn author_sk() -> PrivateKey {
        PrivateKey::from([0x22u8; 32])
    }

    fn typing() -> Option<Vec<u8>> {
        Some(br#"{"typing":true}"#.to_vec())
    }

    /// A device key and the root-signed certificate that ties it to an account.
    fn test_account_device() -> (PrivateKey, AccountProof<DeviceCert>) {
        let root_sk = PrivateKey::from([0x55u8; 32]);
        let genesis = AccountGenesis::new(root_sk.public_key());
        let device_sk = PrivateKey::from([0x66u8; 32]);
        let cert = DeviceCert::sign(
            &root_sk,
            genesis.account_id(),
            DeviceId::from([0x77u8; 32]),
            &device_sk.public_key(),
            &KemPublicKey::from([0x88u8; 32]),
            0,
            1,
        )
        .expect("the root signs its device");
        (
            device_sk,
            AccountProof {
                genesis,
                chain: vec![],
                statement: cert,
            },
        )
    }

    #[test]
    fn node_update_round_trips_and_verifies() {
        let update =
            PresenceUpdate::signed(&author_sk(), ctx(), 7, SENT_AT, typing(), None).unwrap();
        let bytes = borsh::to_vec(&update).unwrap();
        let back: PresenceUpdate = borsh::from_slice(&bytes).unwrap();
        let verified = back.verify(ctx()).expect("verifies");
        assert_eq!(verified.author, author_sk().public_key());
        assert_eq!(verified.account, None);
        assert_eq!(verified.seq, 7);
    }

    #[test]
    fn refuses_another_context() {
        let update =
            PresenceUpdate::signed(&author_sk(), ctx(), 7, SENT_AT, typing(), None).unwrap();
        assert!(matches!(
            update.verify(ContextId::from([0x99u8; 32])),
            Err(PresenceRefusal::ContextMismatch)
        ));
    }

    #[test]
    fn refuses_swapped_state() {
        let mut update =
            PresenceUpdate::signed(&author_sk(), ctx(), 7, SENT_AT, typing(), None).unwrap();
        update.state = Some(br#"{"typing":false}"#.to_vec());
        assert!(matches!(
            update.verify(ctx()),
            Err(PresenceRefusal::StateHashMismatch)
        ));
    }

    #[test]
    fn refuses_a_signature_by_another_key() {
        let mut update =
            PresenceUpdate::signed(&author_sk(), ctx(), 7, SENT_AT, typing(), None).unwrap();
        update.statement.author = PrivateKey::from([0x44u8; 32]).public_key();
        assert!(matches!(
            update.verify(ctx()),
            Err(PresenceRefusal::BadSignature)
        ));
    }

    #[test]
    fn retract_hashes_none() {
        let update = PresenceUpdate::signed(&author_sk(), ctx(), 8, SENT_AT, None, None).unwrap();
        assert!(update.verify(ctx()).is_ok());
        assert_eq!(update.statement.state_hash, state_hash(&None));
        assert_ne!(state_hash(&None), state_hash(&Some(vec![])));
    }

    #[test]
    fn account_update_names_the_account() {
        let (device_sk, proof) = test_account_device();
        let account = proof.statement.account;
        let update =
            PresenceUpdate::signed(&device_sk, ctx(), 7, SENT_AT, typing(), Some(proof)).unwrap();
        let verified = update.verify(ctx()).expect("verifies");
        assert_eq!(verified.account, Some(account));
        assert_eq!(verified.device, Some(DeviceId::from([0x77u8; 32])));
    }

    #[test]
    fn refuses_a_certificate_for_another_key() {
        let (_device_sk, proof) = test_account_device();
        let update =
            PresenceUpdate::signed(&author_sk(), ctx(), 7, SENT_AT, typing(), Some(proof)).unwrap();
        assert!(matches!(
            update.verify(ctx()),
            Err(PresenceRefusal::CertificateKeyMismatch)
        ));
    }

    #[test]
    fn refuses_a_certificate_from_another_root() {
        let (device_sk, mut proof) = test_account_device();
        // A genesis that does not hash to the certificate's account.
        proof.genesis = AccountGenesis::new(PrivateKey::from([0x99u8; 32]).public_key());
        let update =
            PresenceUpdate::signed(&device_sk, ctx(), 7, SENT_AT, typing(), Some(proof)).unwrap();
        assert!(matches!(
            update.verify(ctx()),
            Err(PresenceRefusal::CertificateInvalid(_))
        ));
    }

    /// The hand-written bytes are exactly borsh's, so any borsh reader agrees.
    #[test]
    fn signing_bytes_and_state_hash_are_borsh() {
        let statement =
            PresenceStatement::new(ctx(), author_sk().public_key(), 7, SENT_AT, &typing());
        assert_eq!(
            statement.signing_bytes(),
            borsh::to_vec(&statement).unwrap()
        );
        for state in [None, Some(vec![]), typing()] {
            let expected: [u8; 32] = Sha256::digest(borsh::to_vec(&state).unwrap()).into();
            assert_eq!(state_hash(&state), expected);
        }
    }

    /// Pinned for mero-js (`src/presence/statement.test.ts` asserts the same hex).
    #[test]
    fn statement_vector_is_stable() {
        let statement =
            PresenceStatement::new(ctx(), author_sk().public_key(), 7, SENT_AT, &typing());
        assert_eq!(
            hex::encode(statement.signing_bytes()),
            "63616c696d65726f2f70726573656e63652f311111111111111111111111111111111111111111111111111111111111111111a09aa5f47a6759802ff955f8dc2d2a14a5c99d23be97f864127ff9383455a4f007000000000000000068e5cf8b010000f25c119b355e48fcfc5c8acd6b7681d0d8dfe33f75ee6408aa162e13b05ca486",
            "printed: {}",
            hex::encode(statement.signing_bytes())
        );
    }
}
