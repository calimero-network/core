//! **Account identity** — one identity, many devices.
//!
//! The unified causal log authenticates an op with the key that signed it and
//! authorizes it against a folded ACL. Historically those were the *same* key:
//! a per-namespace member key that simultaneously served as the signing key,
//! the ACL subject, the scope-key delivery recipient, and the CRDT replica id.
//! One key doing four jobs is why a person could not hold two devices — sharing
//! the key corrupts the CRDT planes (counter slots and HLC seeds are keyed by
//! replica id and assume one writer each), and *not* sharing it splits one
//! person into two unrelated members.
//!
//! This crate introduces the two ids that break the conflation:
//!
//! - [`AccountId`] — the person. The **only** authorization subject, and the
//!   app-visible "who wrote this". Never signs anything itself.
//! - [`DeviceId`] — one installation. The CRDT replica id, and the holder of
//!   the keypair that actually signs ops. **Never** an authorization input.
//!
//! # Ids are not keys
//!
//! The governing principle: *every identity is a stable id whose current key
//! set is projected state*. Both ids are content addresses, not public keys, so
//! every key in the system can rotate without the identity changing. Making
//! `AccountId` the account's public key — the obvious shortcut — would mean the
//! root key can never rotate (rotating would mean becoming a different person
//! and losing all membership and authorship), and it would force the root key
//! out of cold storage for routine work.
//!
//! # Self-certifying anchor
//!
//! [`AccountId`] is the content address of the account's [`AccountGenesis`],
//! and the genesis names the epoch-0 root key. So *the initial root key is
//! recoverable from the account id itself*: a verifier handed a
//! [`DeviceCert`] can check `AccountGenesis::account_id() == cert.account` and
//! then walk a signed [`RootKeyHandoff`] chain up to the cert's epoch, with **no
//! prior state and no ordering dependency**. That is what lets a device link
//! itself into a scope in a single self-contained op.
//!
//! What this crate does *not* do is decide anything. It is pure verification of
//! self-contained credentials; the at-cut checks that make a credential
//! *authoritative* (has this root key been superseded? has this device been
//! revoked? is the account even a member here?) belong to the projection and
//! `calimero-authz`, because only those see the causal cut.

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use thiserror::Error as ThisError;

use calimero_primitives::identity::{PrivateKey, PublicKey};

/// Version tag written into [`AccountGenesis`]. It is part of the preimage of
/// [`AccountId`], so bumping it makes every id under the new version distinct
/// from every id under the old one — a deliberate hard fork of the namespace
/// rather than a silent reinterpretation of existing ids.
pub const ACCOUNT_GENESIS_VERSION: u8 = 1;

/// Max root-key handoffs in one credential chain.
///
/// Each entry costs an Ed25519 verification in [`resolve_root_keys`], on a path
/// reachable from untrusted bytes, so an uncapped chain is verification
/// amplification. Generous against real use: an account rotating its root key
/// daily would take over two years to reach it.
pub const MAX_ROOT_KEY_HANDOFFS: usize = 1_024;

/// Domain separator for the [`AccountId`] content address.
const ACCOUNT_ID_DOMAIN: &[u8] = b"calimero.account.genesis.v1";
/// Domain separator for the [`DeviceId`] content address.
const DEVICE_ID_DOMAIN: &[u8] = b"calimero.device.id.v1";
/// Domain separator for the bytes a root key signs to hand off to its successor.
const HANDOFF_SIGN_DOMAIN: &[u8] = b"calimero.account.handoff.v1";
/// Domain separator for the bytes a root key signs to grant a device.
const DEVICE_CERT_SIGN_DOMAIN: &[u8] = b"calimero.device.cert.v1";

/// Domain for deriving a per-namespace account nonce from the node's account root
/// secret. Distinct from every signing and id domain, so a derived nonce can never
/// be confused with a signature preimage or an id.
const ACCOUNT_NONCE_DOMAIN: &[u8] = b"calimero.account.nonce.v1";

/// Domain for a member's endorsement of an account. Distinct from every other
/// signing domain, so an endorsement signature can never be replayed as a device
/// certificate, a handoff, or an op.
const ACCOUNT_ENDORSEMENT_SIGN_DOMAIN: &[u8] = b"calimero.account.endorsement.v1";

/// Domain for a root-signed device revocation.
const DEVICE_REVOCATION_SIGN_DOMAIN: &[u8] = b"calimero.device.revocation.v1";

/// Every signing domain used by this crate, for the test that asserts they are
/// pairwise distinct. A collision here would let a signature minted for one
/// purpose be replayed as another.
#[cfg(test)]
const ALL_DOMAINS: &[&[u8]] = &[
    ACCOUNT_ID_DOMAIN,
    DEVICE_ID_DOMAIN,
    HANDOFF_SIGN_DOMAIN,
    DEVICE_CERT_SIGN_DOMAIN,
    ACCOUNT_ENDORSEMENT_SIGN_DOMAIN,
    DEVICE_REVOCATION_SIGN_DOMAIN,
];

/// Hash `domain ‖ parts` — the one hashing helper, so every content address and
/// signing preimage in this crate is domain-separated the same way.
///
/// The domain is length-prefixed rather than merely concatenated: with a bare
/// concatenation, a shorter domain whose bytes are a prefix of a longer one
/// could be made to produce the same digest by shifting bytes between the
/// domain and the body.
fn domain_hash(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update((domain.len() as u64).to_le_bytes());
    hasher.update(domain);
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// Derive the genesis nonce for `namespace_id` from the node's account root
/// secret.
///
/// Derived rather than stored, and that is what makes recovery possible: a stored
/// nonce lives on the node, so losing every device loses the nonces and the root
/// can no longer *name* the accounts it owns. With derivation, the whole recovery
/// input is one secret plus a list of namespace ids — and the list is not secret.
///
/// Per-namespace rather than one nonce for the root, because that is what lets
/// recovery and unlinkability coexist. One root spans every namespace, but each
/// yields a **different** `AccountId`, so nobody correlates a person across
/// namespaces. A single shared nonce would make every account id equal and link
/// them all.
///
/// Takes the root **secret**, not its public key. The public key travels in every
/// genesis, so deriving from it would let any observer compute the account ids for
/// namespaces it has never seen — reintroducing exactly the correlation the
/// per-namespace nonce exists to prevent.
#[must_use]
pub fn derive_account_nonce(root_secret: &[u8; 32], namespace_id: &[u8; 32]) -> [u8; 16] {
    let full = domain_hash(ACCOUNT_NONCE_DOMAIN, &[root_secret, namespace_id]);
    let mut nonce = [0u8; 16];
    nonce.copy_from_slice(&full[..16]);
    nonce
}

/// Serialize with borsh into a `Vec<u8>`.
///
/// # Panics
/// Never in practice — borsh-serializing these plain-data types into an
/// in-memory buffer is infallible; the `expect` documents that invariant.
fn borsh_bytes<T: BorshSerialize>(value: &T) -> Vec<u8> {
    borsh::to_vec(value).expect("borsh serialization of a plain-data type is infallible")
}

macro_rules! content_address_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash,
            BorshSerialize, BorshDeserialize,
        )]
        pub struct $name([u8; 32]);

        impl $name {
            /// The raw 32 bytes of this id.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
        }

        impl From<[u8; 32]> for $name {
            fn from(value: [u8; 32]) -> Self {
                Self(value)
            }
        }

        impl AsRef<[u8]> for $name {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{}", hex::encode(self.0))
            }
        }
    };
}

content_address_id! {
    /// Stable identity of a person or agent — the **only** authorization
    /// subject in the system, and what an app sees as "who wrote this".
    ///
    /// This is the content address of the account's [`AccountGenesis`], not a
    /// public key. See the crate docs for why that distinction is load-bearing.
    AccountId
}

content_address_id! {
    /// Stable identity of one installation belonging to an account.
    ///
    /// This is the **CRDT replica id**: counter slots and HLC seeds key on it,
    /// and both require one writer per id. It is never an authorization input —
    /// authority always resolves through the [`AccountId`] the device is bound
    /// to.
    DeviceId
}

/// An X25519 public key used only as a scope-key delivery recipient.
///
/// Deliberately a distinct type from [`PublicKey`] (Ed25519). A device carries
/// **two** keys — one that signs, one that receives wrapped keys — rather than
/// reusing a single Ed25519 key for both signing and key agreement. Single-key
/// dual-use across a signature scheme and a Diffie-Hellman is a well-known
/// footgun with no compensating benefit, and the type split makes it impossible
/// to pass one where the other belongs.
///
/// This crate only carries the bytes; the wrapping itself lives with scope-key
/// delivery.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct KemPublicKey([u8; 32]);

impl KemPublicKey {
    /// The raw 32 bytes of this key.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for KemPublicKey {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl core::fmt::Display for KemPublicKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// The immutable root of an account. Hashing it yields the [`AccountId`], which
/// is why a verifier can recover the epoch-0 root key from the id alone.
///
/// `nonce` exists so two accounts created with the same root key are still
/// distinct identities, and so an account id cannot be predicted from a public
/// key alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct AccountGenesis {
    /// Always [`ACCOUNT_GENESIS_VERSION`] for accounts this build mints.
    pub version: u8,
    /// The account's epoch-0 root signing key.
    pub root_sign_pk: PublicKey,
    /// Random, chosen once at account creation.
    pub nonce: [u8; 16],
}

impl AccountGenesis {
    /// Build a genesis at the current [`ACCOUNT_GENESIS_VERSION`].
    #[must_use]
    pub const fn new(root_sign_pk: PublicKey, nonce: [u8; 16]) -> Self {
        Self {
            version: ACCOUNT_GENESIS_VERSION,
            root_sign_pk,
            nonce,
        }
    }

    /// The [`AccountId`] this genesis addresses.
    #[must_use]
    pub fn account_id(&self) -> AccountId {
        AccountId(domain_hash(ACCOUNT_ID_DOMAIN, &[&borsh_bytes(self)]))
    }
}

impl DeviceId {
    /// Mint a device id. Called once per installation, before the device has
    /// any certificate.
    ///
    /// Derived from the account and a fresh nonce rather than from the device's
    /// keys, so rotating a device's keypair keeps its replica identity — and
    /// therefore its counter slots and HLC lineage — intact.
    #[must_use]
    pub fn mint(account: AccountId, nonce: [u8; 16]) -> Self {
        Self(domain_hash(DEVICE_ID_DOMAIN, &[account.as_bytes(), &nonce]))
    }

    /// The 16-byte prefix used as this device's HLC instance seed.
    ///
    /// RGA character ids are minted from this seed, and two replicas sharing a
    /// seed mint colliding ids — which loses characters silently. So at most one
    /// of a colliding pair may be live in a scope, and the **lower** device id is
    /// the arbitrary-but-fixed winner. (Scope-local is sufficient: character ids
    /// only need to be unique within the scope that stores them.)
    ///
    /// That rule is applied when the device set is **read**, not when a link is
    /// admitted — see `ScopeState::live_devices`. Deciding it per link cannot
    /// work: "is there a lower colliding id" reads only what has folded so far,
    /// so the live set would depend on arrival order. Minting the id from a fresh
    /// nonce makes a collision vanishingly unlikely in any case; the rule is
    /// there so that a deliberate one is resolved identically everywhere rather
    /// than corrupting the CRDT planes.
    #[must_use]
    pub fn hlc_seed(&self) -> [u8; 16] {
        let mut seed = [0u8; 16];
        seed.copy_from_slice(&self.0[..16]);
        seed
    }
}

/// A group member's statement that an account is theirs.
///
/// Exists because an account root is deliberately **not** a member key. The root is
/// kept offline so it survives losing every device, which means the link gate
/// cannot ask "is this account's root a member?" — it is a member nowhere. Instead
/// the member key that *is* granted signs the account id, and the gate asks whether
/// the **endorser** is a member and whether it really signed this account.
///
/// Equally strong as the old question: only a member can produce a valid
/// endorsement, and only the root holder can certify devices into the account. It
/// takes both to enroll, and neither alone is enough.
///
/// Anyone may endorse an account they do not own — the account id is public — and it
/// gains them nothing, exactly as constructing a genesis over someone else's key
/// gains nothing: enrolling a device still needs the root's signature.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct AccountMemberEndorsement {
    /// The account being endorsed.
    pub account: AccountId,
    /// The member key making the statement. Must be a member at the op's cut.
    pub member: PublicKey,
    /// `member`'s signature over [`Self::signing_payload`].
    pub signature: [u8; 64],
}

impl AccountMemberEndorsement {
    /// Canonical bytes an endorser signs.
    ///
    /// Covers the account id and the endorsing key. Including the endorser is what
    /// stops a valid endorsement being re-presented as though a *different* member
    /// had made it — the signature would then verify against a key that never
    /// signed anything.
    #[must_use]
    pub fn signing_payload(account: AccountId, member: &PublicKey) -> [u8; 32] {
        domain_hash(
            ACCOUNT_ENDORSEMENT_SIGN_DOMAIN,
            &[account.as_bytes(), AsRef::<[u8; 32]>::as_ref(member)],
        )
    }
}

/// Sign an endorsement of `account` with a granted member key.
///
/// # Errors
/// [`AccountError::SigningFailed`] if the key cannot sign.
pub fn sign_account_endorsement(
    member_sk: &PrivateKey,
    account: AccountId,
) -> Result<AccountMemberEndorsement, AccountError> {
    let member = member_sk.public_key();
    let payload = AccountMemberEndorsement::signing_payload(account, &member);
    Ok(AccountMemberEndorsement {
        account,
        member,
        signature: member_sk
            .sign(&payload)
            .map_err(|_| AccountError::SigningFailed)?
            .to_bytes(),
    })
}

/// Check an endorsement is internally valid — that `member` really signed
/// `account`.
///
/// Says nothing about whether `member` is actually a member: that is an at-cut
/// question only the projection can answer, and keeping the two apart is the same
/// split as [`VerifiedDeviceCert`] versus "in force".
///
/// # Errors
/// [`AccountError::EndorsementSignatureInvalid`] if the signature does not verify.
pub fn verify_account_endorsement(
    endorsement: &AccountMemberEndorsement,
) -> Result<(), AccountError> {
    let payload =
        AccountMemberEndorsement::signing_payload(endorsement.account, &endorsement.member);
    endorsement
        .member
        .verify_raw_signature(&payload, &endorsement.signature)
        .map_err(|_| AccountError::EndorsementSignatureInvalid)
}

/// Rolls an account's root key from epoch `from_epoch` to `from_epoch + 1`.
///
/// Signed by the **outgoing** key, so the chain from the genesis forward is a
/// standard forward key-rollover: each key authorizes its own successor. A
/// verifier that trusts the genesis (and it can, because the genesis *is* the
/// account id) can therefore verify any later key without external input.
///
/// Note what this does not give you: an attacker holding a stolen root key can
/// sign a handoff of their own. Recovering from root-key compromise needs a
/// separate recovery authority and is deliberately out of scope here — but
/// because `AccountId` is not the key, adding one later is a key-set change
/// rather than a new identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct RootKeyHandoff {
    /// The account whose key is rolling. Bound into the signature so a handoff
    /// cannot be replayed onto a different account.
    pub account: AccountId,
    /// Epoch of the key that signs this handoff.
    pub from_epoch: u32,
    /// The incoming key, which becomes epoch `from_epoch + 1`.
    pub new_root_sign_pk: PublicKey,
    /// Signature by the epoch-`from_epoch` root key over
    /// [`RootKeyHandoff::signing_payload`].
    pub signature: [u8; 64],
}

impl RootKeyHandoff {
    /// Canonical bytes the outgoing root key signs. Covers every field except
    /// the signature itself.
    #[must_use]
    pub fn signing_payload(
        account: AccountId,
        from_epoch: u32,
        new_root_sign_pk: &PublicKey,
    ) -> [u8; 32] {
        domain_hash(
            HANDOFF_SIGN_DOMAIN,
            &[
                account.as_bytes(),
                &from_epoch.to_le_bytes(),
                new_root_sign_pk.as_ref(),
            ],
        )
    }

    /// The bytes this handoff's signature covers.
    #[must_use]
    pub fn payload(&self) -> [u8; 32] {
        Self::signing_payload(self.account, self.from_epoch, &self.new_root_sign_pk)
    }
}

/// A root-signed grant binding a device to an account.
///
/// There is deliberately **no expiry field**. Expiry requires participants to
/// agree on wall-clock time, which a causally-ordered system does not provide;
/// a certificate that expires "at" some timestamp would be valid on one node
/// and invalid on another, and authorization would stop converging. Withdrawal
/// is expressed as a revocation op instead, which is causally ordered like
/// every other decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeviceCert {
    /// The account this device speaks for.
    pub account: AccountId,
    /// The device being granted.
    pub device: DeviceId,
    /// Ed25519 key the device signs ops with.
    pub sign_pk: PublicKey,
    /// X25519 key wrapped scope keys are delivered to.
    pub kem_pk: KemPublicKey,
    /// Which account root-key epoch signed this certificate.
    pub key_epoch: u32,
    /// Monotone per device. A higher value supersedes lower ones and expresses
    /// **device key rotation** — the same device with fresh keys, keeping its
    /// replica identity — as distinct from enrolling a new device.
    pub device_epoch: u32,
    /// Signature by the epoch-`key_epoch` root key over
    /// [`DeviceCert::signing_payload`].
    pub signature: [u8; 64],
}

impl DeviceCert {
    /// Canonical bytes the root key signs. Covers every field except the
    /// signature itself.
    ///
    /// Both keys are covered, so neither the signing key nor the delivery key
    /// can be substituted in a certificate that otherwise verifies.
    #[must_use]
    pub fn signing_payload(
        account: AccountId,
        device: DeviceId,
        sign_pk: &PublicKey,
        kem_pk: &KemPublicKey,
        key_epoch: u32,
        device_epoch: u32,
    ) -> [u8; 32] {
        domain_hash(
            DEVICE_CERT_SIGN_DOMAIN,
            &[
                account.as_bytes(),
                device.as_bytes(),
                sign_pk.as_ref(),
                kem_pk.as_bytes(),
                &key_epoch.to_le_bytes(),
                &device_epoch.to_le_bytes(),
            ],
        )
    }

    /// The bytes this certificate's signature covers.
    #[must_use]
    pub fn payload(&self) -> [u8; 32] {
        Self::signing_payload(
            self.account,
            self.device,
            &self.sign_pk,
            &self.kem_pk,
            self.key_epoch,
            self.device_epoch,
        )
    }
}

/// Mint a root-key handoff, signed by the **outgoing** key.
///
/// Pairs with [`resolve_root_keys`], which is the only consumer of the format.
/// Exists so no caller ever assembles the signing preimage by hand: a
/// hand-rolled payload that omits a field still produces a signature that
/// *verifies*, while silently leaving that field unauthenticated — and the
/// omission is invisible at the call site. Routing every signer through the
/// same `signing_payload` the verifier uses makes that class of bug
/// unexpressible.
///
/// `from_epoch` must be the epoch of `current_root_sk`; the resulting handoff
/// establishes `from_epoch + 1`.
///
/// # Errors
/// [`AccountError::SigningFailed`] if the key refuses to sign.
pub fn sign_root_key_handoff(
    current_root_sk: &PrivateKey,
    account: AccountId,
    from_epoch: u32,
    new_root_sign_pk: &PublicKey,
) -> Result<RootKeyHandoff, AccountError> {
    let payload = RootKeyHandoff::signing_payload(account, from_epoch, new_root_sign_pk);
    Ok(RootKeyHandoff {
        account,
        from_epoch,
        new_root_sign_pk: *new_root_sign_pk,
        signature: current_root_sk
            .sign(&payload)
            .map_err(|_| AccountError::SigningFailed)?
            .to_bytes(),
    })
}

/// Mint a device certificate, signed by the account root key at `key_epoch`.
///
/// Same reasoning as [`sign_root_key_handoff`]: one place assembles the
/// preimage, so signer and verifier cannot drift.
///
/// `device_epoch` must strictly exceed any epoch already folded for this device
/// — the projection refuses a link that does not advance it, so a re-issued
/// certificate that reuses an epoch is inert rather than a rollback.
///
/// Takes one parameter per signed field rather than a builder: the argument
/// list *is* the list of fields the signature covers, and hiding it behind
/// optional setters would make an unsigned field easy to miss.
///
/// # Errors
/// [`AccountError::SigningFailed`] if the key refuses to sign.
pub fn sign_device_cert(
    root_sk: &PrivateKey,
    account: AccountId,
    device: DeviceId,
    sign_pk: &PublicKey,
    kem_pk: &KemPublicKey,
    key_epoch: u32,
    device_epoch: u32,
) -> Result<DeviceCert, AccountError> {
    let payload =
        DeviceCert::signing_payload(account, device, sign_pk, kem_pk, key_epoch, device_epoch);
    Ok(DeviceCert {
        account,
        device,
        sign_pk: *sign_pk,
        kem_pk: *kem_pk,
        key_epoch,
        device_epoch,
        signature: root_sk
            .sign(&payload)
            .map_err(|_| AccountError::SigningFailed)?
            .to_bytes(),
    })
}

/// A [`DeviceCert`] whose genesis anchor, key chain, and signature have all
/// been checked.
///
/// Holding one means the credential is **internally** valid. It does **not**
/// mean the binding is currently in force: whether the signing epoch has since
/// been superseded, whether the device was revoked, and whether the account is
/// even a member of the scope are all at-cut questions that only the projection
/// can answer. Those checks are the projection's, and this type exists so the
/// two stages can't be confused for one another.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedDeviceCert {
    /// Account the device is bound to.
    pub account: AccountId,
    /// The bound device.
    pub device: DeviceId,
    /// The device's op-signing key.
    pub sign_pk: PublicKey,
    /// The device's scope-key delivery key.
    pub kem_pk: KemPublicKey,
    /// Root-key epoch that signed the certificate.
    pub key_epoch: u32,
    /// The device's key-rotation epoch.
    pub device_epoch: u32,
}

/// Why a credential failed verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ThisError)]
pub enum AccountError {
    /// The genesis does not hash to the account id it claims.
    #[error("genesis hashes to {actual}, not the claimed account {claimed}")]
    GenesisMismatch {
        /// The account id the credential claims.
        claimed: AccountId,
        /// The account id the supplied genesis actually addresses.
        actual: AccountId,
    },
    /// The genesis carries a version this build does not understand.
    #[error("unsupported account genesis version {found} (this build supports {supported})")]
    UnsupportedVersion {
        /// Version read from the genesis.
        found: u8,
        /// Version this build mints and accepts.
        supported: u8,
    },
    /// A member endorsement's signature does not verify under the key it names.
    #[error("account endorsement is not validly signed by the member it names")]
    EndorsementSignatureInvalid,
    /// The supplied chain is longer than [`MAX_ROOT_KEY_HANDOFFS`].
    #[error("handoff chain has {found} entries, over the {limit} cap")]
    ChainTooLong {
        /// Length of the supplied chain.
        found: usize,
        /// The cap.
        limit: usize,
    },
    /// A handoff in the chain is not the immediate successor of the previous
    /// one. The chain must start at epoch 0 and step by exactly one.
    #[error("handoff chain not contiguous: expected from_epoch {expected}, found {found}")]
    ChainNotContiguous {
        /// The epoch the chain position requires.
        expected: u32,
        /// The epoch the handoff actually declares.
        found: u32,
    },
    /// A handoff names a different account than the one being verified.
    #[error("handoff at epoch {epoch} is for a different account")]
    HandoffAccountMismatch {
        /// Chain position of the offending handoff.
        epoch: u32,
    },
    /// A handoff is not validly signed by the outgoing root key.
    #[error("handoff at epoch {epoch} has an invalid signature")]
    HandoffSignatureInvalid {
        /// Chain position of the offending handoff.
        epoch: u32,
    },
    /// The certificate claims a root-key epoch the supplied chain does not
    /// reach.
    #[error("certificate claims key epoch {key_epoch} but the chain only reaches {reachable}")]
    EpochOutOfRange {
        /// Epoch the certificate claims.
        key_epoch: u32,
        /// Highest epoch the supplied chain establishes.
        reachable: u32,
    },
    /// The certificate names a different account than the genesis.
    #[error("certificate is for a different account than the supplied genesis")]
    CertAccountMismatch,
    /// The certificate is not validly signed by the root key at its claimed
    /// epoch.
    #[error("certificate has an invalid signature for its claimed key epoch")]
    CertSignatureInvalid,
    /// The signing key refused to produce a signature.
    #[error("signing failed")]
    SigningFailed,
    /// The revocation names a different account than the genesis.
    #[error("revocation is for a different account than the supplied genesis")]
    RevocationAccountMismatch,
    /// The revocation is not validly signed by the root key at its claimed
    /// epoch.
    #[error("revocation has an invalid signature for its claimed key epoch")]
    RevocationSignatureInvalid,
}

/// Walk a handoff chain from `genesis` and return the root key at each epoch.
///
/// Index `i` of the returned vector is the root key at epoch `i`; index 0 is
/// always the genesis key, so the result is never empty. The chain must start
/// at epoch 0 and step by exactly one — a gap would mean accepting a key whose
/// authorization was never demonstrated, and a repeat would make "the key at
/// epoch n" ambiguous.
///
/// # Errors
/// [`AccountError::UnsupportedVersion`] for an unknown genesis version, and the
/// `Chain*` / `Handoff*` variants for a chain that is discontinuous, addressed
/// to another account, or not properly signed.
pub fn resolve_root_keys(
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
) -> Result<Vec<PublicKey>, AccountError> {
    if genesis.version != ACCOUNT_GENESIS_VERSION {
        return Err(AccountError::UnsupportedVersion {
            found: genesis.version,
            supported: ACCOUNT_GENESIS_VERSION,
        });
    }

    // Cap before allocating or verifying anything. Each entry costs an Ed25519
    // verification, and this function is reachable from the wire — the governance
    // path bounds the field at decode, but `calimero-op` has no bounds layer at
    // all, so the check has to exist here too rather than relying on every caller
    // to have one.
    if chain.len() > MAX_ROOT_KEY_HANDOFFS {
        return Err(AccountError::ChainTooLong {
            found: chain.len(),
            limit: MAX_ROOT_KEY_HANDOFFS,
        });
    }

    let account = genesis.account_id();
    let mut keys = Vec::with_capacity(chain.len().saturating_add(1));
    keys.push(genesis.root_sign_pk);

    for (index, handoff) in chain.iter().enumerate() {
        // `index` is bounded by the chain length; a chain long enough to
        // overflow u32 cannot be held in memory.
        let expected = u32::try_from(index).unwrap_or(u32::MAX);
        if handoff.from_epoch != expected {
            return Err(AccountError::ChainNotContiguous {
                expected,
                found: handoff.from_epoch,
            });
        }
        if handoff.account != account {
            return Err(AccountError::HandoffAccountMismatch { epoch: expected });
        }
        // Signed by the OUTGOING key — the one already established at this
        // position — which is what makes the chain an authorization chain
        // rather than a list of assertions.
        let outgoing = keys[index];
        if outgoing
            .verify_raw_signature(&handoff.payload(), &handoff.signature)
            .is_err()
        {
            return Err(AccountError::HandoffSignatureInvalid { epoch: expected });
        }
        keys.push(handoff.new_root_sign_pk);
    }

    Ok(keys)
}

/// Verify a device certificate end to end against a self-certifying genesis.
///
/// Checks, in order: the genesis addresses `claimed_account`; the certificate
/// is for that same account; the handoff chain is valid and reaches the
/// certificate's epoch; and the certificate is signed by the root key at that
/// epoch.
///
/// The `claimed_account` parameter is what ties the whole credential to
/// something the caller already trusts — typically `op.author`. Without it a
/// caller could verify a perfectly well-formed credential for an account nobody
/// asked about.
///
/// # Errors
/// See [`AccountError`]; every variant is reachable from this function.
pub fn verify_device_cert(
    claimed_account: AccountId,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    cert: &DeviceCert,
) -> Result<VerifiedDeviceCert, AccountError> {
    let derived = genesis.account_id();
    if derived != claimed_account {
        return Err(AccountError::GenesisMismatch {
            claimed: claimed_account,
            actual: derived,
        });
    }
    if cert.account != derived {
        return Err(AccountError::CertAccountMismatch);
    }

    let keys = resolve_root_keys(genesis, chain)?;

    let epoch_index = usize::try_from(cert.key_epoch).unwrap_or(usize::MAX);
    let Some(signer) = keys.get(epoch_index) else {
        // `keys` is non-empty, so `len() - 1` cannot underflow.
        let reachable = u32::try_from(keys.len() - 1).unwrap_or(u32::MAX);
        return Err(AccountError::EpochOutOfRange {
            key_epoch: cert.key_epoch,
            reachable,
        });
    };

    if signer
        .verify_raw_signature(&cert.payload(), &cert.signature)
        .is_err()
    {
        return Err(AccountError::CertSignatureInvalid);
    }

    Ok(VerifiedDeviceCert {
        account: cert.account,
        device: cert.device,
        sign_pk: cert.sign_pk,
        kem_pk: cert.kem_pk,
        key_epoch: cert.key_epoch,
        device_epoch: cert.device_epoch,
    })
}

/// A root-signed withdrawal of a device.
///
/// The counterpart of [`DeviceCert`], and self-certifying for the same reason:
/// whether the account owner may revoke their own device cannot be answered from
/// folded state. "Is the signer this account's current root key" depends on which
/// rotations a given replica has folded, so two replicas would reach different
/// verdicts on one op and disagree permanently about who may author. Carrying the
/// proof makes the answer a property of the op rather than of the receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeviceRevocation {
    /// The account withdrawing the device. Bound into the signature so a
    /// revocation cannot be replayed against another account.
    pub account: AccountId,
    /// The device being withdrawn.
    pub device: DeviceId,
    /// Which account root-key epoch signed this.
    pub key_epoch: u32,
    /// Signature by the epoch-`key_epoch` root key over
    /// [`DeviceRevocation::signing_payload`].
    pub signature: [u8; 64],
}

impl DeviceRevocation {
    /// Canonical bytes the root key signs. Covers every field but the signature.
    #[must_use]
    pub fn signing_payload(account: AccountId, device: DeviceId, key_epoch: u32) -> [u8; 32] {
        domain_hash(
            DEVICE_REVOCATION_SIGN_DOMAIN,
            &[
                account.as_bytes(),
                device.as_bytes(),
                &key_epoch.to_le_bytes(),
            ],
        )
    }

    /// The bytes this revocation's signature covers.
    #[must_use]
    pub fn payload(&self) -> [u8; 32] {
        Self::signing_payload(self.account, self.device, self.key_epoch)
    }
}

/// Mint a revocation for `device`, signed by the account root at `key_epoch`.
///
/// # Errors
/// [`AccountError::SigningFailed`] if the key refuses to sign.
pub fn sign_device_revocation(
    root_sk: &PrivateKey,
    account: AccountId,
    device: DeviceId,
    key_epoch: u32,
) -> Result<DeviceRevocation, AccountError> {
    let payload = DeviceRevocation::signing_payload(account, device, key_epoch);
    Ok(DeviceRevocation {
        account,
        device,
        key_epoch,
        signature: root_sk
            .sign(&payload)
            .map_err(|_| AccountError::SigningFailed)?
            .to_bytes(),
    })
}

/// A [`DeviceRevocation`] together with everything needed to verify it.
///
/// Bundled rather than passed as three fields on the op for the same reason a
/// link carries its genesis and chain: the proof has to stand on its own, so a
/// receiver can check it without having folded any prior op about the account.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SignedDeviceRevocation {
    /// The account's genesis, which hashes to the account being withdrawn from.
    pub genesis: AccountGenesis,
    /// Root-key handoffs up to `revocation.key_epoch`.
    pub chain: Vec<RootKeyHandoff>,
    /// The revocation itself.
    pub revocation: DeviceRevocation,
}

impl SignedDeviceRevocation {
    /// Whether this proof authorises withdrawing `device` from `account`.
    ///
    /// Checks the account and device the caller expects against the ones the
    /// proof names, so a valid proof for one device cannot authorise another.
    ///
    /// # Errors
    /// Propagates [`verify_device_revocation`]; also
    /// [`AccountError::RevocationAccountMismatch`] when the proof is for a
    /// different device than the op names.
    pub fn authorises(&self, account: AccountId, device: DeviceId) -> Result<(), AccountError> {
        if self.revocation.device != device {
            return Err(AccountError::RevocationAccountMismatch);
        }
        verify_device_revocation(account, &self.genesis, &self.chain, &self.revocation)
    }
}

/// Verify a revocation against the account it names, from the account id alone.
///
/// **Any epoch the carried chain resolves is accepted, not merely the newest.**
/// That is a deliberate asymmetry with [`verify_device_cert`], whose superseded
/// epochs are filtered when the view is read. Applying the same filter here would
/// mean rotating an account's root key silently *un-revokes* every device it had
/// withdrawn — and a revocation is terminal by design, precisely so a spent
/// `DeviceId` can never come back.
///
/// The cost is that a compromised *old* root key can still revoke devices. That
/// is accepted rather than overlooked: whoever holds any root key of an account
/// can already sign a handoff and take it over, so root-key compromise is
/// unrecoverable regardless, and the terminal-revocation guarantee is worth more
/// than narrowing a capability an attacker in that position already has.
///
/// # Errors
/// - [`AccountError::GenesisMismatch`] if the genesis does not hash to
///   `claimed_account`.
/// - [`AccountError::RevocationAccountMismatch`] if the revocation names a
///   different account.
/// - [`AccountError::EpochOutOfRange`] if `key_epoch` exceeds the chain.
/// - [`AccountError::RevocationSignatureInvalid`] if the signature does not
///   verify under the key at that epoch.
pub fn verify_device_revocation(
    claimed_account: AccountId,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    revocation: &DeviceRevocation,
) -> Result<(), AccountError> {
    let derived = genesis.account_id();
    if derived != claimed_account {
        return Err(AccountError::GenesisMismatch {
            claimed: claimed_account,
            actual: derived,
        });
    }
    if revocation.account != derived {
        return Err(AccountError::RevocationAccountMismatch);
    }

    let keys = resolve_root_keys(genesis, chain)?;
    let epoch_index = usize::try_from(revocation.key_epoch).unwrap_or(usize::MAX);
    let Some(signer) = keys.get(epoch_index) else {
        // `keys` is non-empty, so `len() - 1` cannot underflow.
        let reachable = u32::try_from(keys.len() - 1).unwrap_or(u32::MAX);
        return Err(AccountError::EpochOutOfRange {
            key_epoch: revocation.key_epoch,
            reachable,
        });
    };

    if signer
        .verify_raw_signature(&revocation.payload(), &revocation.signature)
        .is_err()
    {
        return Err(AccountError::RevocationSignatureInvalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::identity::PrivateKey;
    use std::collections::HashSet;

    /// An account rooted at `root`, plus a handoff rolling it onto `next`.
    fn rotated(root: &PrivateKey, next: &PrivateKey) -> (AccountGenesis, RootKeyHandoff) {
        let genesis = AccountGenesis::new(root.public_key(), [0x11; 16]);
        let account = genesis.account_id();
        let payload = RootKeyHandoff::signing_payload(account, 0, &next.public_key());
        let handoff = RootKeyHandoff {
            account,
            from_epoch: 0,
            new_root_sign_pk: next.public_key(),
            signature: root.sign(&payload).expect("sign").to_bytes(),
        };
        (genesis, handoff)
    }

    #[test]
    fn a_root_signed_revocation_verifies_from_the_account_id_alone() {
        let root = PrivateKey::from([7u8; 32]);
        let genesis = AccountGenesis::new(root.public_key(), [0x11; 16]);
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [0x22; 16]);

        let revocation = sign_device_revocation(&root, account, device, 0).expect("sign");
        assert!(verify_device_revocation(account, &genesis, &[], &revocation).is_ok());
    }

    #[test]
    fn a_revocation_survives_a_later_root_key_rotation() {
        // The asymmetry with `verify_device_cert`, and the reason it exists.
        // Superseded epochs are filtered for certificates when the view is read;
        // applying that rule here would mean rotating the root silently
        // UN-revokes every device the account had withdrawn. Revocation is
        // terminal by design — a spent DeviceId must never come back.
        let root = PrivateKey::from([7u8; 32]);
        let next = PrivateKey::from([8u8; 32]);
        let (genesis, handoff) = rotated(&root, &next);
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [0x22; 16]);

        // Signed by the OLD root, before the rotation.
        let revocation = sign_device_revocation(&root, account, device, 0).expect("sign");

        assert!(
            verify_device_revocation(account, &genesis, &[handoff], &revocation).is_ok(),
            "a rotation must not resurrect a revoked device"
        );
    }

    #[test]
    fn the_new_root_may_also_revoke() {
        let root = PrivateKey::from([7u8; 32]);
        let next = PrivateKey::from([8u8; 32]);
        let (genesis, handoff) = rotated(&root, &next);
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [0x22; 16]);

        let revocation = sign_device_revocation(&next, account, device, 1).expect("sign");
        assert!(verify_device_revocation(account, &genesis, &[handoff], &revocation).is_ok());
    }

    #[test]
    fn a_revocation_signed_by_a_stranger_is_refused() {
        // The whole point of the proof: without it, "may this signer revoke" would
        // have to be answered from folded state, and two replicas would disagree.
        let root = PrivateKey::from([7u8; 32]);
        let stranger = PrivateKey::from([9u8; 32]);
        let genesis = AccountGenesis::new(root.public_key(), [0x11; 16]);
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [0x22; 16]);

        let forged = sign_device_revocation(&stranger, account, device, 0).expect("sign");
        assert!(matches!(
            verify_device_revocation(account, &genesis, &[], &forged),
            Err(AccountError::RevocationSignatureInvalid)
        ));
    }

    #[test]
    fn a_revocation_cannot_be_replayed_onto_another_device_or_account() {
        let root = PrivateKey::from([7u8; 32]);
        let genesis = AccountGenesis::new(root.public_key(), [0x11; 16]);
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [0x22; 16]);
        let other_device = DeviceId::mint(account, [0x23; 16]);

        let mut revocation = sign_device_revocation(&root, account, device, 0).expect("sign");
        revocation.device = other_device;
        assert!(
            matches!(
                verify_device_revocation(account, &genesis, &[], &revocation),
                Err(AccountError::RevocationSignatureInvalid)
            ),
            "the device is inside the signed payload"
        );

        let elsewhere = AccountGenesis::new(root.public_key(), [0x99; 16]);
        let honest = sign_device_revocation(&root, account, device, 0).expect("sign");
        assert!(
            matches!(
                verify_device_revocation(elsewhere.account_id(), &elsewhere, &[], &honest),
                Err(AccountError::RevocationAccountMismatch)
            ),
            "a revocation is bound to the account that minted it"
        );
    }

    #[test]
    fn a_revocation_claiming_an_unreachable_epoch_is_refused() {
        let root = PrivateKey::from([7u8; 32]);
        let genesis = AccountGenesis::new(root.public_key(), [0x11; 16]);
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [0x22; 16]);

        let revocation = sign_device_revocation(&root, account, device, 5).expect("sign");
        assert!(matches!(
            verify_device_revocation(account, &genesis, &[], &revocation),
            Err(AccountError::EpochOutOfRange { .. })
        ));
    }

    /// Deterministic keypair, so failures reproduce exactly.
    fn key(seed: u8) -> PrivateKey {
        PrivateKey::from([seed; 32])
    }

    fn genesis_for(root: &PrivateKey) -> AccountGenesis {
        AccountGenesis::new(root.public_key(), [7u8; 16])
    }

    fn sign_handoff(
        signer: &PrivateKey,
        account: AccountId,
        from_epoch: u32,
        new_root: &PrivateKey,
    ) -> RootKeyHandoff {
        let new_root_sign_pk = new_root.public_key();
        let payload = RootKeyHandoff::signing_payload(account, from_epoch, &new_root_sign_pk);
        RootKeyHandoff {
            account,
            from_epoch,
            new_root_sign_pk,
            signature: signer.sign(&payload).expect("sign").to_bytes(),
        }
    }

    fn sign_cert(
        signer: &PrivateKey,
        account: AccountId,
        device: DeviceId,
        device_sign: &PrivateKey,
        key_epoch: u32,
        device_epoch: u32,
    ) -> DeviceCert {
        let sign_pk = device_sign.public_key();
        let kem_pk = KemPublicKey::from([9u8; 32]);
        let payload = DeviceCert::signing_payload(
            account,
            device,
            &sign_pk,
            &kem_pk,
            key_epoch,
            device_epoch,
        );
        DeviceCert {
            account,
            device,
            sign_pk,
            kem_pk,
            key_epoch,
            device_epoch,
            signature: signer.sign(&payload).expect("sign").to_bytes(),
        }
    }

    // ---- ids ----

    #[test]
    fn account_id_is_the_content_address_of_its_genesis() {
        let root = key(1);
        let g = genesis_for(&root);
        assert_eq!(
            g.account_id(),
            g.account_id(),
            "derivation is deterministic"
        );

        // The whole point of the anchor: the id commits to the epoch-0 key.
        let mut other = g;
        other.root_sign_pk = key(2).public_key();
        assert_ne!(g.account_id(), other.account_id());
    }

    #[test]
    fn same_root_key_with_different_nonce_is_a_different_account() {
        let root = key(1);
        let a = AccountGenesis::new(root.public_key(), [1u8; 16]);
        let b = AccountGenesis::new(root.public_key(), [2u8; 16]);
        assert_ne!(a.account_id(), b.account_id());
    }

    #[test]
    fn genesis_version_is_part_of_the_account_id() {
        let root = key(1);
        let mut v1 = genesis_for(&root);
        let id_v1 = v1.account_id();
        v1.version = ACCOUNT_GENESIS_VERSION + 1;
        assert_ne!(id_v1, v1.account_id());
    }

    #[test]
    fn device_id_is_stable_across_device_key_rotation() {
        // The reason DeviceId is minted from a nonce rather than from the
        // device's keys: rotating keys must not orphan the replica's CRDT state.
        let account = genesis_for(&key(1)).account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        assert_eq!(device, DeviceId::mint(account, [3u8; 16]));
    }

    #[test]
    fn device_ids_differ_by_account_and_by_nonce() {
        let a = genesis_for(&key(1)).account_id();
        let b = genesis_for(&key(2)).account_id();
        assert_ne!(DeviceId::mint(a, [1u8; 16]), DeviceId::mint(a, [2u8; 16]));
        assert_ne!(DeviceId::mint(a, [1u8; 16]), DeviceId::mint(b, [1u8; 16]));
    }

    #[test]
    fn an_overlong_handoff_chain_is_refused_before_any_verification() {
        // Each entry costs an Ed25519 verification, and this is reachable from
        // untrusted bytes, so the cap has to be checked before the walk rather
        // than relying on every caller to bound the field first.
        let g = genesis_for(&key(1));
        let bogus =
            sign_root_key_handoff(&key(1), g.account_id(), 0, &key(2).public_key()).expect("sign");
        let chain = vec![bogus; MAX_ROOT_KEY_HANDOFFS + 1];
        assert_eq!(
            resolve_root_keys(&g, &chain),
            Err(AccountError::ChainTooLong {
                found: MAX_ROOT_KEY_HANDOFFS + 1,
                limit: MAX_ROOT_KEY_HANDOFFS,
            }),
            "an overlong chain must be refused by length, not by the first bad link"
        );

        // A chain exactly at the cap is still refused on its merits (this one is
        // not contiguous), not by the length gate.
        let at_cap = vec![bogus; MAX_ROOT_KEY_HANDOFFS];
        assert!(!matches!(
            resolve_root_keys(&g, &at_cap),
            Err(AccountError::ChainTooLong { .. })
        ));
    }

    #[test]
    fn an_endorsement_round_trips_and_rejects_forgery() {
        let member = key(1);
        let account = genesis_for(&key(9)).account_id();

        let endorsement = sign_account_endorsement(&member, account).expect("sign");
        assert_eq!(verify_account_endorsement(&endorsement), Ok(()));

        // A flipped signature byte fails.
        let mut tampered = endorsement;
        tampered.signature[0] ^= 0xFF;
        assert_eq!(
            verify_account_endorsement(&tampered),
            Err(AccountError::EndorsementSignatureInvalid)
        );

        // Naming a different account fails: the account is inside the payload.
        let mut moved = endorsement;
        moved.account = genesis_for(&key(8)).account_id();
        assert_eq!(
            verify_account_endorsement(&moved),
            Err(AccountError::EndorsementSignatureInvalid)
        );
    }

    #[test]
    fn an_endorsement_cannot_be_re_presented_as_another_members() {
        // Why the endorser is inside the signed payload. Without it, swapping the
        // `member` field would leave a signature that verifies against a key which
        // never signed anything — a member could be shown to have endorsed an
        // account it never touched.
        let real = key(1);
        let other = key(2);
        let account = genesis_for(&key(9)).account_id();

        let mut stolen = sign_account_endorsement(&real, account).expect("sign");
        stolen.member = other.public_key();
        assert_eq!(
            verify_account_endorsement(&stolen),
            Err(AccountError::EndorsementSignatureInvalid)
        );
    }

    #[test]
    fn endorsing_someone_elses_account_is_harmless() {
        // Account ids are public, so anyone can endorse one. It grants nothing:
        // enrolling a device still needs the ROOT's signature, which an endorser
        // does not hold. Pinned so the gate is never "tightened" into rejecting a
        // valid endorsement on the mistaken grounds that endorsement implies
        // ownership.
        let stranger = key(7);
        let someone_elses = genesis_for(&key(9)).account_id();

        let endorsement = sign_account_endorsement(&stranger, someone_elses).expect("sign");
        assert_eq!(
            verify_account_endorsement(&endorsement),
            Ok(()),
            "an endorsement is internally valid regardless of who made it; whether \
             the endorser is a member is a separate at-cut question"
        );
    }

    #[test]
    fn a_derived_nonce_is_stable_per_namespace_and_unlinkable_across_them() {
        // The two properties that make one offline root workable at all.
        let root = [0x11u8; 32];
        let ns_a = [0xAAu8; 32];
        let ns_b = [0xBBu8; 32];

        // Stable: recovery recomputes the same account id from the root alone.
        assert_eq!(
            derive_account_nonce(&root, &ns_a),
            derive_account_nonce(&root, &ns_a),
            "derivation must be deterministic or a recovered node names a different account"
        );

        // Unlinkable: the same person in two namespaces is two account ids.
        assert_ne!(
            derive_account_nonce(&root, &ns_a),
            derive_account_nonce(&root, &ns_b),
            "one nonce across namespaces would make every account id equal and link them"
        );

        // And two people in one namespace are distinct.
        assert_ne!(
            derive_account_nonce(&root, &ns_a),
            derive_account_nonce(&[0x22u8; 32], &ns_a)
        );
    }

    #[test]
    fn the_nonce_cannot_be_derived_from_the_public_root_key() {
        // Structural, not a behavioural assertion: `derive_account_nonce` takes the
        // SECRET. The public root travels in every genesis, so a derivation from it
        // would let any observer compute this root's account id in namespaces it has
        // never seen — recreating the correlation the per-namespace nonce prevents.
        //
        // Pinned by deriving from a secret and its own public bytes and requiring
        // they differ, so a refactor that swaps one for the other is caught.
        let root_sk = key(1);
        let secret_derived = derive_account_nonce(root_sk.as_bytes(), &[0xAAu8; 32]);
        let public_derived = derive_account_nonce(
            AsRef::<[u8; 32]>::as_ref(&root_sk.public_key()),
            &[0xAAu8; 32],
        );
        assert_ne!(
            secret_derived, public_derived,
            "deriving from the public key must not produce the same nonce as the secret"
        );
    }

    #[test]
    fn hlc_seed_is_the_device_id_prefix() {
        let account = genesis_for(&key(1)).account_id();
        let device = DeviceId::mint(account, [4u8; 16]);
        assert_eq!(&device.hlc_seed()[..], &device.as_bytes()[..16]);
    }

    #[test]
    fn distinct_devices_get_distinct_hlc_seeds() {
        // Not a proof of uniqueness — that is enforced at link time by the
        // projection. This only guards against a derivation that collapses.
        let account = genesis_for(&key(1)).account_id();
        let seeds: HashSet<[u8; 16]> = (0..64u8)
            .map(|n| DeviceId::mint(account, [n; 16]).hlc_seed())
            .collect();
        assert_eq!(seeds.len(), 64);
    }

    #[test]
    fn signing_domains_are_pairwise_distinct() {
        let unique: HashSet<&[u8]> = ALL_DOMAINS.iter().copied().collect();
        assert_eq!(
            unique.len(),
            ALL_DOMAINS.len(),
            "a shared domain would let a signature be replayed across purposes"
        );
    }

    #[test]
    fn domain_hash_is_not_confusable_by_shifting_bytes() {
        // Length-prefixing is what stops ("ab", "c") and ("a", "bc") colliding.
        assert_ne!(domain_hash(b"ab", &[b"c"]), domain_hash(b"a", &[b"bc"]),);
        assert_ne!(
            domain_hash(b"d", &[b"ab", b"c"]),
            domain_hash(b"d", &[b"a", b"bc"]),
        );
    }

    // ---- key chain ----

    #[test]
    fn empty_chain_resolves_to_the_genesis_key() {
        let root = key(1);
        let g = genesis_for(&root);
        let keys = resolve_root_keys(&g, &[]).expect("valid");
        assert_eq!(keys, vec![root.public_key()]);
    }

    #[test]
    fn chain_resolves_each_epoch_in_order() {
        let (r0, r1, r2) = (key(1), key(2), key(3));
        let g = genesis_for(&r0);
        let account = g.account_id();
        let chain = [
            sign_handoff(&r0, account, 0, &r1),
            sign_handoff(&r1, account, 1, &r2),
        ];
        let keys = resolve_root_keys(&g, &chain).expect("valid");
        assert_eq!(
            keys,
            vec![r0.public_key(), r1.public_key(), r2.public_key()]
        );
    }

    #[test]
    fn handoff_must_be_signed_by_the_outgoing_key() {
        let (r0, r1, imposter) = (key(1), key(2), key(9));
        let g = genesis_for(&r0);
        let account = g.account_id();
        // Signed by a key that was never the account's root.
        let chain = [sign_handoff(&imposter, account, 0, &r1)];
        assert_eq!(
            resolve_root_keys(&g, &chain),
            Err(AccountError::HandoffSignatureInvalid { epoch: 0 })
        );
    }

    #[test]
    fn a_superseded_key_cannot_re_sign_a_later_handoff() {
        // Epoch 0 authorizes epoch 1; epoch 0 must not then authorize epoch 2.
        let (r0, r1, r2) = (key(1), key(2), key(3));
        let g = genesis_for(&r0);
        let account = g.account_id();
        let chain = [
            sign_handoff(&r0, account, 0, &r1),
            sign_handoff(&r0, account, 1, &r2), // wrong signer for this position
        ];
        assert_eq!(
            resolve_root_keys(&g, &chain),
            Err(AccountError::HandoffSignatureInvalid { epoch: 1 })
        );
    }

    #[test]
    fn chain_must_start_at_epoch_zero() {
        let (r0, r1) = (key(1), key(2));
        let g = genesis_for(&r0);
        let account = g.account_id();
        let chain = [sign_handoff(&r0, account, 1, &r1)];
        assert_eq!(
            resolve_root_keys(&g, &chain),
            Err(AccountError::ChainNotContiguous {
                expected: 0,
                found: 1
            })
        );
    }

    #[test]
    fn chain_must_not_skip_an_epoch() {
        let (r0, r1, r2) = (key(1), key(2), key(3));
        let g = genesis_for(&r0);
        let account = g.account_id();
        let chain = [
            sign_handoff(&r0, account, 0, &r1),
            sign_handoff(&r1, account, 2, &r2),
        ];
        assert_eq!(
            resolve_root_keys(&g, &chain),
            Err(AccountError::ChainNotContiguous {
                expected: 1,
                found: 2
            })
        );
    }

    #[test]
    fn handoff_cannot_be_replayed_onto_another_account() {
        let (r0, r1) = (key(1), key(2));
        let g = genesis_for(&r0);
        let other = AccountGenesis::new(r0.public_key(), [99u8; 16]);
        // Validly signed by r0, but minted for a different account id.
        let stolen = sign_handoff(&r0, other.account_id(), 0, &r1);
        assert_eq!(
            resolve_root_keys(&g, &[stolen]),
            Err(AccountError::HandoffAccountMismatch { epoch: 0 })
        );
    }

    #[test]
    fn unknown_genesis_version_is_rejected() {
        let mut g = genesis_for(&key(1));
        g.version = 200;
        assert_eq!(
            resolve_root_keys(&g, &[]),
            Err(AccountError::UnsupportedVersion {
                found: 200,
                supported: ACCOUNT_GENESIS_VERSION
            })
        );
    }

    // ---- certificates ----

    #[test]
    fn valid_cert_verifies_and_reports_its_fields() {
        let (root, dev) = (key(1), key(5));
        let g = genesis_for(&root);
        let account = g.account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        let cert = sign_cert(&root, account, device, &dev, 0, 0);

        let verified = verify_device_cert(account, &g, &[], &cert).expect("valid");
        assert_eq!(verified.account, account);
        assert_eq!(verified.device, device);
        assert_eq!(verified.sign_pk, dev.public_key());
        assert_eq!(verified.key_epoch, 0);
        assert_eq!(verified.device_epoch, 0);
    }

    #[test]
    fn cert_signed_by_a_rotated_key_verifies_against_the_chain() {
        let (r0, r1, dev) = (key(1), key(2), key(5));
        let g = genesis_for(&r0);
        let account = g.account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        let chain = [sign_handoff(&r0, account, 0, &r1)];
        let cert = sign_cert(&r1, account, device, &dev, 1, 0);
        assert!(verify_device_cert(account, &g, &chain, &cert).is_ok());
    }

    #[test]
    fn genesis_must_address_the_claimed_account() {
        // The anchor check: a well-formed credential for account X must not
        // verify when the caller asked about account Y.
        let (root, dev) = (key(1), key(5));
        let g = genesis_for(&root);
        let account = g.account_id();
        let other = AccountGenesis::new(root.public_key(), [42u8; 16]).account_id();
        let cert = sign_cert(
            &root,
            account,
            DeviceId::mint(account, [3u8; 16]),
            &dev,
            0,
            0,
        );

        assert_eq!(
            verify_device_cert(other, &g, &[], &cert),
            Err(AccountError::GenesisMismatch {
                claimed: other,
                actual: account
            })
        );
    }

    #[test]
    fn cert_for_a_different_account_than_the_genesis_is_rejected() {
        let (root, dev) = (key(1), key(5));
        let g = genesis_for(&root);
        let account = g.account_id();
        let foreign = AccountGenesis::new(key(8).public_key(), [1u8; 16]).account_id();
        let cert = sign_cert(
            &root,
            foreign,
            DeviceId::mint(foreign, [3u8; 16]),
            &dev,
            0,
            0,
        );
        assert_eq!(
            verify_device_cert(account, &g, &[], &cert),
            Err(AccountError::CertAccountMismatch)
        );
    }

    #[test]
    fn cert_claiming_an_epoch_beyond_the_chain_is_rejected() {
        let (root, dev) = (key(1), key(5));
        let g = genesis_for(&root);
        let account = g.account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        let cert = sign_cert(&root, account, device, &dev, 3, 0);
        assert_eq!(
            verify_device_cert(account, &g, &[], &cert),
            Err(AccountError::EpochOutOfRange {
                key_epoch: 3,
                reachable: 0
            })
        );
    }

    #[test]
    fn cert_signed_by_the_wrong_epoch_key_is_rejected() {
        // r0 signs but claims epoch 1, whose key is r1.
        let (r0, r1, dev) = (key(1), key(2), key(5));
        let g = genesis_for(&r0);
        let account = g.account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        let chain = [sign_handoff(&r0, account, 0, &r1)];
        let cert = sign_cert(&r0, account, device, &dev, 1, 0);
        assert_eq!(
            verify_device_cert(account, &g, &chain, &cert),
            Err(AccountError::CertSignatureInvalid)
        );
    }

    #[test]
    fn substituting_the_device_signing_key_invalidates_the_cert() {
        let (root, dev, attacker) = (key(1), key(5), key(6));
        let g = genesis_for(&root);
        let account = g.account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        let mut cert = sign_cert(&root, account, device, &dev, 0, 0);
        cert.sign_pk = attacker.public_key();
        assert_eq!(
            verify_device_cert(account, &g, &[], &cert),
            Err(AccountError::CertSignatureInvalid)
        );
    }

    #[test]
    fn substituting_the_kem_key_invalidates_the_cert() {
        // Otherwise an attacker could redirect wrapped scope keys to themselves
        // while leaving a valid-looking signing binding in place.
        let (root, dev) = (key(1), key(5));
        let g = genesis_for(&root);
        let account = g.account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        let mut cert = sign_cert(&root, account, device, &dev, 0, 0);
        cert.kem_pk = KemPublicKey::from([0xAAu8; 32]);
        assert_eq!(
            verify_device_cert(account, &g, &[], &cert),
            Err(AccountError::CertSignatureInvalid)
        );
    }

    #[test]
    fn substituting_the_device_id_invalidates_the_cert() {
        let (root, dev) = (key(1), key(5));
        let g = genesis_for(&root);
        let account = g.account_id();
        let mut cert = sign_cert(
            &root,
            account,
            DeviceId::mint(account, [3u8; 16]),
            &dev,
            0,
            0,
        );
        cert.device = DeviceId::mint(account, [4u8; 16]);
        assert_eq!(
            verify_device_cert(account, &g, &[], &cert),
            Err(AccountError::CertSignatureInvalid)
        );
    }

    #[test]
    fn bumping_the_device_epoch_invalidates_the_cert() {
        // device_epoch drives supersession at the projection, so it must be
        // signed — otherwise anyone could replay an old cert at a higher epoch.
        let (root, dev) = (key(1), key(5));
        let g = genesis_for(&root);
        let account = g.account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        let mut cert = sign_cert(&root, account, device, &dev, 0, 0);
        cert.device_epoch = 7;
        assert_eq!(
            verify_device_cert(account, &g, &[], &cert),
            Err(AccountError::CertSignatureInvalid)
        );
    }

    // ---- minting ----

    #[test]
    fn a_minted_cert_verifies() {
        // The round trip that matters: whatever the signer produces, the
        // verifier must accept. If the two ever assemble the preimage
        // differently, this is what catches it.
        let (root, dev) = (key(1), key(5));
        let g = genesis_for(&root);
        let account = g.account_id();
        let device = DeviceId::mint(account, [3u8; 16]);

        let cert = sign_device_cert(
            &root,
            account,
            device,
            &dev.public_key(),
            &KemPublicKey::from([9u8; 32]),
            0,
            0,
        )
        .expect("sign");

        let verified = verify_device_cert(account, &g, &[], &cert).expect("verify");
        assert_eq!(verified.device, device);
        assert_eq!(verified.sign_pk, dev.public_key());
    }

    #[test]
    fn a_minted_handoff_chain_verifies() {
        let (r0, r1, r2) = (key(1), key(2), key(3));
        let g = genesis_for(&r0);
        let account = g.account_id();

        let chain = [
            sign_root_key_handoff(&r0, account, 0, &r1.public_key()).expect("sign"),
            sign_root_key_handoff(&r1, account, 1, &r2.public_key()).expect("sign"),
        ];

        assert_eq!(
            resolve_root_keys(&g, &chain).expect("resolve"),
            vec![r0.public_key(), r1.public_key(), r2.public_key()]
        );
    }

    #[test]
    fn a_cert_minted_under_a_rotated_key_verifies_against_the_chain() {
        // End to end through both minters: rotate the root, then certify a
        // device with the new key.
        let (r0, r1, dev) = (key(1), key(2), key(5));
        let g = genesis_for(&r0);
        let account = g.account_id();
        let chain = [sign_root_key_handoff(&r0, account, 0, &r1.public_key()).expect("sign")];

        let cert = sign_device_cert(
            &r1,
            account,
            DeviceId::mint(account, [3u8; 16]),
            &dev.public_key(),
            &KemPublicKey::from([9u8; 32]),
            1,
            0,
        )
        .expect("sign");

        assert!(verify_device_cert(account, &g, &chain, &cert).is_ok());
    }

    #[test]
    fn minting_with_the_wrong_key_for_the_claimed_epoch_fails_verification() {
        // The minter does not check that the signer matches `key_epoch` — it
        // signs what it is told. The verifier is what enforces the pairing, so
        // a caller that passes the stale key gets a cert that simply does not
        // verify, rather than one that quietly works.
        let (r0, r1, dev) = (key(1), key(2), key(5));
        let g = genesis_for(&r0);
        let account = g.account_id();
        let chain = [sign_root_key_handoff(&r0, account, 0, &r1.public_key()).expect("sign")];

        let cert = sign_device_cert(
            &r0, // superseded key...
            account,
            DeviceId::mint(account, [3u8; 16]),
            &dev.public_key(),
            &KemPublicKey::from([9u8; 32]),
            1, // ...claiming the new epoch
            0,
        )
        .expect("sign");

        assert_eq!(
            verify_device_cert(account, &g, &chain, &cert),
            Err(AccountError::CertSignatureInvalid)
        );
    }

    #[test]
    fn minted_credentials_are_deterministic() {
        // Ed25519 signatures here are deterministic, so the same inputs give
        // byte-identical output. Worth pinning: a nondeterministic credential
        // would change an op's content address on every re-issue.
        let root = key(1);
        let g = genesis_for(&root);
        let account = g.account_id();
        let mint = || {
            sign_device_cert(
                &root,
                account,
                DeviceId::mint(account, [3u8; 16]),
                &key(5).public_key(),
                &KemPublicKey::from([9u8; 32]),
                0,
                0,
            )
            .expect("sign")
        };
        assert_eq!(mint(), mint());
    }

    // ---- wire format ----

    #[test]
    fn credentials_round_trip_through_borsh() {
        let (root, r1, dev) = (key(1), key(2), key(5));
        let g = genesis_for(&root);
        let account = g.account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        let handoff = sign_handoff(&root, account, 0, &r1);
        let cert = sign_cert(&root, account, device, &dev, 0, 0);

        for (label, bytes, ok) in [
            (
                "genesis",
                borsh_bytes(&g),
                borsh::from_slice::<AccountGenesis>(&borsh_bytes(&g)).map(|v| v == g),
            ),
            (
                "handoff",
                borsh_bytes(&handoff),
                borsh::from_slice::<RootKeyHandoff>(&borsh_bytes(&handoff)).map(|v| v == handoff),
            ),
            (
                "cert",
                borsh_bytes(&cert),
                borsh::from_slice::<DeviceCert>(&borsh_bytes(&cert)).map(|v| v == cert),
            ),
        ] {
            assert!(!bytes.is_empty(), "{label} encodes to nothing");
            assert_eq!(ok.ok(), Some(true), "{label} did not round-trip");
        }
    }

    #[test]
    fn ids_round_trip_through_borsh() {
        let account = genesis_for(&key(1)).account_id();
        let device = DeviceId::mint(account, [3u8; 16]);
        assert_eq!(
            borsh::from_slice::<AccountId>(&borsh_bytes(&account)).expect("decode"),
            account
        );
        assert_eq!(
            borsh::from_slice::<DeviceId>(&borsh_bytes(&device)).expect("decode"),
            device
        );
    }
}
