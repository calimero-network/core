//! Owner-level governance ops, and the root-signed proof they must carry.
//!
//! # Why it is shaped this way
//!
//! **A device key is not the account.** Every governance gate resolves the key
//! that signed an op to the account it speaks for, and then asks whether that
//! account is the owner or an admin. So whoever holds *any* of the owner's device
//! keys passes those gates. For most governance that is the point, since a person
//! acts through their devices. For the few ops that can take a namespace or a group
//! away from its owner, it is the hole: a stolen laptop could promote an
//! accomplice, transfer ownership to them and remove the owner, and nothing the
//! owner still holds could undo it. The ops that carry this proof are exactly those
//! ops. Each one needs a statement signed by the account's **root** key, which a
//! device does not hold.
//!
//! **Self-certifying, like [`crate::DeviceRevocation`], and for the same reason.**
//! A receiver checks the genesis, the handoff chain and the root signature from
//! the op alone, so two replicas that have folded different histories still agree
//! about whether the proof is genuine.
//!
//! **Everything the op does is bound into the signature.** The account, the
//! namespace and the group, which op this is ([`OwnerOpKind`]), and a digest of the
//! op's own bytes, so the proof for "transfer to Bob" cannot be presented with
//! "transfer to Mallory". The epoch is bound for the usual reason. The
//! `counter` is what makes a proof single-use. The group counts the guarded ops it
//! has applied, the proof must name that count, and applying one advances it. A
//! proof that has been spent, or one signed for a later op, names the wrong number
//! and is refused. Nothing in the proof expires, so it can be signed offline, by a
//! root that never touches a node, as long as the signer first reads the counter.
//!
//! **Any epoch the chain reaches is accepted, as for a revocation.** A compromised
//! old root key could therefore still sign one. That is accepted for the reason it
//! is accepted there: whoever holds any root key of an account can already sign a
//! handoff and take it over. The apply path narrows it where it can, by requiring
//! the chain to reach the epoch the group has recorded for the account. See the
//! governance store's guard for that check and for its cost.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{domain_hash, AccountId, PrivateKey};

use crate::domain::{OWNER_OP_BODY_DOMAIN, OWNER_OP_SIGN_DOMAIN};
use crate::error::AccountError;
use crate::signed::{sign_payload, AccountProof, RootSigned, Verified};

/// Which owner-level op a proof authorises.
///
/// Bound into the signature beside the op's digest. The digest alone would pin
/// the op, but naming the kind also means a verifier can refuse an op of the
/// wrong kind before it hashes anything, and a reader of a proof can tell what
/// it is for.
///
/// **Append-only wire format.** Borsh encodes each variant by its position, and
/// that byte is signed. Reordering the variants would change what every proof
/// already signed means.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, BorshSerialize, BorshDeserialize)]
pub enum OwnerOpKind {
    /// `GroupOp::TransferOwnership`: owner-only.
    TransferOwnership,
    /// `RootOp::AdminChanged`: owner-only.
    AdminChanged,
    /// `GroupOp::GroupDelete`: owner-only.
    GroupDelete,
    /// `GroupOp::TeeAdmissionPolicySet` and its V2 form: admin-level.
    TeeAdmissionPolicy,
    /// `GroupOp::TeeAuthoringPolicySet`: admin-level.
    TeeAuthoringPolicy,
    /// `GroupOp::TeeReleaseAdmissionPolicySet` and its V2 form: admin-level.
    TeeReleaseAdmissionPolicy,
}

impl OwnerOpKind {
    /// The byte this kind signs as.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::TransferOwnership => 0,
            Self::AdminChanged => 1,
            Self::GroupDelete => 2,
            Self::TeeAdmissionPolicy => 3,
            Self::TeeAuthoringPolicy => 4,
            Self::TeeReleaseAdmissionPolicy => 5,
        }
    }

    /// Whether only the group's owner may perform this op.
    ///
    /// The TEE policy ops are admin-level. For them the proof must come from the
    /// signing admin's own account, so a stolen admin device cannot set them. It
    /// does not have to come from the owner.
    #[must_use]
    pub const fn owner_only(self) -> bool {
        matches!(
            self,
            Self::TransferOwnership | Self::AdminChanged | Self::GroupDelete
        )
    }

    /// Stable name, for errors and logs.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::TransferOwnership => "transfer_ownership",
            Self::AdminChanged => "admin_changed",
            Self::GroupDelete => "group_delete",
            Self::TeeAdmissionPolicy => "tee_admission_policy",
            Self::TeeAuthoringPolicy => "tee_authoring_policy",
            Self::TeeReleaseAdmissionPolicy => "tee_release_admission_policy",
        }
    }
}

/// A root-signed authorisation for one owner-level op.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct OwnerOpAuthorization {
    /// The account authorising the op. It must be the account the op's signing
    /// device speaks for.
    pub account: AccountId,
    /// The namespace the op is published in.
    pub namespace_id: [u8; 32],
    /// The group the op acts on. For a namespace-level op this is the namespace
    /// root, so it equals `namespace_id`.
    pub group_id: [u8; 32],
    /// Which op this authorises.
    pub kind: OwnerOpKind,
    /// [`OwnerOpAuthorization::op_digest`] of the op's borsh bytes.
    pub op_digest: [u8; 32],
    /// The group's guarded-op counter this proof is for. Valid only while the
    /// group's counter still equals it.
    pub counter: u64,
    /// Which account root-key epoch signed this.
    pub key_epoch: u32,
    /// Signature by the epoch-`key_epoch` root key over
    /// [`OwnerOpAuthorization::signing_payload`].
    pub signature: [u8; 64],
}

/// Everything an [`OwnerOpAuthorization`] binds, apart from the signature.
///
/// Its own type so the minter and the verifier assemble the preimage from one
/// value instead of seven loose arguments that could be passed in the wrong
/// order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerOpTerms {
    /// See [`OwnerOpAuthorization::account`].
    pub account: AccountId,
    /// See [`OwnerOpAuthorization::namespace_id`].
    pub namespace_id: [u8; 32],
    /// See [`OwnerOpAuthorization::group_id`].
    pub group_id: [u8; 32],
    /// See [`OwnerOpAuthorization::kind`].
    pub kind: OwnerOpKind,
    /// See [`OwnerOpAuthorization::op_digest`].
    pub op_digest: [u8; 32],
    /// See [`OwnerOpAuthorization::counter`].
    pub counter: u64,
    /// See [`OwnerOpAuthorization::key_epoch`].
    pub key_epoch: u32,
}

impl OwnerOpTerms {
    /// The canonical bytes the root key signs.
    #[must_use]
    pub fn signing_payload(&self) -> [u8; 32] {
        domain_hash(
            OWNER_OP_SIGN_DOMAIN,
            &[
                self.account.as_bytes(),
                &self.namespace_id,
                &self.group_id,
                &[self.kind.tag()],
                &self.op_digest,
                &self.counter.to_le_bytes(),
                &self.key_epoch.to_le_bytes(),
            ],
        )
    }
}

impl OwnerOpAuthorization {
    /// The commitment a proof carries to its op: a domain-separated hash of the
    /// op's borsh bytes.
    ///
    /// Hashed under its own domain rather than signed as raw bytes, so the
    /// commitment can never equal a digest taken for another purpose, and so the
    /// signed preimage stays fixed-size whatever the op carries.
    #[must_use]
    pub fn op_digest(op_bytes: &[u8]) -> [u8; 32] {
        domain_hash(OWNER_OP_BODY_DOMAIN, &[op_bytes])
    }

    /// The terms this authorisation binds.
    #[must_use]
    pub const fn terms(&self) -> OwnerOpTerms {
        OwnerOpTerms {
            account: self.account,
            namespace_id: self.namespace_id,
            group_id: self.group_id,
            kind: self.kind,
            op_digest: self.op_digest,
            counter: self.counter,
            key_epoch: self.key_epoch,
        }
    }

    /// Mint an authorisation for `terms`, signed by the account root at
    /// `terms.key_epoch`.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(root_sk: &PrivateKey, terms: OwnerOpTerms) -> Result<Self, AccountError> {
        let signature = sign_payload(root_sk, &terms.signing_payload())?;
        Ok(Self {
            account: terms.account,
            namespace_id: terms.namespace_id,
            group_id: terms.group_id,
            kind: terms.kind,
            op_digest: terms.op_digest,
            counter: terms.counter,
            key_epoch: terms.key_epoch,
            signature,
        })
    }
}

impl RootSigned for OwnerOpAuthorization {
    const ACCOUNT_MISMATCH: AccountError = AccountError::OwnerOpAccountMismatch;
    const SIGNATURE_INVALID: AccountError = AccountError::OwnerOpSignatureInvalid;

    fn account(&self) -> AccountId {
        self.account
    }

    fn key_epoch(&self) -> u32 {
        self.key_epoch
    }

    fn payload(&self) -> [u8; 32] {
        self.terms().signing_payload()
    }

    fn signature(&self) -> &[u8; 64] {
        &self.signature
    }
}

/// An [`OwnerOpAuthorization`] together with everything needed to verify it.
pub type SignedOwnerOp = AccountProof<OwnerOpAuthorization>;

/// An [`OwnerOpAuthorization`] whose anchor, chain, and signature have been
/// checked. It says nothing about whether the counter is current, whether the
/// chain reaches the epoch a group has recorded, or whether the account may
/// perform the op. Those are the apply path's questions.
pub type VerifiedOwnerOp = Verified<OwnerOpAuthorization>;
