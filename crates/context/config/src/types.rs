use core::convert::Infallible;
use core::fmt;
use core::fmt::{Debug, Display, Formatter};
use core::marker::PhantomData;
use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use bs58::decode::Result as Bs58Result;
use ed25519_dalek::{Signature, SignatureError, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::repr::{self, LengthMismatch, Repr, ReprBytes, ReprTransmute};

pub type ExpirationTimestamp = u64;
pub type Revision = u64;

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Debug,
    Deserialize,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
#[non_exhaustive]
pub struct Application<'a> {
    pub id: Repr<ApplicationId>,
    pub blob: Repr<BlobId>,
    pub size: u64,
    #[serde(borrow)]
    pub source: ApplicationSource<'a>,
    pub metadata: ApplicationMetadata<'a>,
}

impl<'a> Application<'a> {
    #[must_use]
    pub const fn new(
        id: Repr<ApplicationId>,
        blob: Repr<BlobId>,
        size: u64,
        source: ApplicationSource<'a>,
        metadata: ApplicationMetadata<'a>,
    ) -> Self {
        Application {
            id,
            blob,
            size,
            source,
            metadata,
        }
    }
}

#[derive(
    Eq,
    Ord,
    Copy,
    Debug,
    Deserialize,
    Clone,
    PartialEq,
    PartialOrd,
    BorshSerialize,
    BorshDeserialize,
    Hash,
    Serialize,
)]
pub struct Identity([u8; 32]);

impl Identity {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl ReprBytes for Identity {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(Self)
    }
}

impl From<[u8; 32]> for Identity {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

#[derive(
    Eq,
    Ord,
    Copy,
    Debug,
    Deserialize,
    Clone,
    PartialEq,
    PartialOrd,
    BorshSerialize,
    BorshDeserialize,
    Hash,
    Serialize,
)]
pub struct SignerId(Identity);

impl ReprBytes for SignerId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

impl SignerId {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

impl From<[u8; 32]> for SignerId {
    fn from(value: [u8; 32]) -> Self {
        Self(Identity(value))
    }
}

#[derive(
    Eq,
    Ord,
    Copy,
    Debug,
    Deserialize,
    Clone,
    PartialEq,
    PartialOrd,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
)]
pub struct ContextId(Identity);

impl ContextId {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

impl ReprBytes for ContextId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

impl From<[u8; 32]> for ContextId {
    fn from(value: [u8; 32]) -> Self {
        Self(Identity(value))
    }
}

#[derive(
    Eq,
    Ord,
    Debug,
    Clone,
    PartialEq,
    PartialOrd,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct ContextStorageEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(
    Eq,
    Ord,
    Copy,
    Debug,
    Deserialize,
    Clone,
    PartialEq,
    PartialOrd,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
)]
pub struct ContextIdentity(Identity);

impl ReprBytes for ContextIdentity {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

impl From<[u8; 32]> for ContextIdentity {
    fn from(value: [u8; 32]) -> Self {
        Self(Identity(value))
    }
}

#[derive(
    Eq,
    Ord,
    Copy,
    Debug,
    Deserialize,
    Clone,
    PartialEq,
    PartialOrd,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Hash,
)]
pub struct ContextGroupId(Identity);

impl ContextGroupId {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

impl ReprBytes for ContextGroupId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

impl From<[u8; 32]> for ContextGroupId {
    fn from(value: [u8; 32]) -> Self {
        Self(Identity(value))
    }
}

/// Maximum number of governance DAG heads that may appear in a
/// [`GovernanceParentEdge`].
///
/// Steady-state head sets have 1–3 entries; concurrent admin activity in a
/// partition might briefly hit ~10. The bound exists to stop a malicious
/// sender from forcing a receiver to allocate or look up an unbounded
/// number of head entries per state-delta receive (DoS surface). Tuned for
/// ~3× headroom over the realistic worst case while staying well below
/// DoS-relevant sizes.
///
/// Defined here (rather than at the receiver) so the manual
/// [`BorshDeserialize`] impl on [`GovernanceParentEdge`] can enforce it
/// *before* allocating the heads vector. Receivers also re-check the bound
/// at use time as defense-in-depth (covers direct struct construction).
///
/// Not a wire-format change — bumping the constant is one-line PR.
pub const MAX_GOVERNANCE_DAG_HEADS: usize = 32;

/// Cross-DAG **parent edge** embedded in state deltas at sign time (core#2716
/// Phase 4).
///
/// Names the exact governance DAG cut the signer authored under — the set of
/// governance heads at sign time. Receivers resolve membership at that cut
/// (`acl_view_at`) to perform the apply-time authorization check — "was this
/// signer a member at the named cut?" — and buffer the delta when the
/// referenced `governance_dag_heads` are not yet known locally.
///
/// The group/scope is intentionally NOT carried here: the receiver derives it
/// from the context (canonical context→group mapping), so a signer cannot cite
/// a *different* group's heads to authorize a write into this context. That is
/// what makes a separate signed `group_id` (and the `GroupIdCheck` that
/// validated it) unnecessary.
///
/// `governance_dag_heads` entries are content-hashes of [`SignedNamespaceOp`]
/// — the same identity scheme used by `parent_op_hashes` and the persisted
/// `NamespaceGovHeadValue.dag_heads`. Empty `Vec` means "before any governance
/// op" (group genesis). Bounded by [`MAX_GOVERNANCE_DAG_HEADS`] at deserialize
/// time.
#[derive(Clone, Debug, Eq, PartialEq, Hash, BorshSerialize, Serialize, Deserialize)]
pub struct GovernanceParentEdge {
    /// Namespace governance DAG heads at sign time. Each entry is the content
    /// hash of a [`SignedNamespaceOp`] (`signed_namespace_op.content_hash()`).
    /// Empty `Vec` means "before any governance op" (group genesis). Length
    /// is bounded by [`MAX_GOVERNANCE_DAG_HEADS`] on both wire-decode paths
    /// (borsh and serde — see the manual `BorshDeserialize` and the
    /// `deserialize_with` attribute below).
    #[serde(deserialize_with = "deserialize_bounded_dag_heads")]
    pub governance_dag_heads: Vec<[u8; 32]>,
}

fn deserialize_bounded_dag_heads<'de, D>(deserializer: D) -> Result<Vec<[u8; 32]>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let heads: Vec<[u8; 32]> = <Vec<[u8; 32]> as Deserialize>::deserialize(deserializer)?;
    if heads.len() > MAX_GOVERNANCE_DAG_HEADS {
        return Err(serde::de::Error::custom(format!(
            "GovernanceParentEdge.governance_dag_heads length {} exceeds \
             MAX_GOVERNANCE_DAG_HEADS={}",
            heads.len(),
            MAX_GOVERNANCE_DAG_HEADS,
        )));
    }
    if has_duplicate_heads(&heads) {
        return Err(serde::de::Error::custom(
            "GovernanceParentEdge.governance_dag_heads contains duplicate entries — \
             a valid governance DAG head set is unique by construction",
        ));
    }
    Ok(heads)
}

/// Reject duplicate entries in a head set. A valid governance DAG never has
/// duplicates among its heads (heads are unique content-hashes by definition);
/// duplicates surfacing on the wire indicate malformed input or a sender
/// trying to slip a multi-entry vector through fast-path equality checks
/// against a single-head receiver.
fn has_duplicate_heads(heads: &[[u8; 32]]) -> bool {
    use std::collections::HashSet;
    let mut seen: HashSet<&[u8; 32]> = HashSet::with_capacity(heads.len());
    for h in heads {
        if !seen.insert(h) {
            return true;
        }
    }
    false
}

/// Returned by [`GovernanceParentEdge::new`] when the supplied
/// `governance_dag_heads` are malformed (oversized or contain duplicates).
///
/// Catching this at construction time prevents the asymmetric encode/decode
/// problem: a sender that builds a malformed edge via
/// `borsh::to_vec(&GovernanceParentEdge { ... })` produces bytes the
/// receiver's bounded `BorshDeserialize` rejects, silently breaking
/// state-delta propagation for that context.
#[derive(Debug, ThisError)]
pub enum GovernanceParentEdgeError {
    #[error(
        "GovernanceParentEdge.governance_dag_heads length {len} exceeds \
         MAX_GOVERNANCE_DAG_HEADS={max}"
    )]
    TooManyHeads { len: usize, max: usize },
    #[error(
        "GovernanceParentEdge.governance_dag_heads contains duplicate entries — \
         a valid governance DAG head set is unique by construction"
    )]
    DuplicateHeads,
}

impl GovernanceParentEdge {
    /// Construct a `GovernanceParentEdge`, validating that
    /// `governance_dag_heads.len() <= MAX_GOVERNANCE_DAG_HEADS` and that the
    /// head set contains no duplicates.
    ///
    /// In legitimate use both bounds are unreachable — head sets are 1–3
    /// entries in steady state and unique by construction — so the error path
    /// is for defensive reporting (a node whose local governance DAG has
    /// somehow accumulated a pathological number of heads or duplicates
    /// should refuse to emit an edge rather than ship one the network
    /// rejects).
    pub fn new(governance_dag_heads: Vec<[u8; 32]>) -> Result<Self, GovernanceParentEdgeError> {
        if governance_dag_heads.len() > MAX_GOVERNANCE_DAG_HEADS {
            return Err(GovernanceParentEdgeError::TooManyHeads {
                len: governance_dag_heads.len(),
                max: MAX_GOVERNANCE_DAG_HEADS,
            });
        }
        if has_duplicate_heads(&governance_dag_heads) {
            return Err(GovernanceParentEdgeError::DuplicateHeads);
        }
        Ok(Self {
            governance_dag_heads,
        })
    }
}

/// Manual [`BorshDeserialize`] enforcing [`MAX_GOVERNANCE_DAG_HEADS`] before
/// allocating the heads vector.
///
/// The derived impl would call `Vec::deserialize_reader` which reads the u32
/// length and immediately allocates with `Vec::with_capacity(len)` — a
/// malicious sender claiming `len = 1_000_000` triggers a 32 MB allocation
/// before any of our bounds checks run. This impl reads the length first,
/// rejects if it exceeds the bound, and only then allocates and reads
/// elements. Field order matches the derived `BorshSerialize` impl above.
impl BorshDeserialize for GovernanceParentEdge {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let len = u32::deserialize_reader(reader)? as usize;
        if len > MAX_GOVERNANCE_DAG_HEADS {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "GovernanceParentEdge.governance_dag_heads length {len} exceeds \
                     MAX_GOVERNANCE_DAG_HEADS={MAX_GOVERNANCE_DAG_HEADS}",
                ),
            ));
        }

        let mut governance_dag_heads = Vec::with_capacity(len);
        for _ in 0..len {
            governance_dag_heads.push(<[u8; 32]>::deserialize_reader(reader)?);
        }

        if has_duplicate_heads(&governance_dag_heads) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "GovernanceParentEdge.governance_dag_heads contains duplicate entries \
                 — a valid governance DAG head set is unique by construction",
            ));
        }

        Ok(Self {
            governance_dag_heads,
        })
    }
}

#[derive(
    Eq,
    Ord,
    Copy,
    Debug,
    Deserialize,
    Clone,
    PartialEq,
    PartialOrd,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Hash,
)]
pub struct AppKey(Identity);

impl AppKey {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

impl ReprBytes for AppKey {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

impl From<[u8; 32]> for AppKey {
    fn from(value: [u8; 32]) -> Self {
        Self(Identity(value))
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct BlobId(Identity);

impl ReprBytes for BlobId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ApplicationId(Identity);

impl ReprBytes for ApplicationId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Debug,
    Default,
    Deserialize,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
#[expect(clippy::exhaustive_structs, reason = "Exhaustive")]
pub struct ApplicationSource<'a>(#[serde(borrow)] pub Cow<'a, str>);

impl ApplicationSource<'_> {
    #[must_use]
    pub fn to_owned(self) -> ApplicationSource<'static> {
        ApplicationSource(Cow::Owned(self.0.into_owned()))
    }
}

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Debug,
    Default,
    Deserialize,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
#[expect(clippy::exhaustive_structs, reason = "Exhaustive")]
pub struct ApplicationMetadata<'a>(#[serde(borrow)] pub Repr<Cow<'a, [u8]>>);

impl ApplicationMetadata<'_> {
    #[must_use]
    pub fn to_owned(self) -> ApplicationMetadata<'static> {
        ApplicationMetadata(Repr::new(Cow::Owned(self.0.into_inner().into_owned())))
    }
}

impl ReprBytes for Signature {
    type EncodeBytes<'a> = [u8; 64];
    type DecodeBytes = [u8; 64];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.to_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(|b| Self::from_bytes(&b))
    }
}

#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum VerificationKeyParseError {
    #[error(transparent)]
    LengthMismatch(LengthMismatch),
    #[error("invalid key: {0}")]
    InvalidVerificationKey(SignatureError),
}

impl ReprBytes for VerifyingKey {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = VerificationKeyParseError;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.to_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        use VerificationKeyParseError::{InvalidVerificationKey, LengthMismatch};

        let bytes = Self::DecodeBytes::from_bytes(f).map_err(|e| e.map(LengthMismatch))?;

        Self::from_bytes(&bytes)
            .map_err(|e| repr::ReprError::DecodeError(InvalidVerificationKey(e)))
    }
}

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum Capability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

impl Capability {
    /// Returns the bitmask for this capability (single bit set).
    #[must_use]
    pub const fn as_bit(self) -> u8 {
        1 << (self as u8)
    }
}

#[derive(Eq, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signed<T> {
    payload: Repr<Box<[u8]>>,
    signature: Repr<Signature>,

    #[serde(skip)]
    _priv: PhantomData<T>,
}

#[derive(ThisError)]
#[non_exhaustive]
pub enum ConfigError<E> {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("json error: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("derivation error: {0}")]
    DerivationError(E),
    #[error(transparent)]
    VerificationKeyParseError(#[from] repr::ReprError<VerificationKeyParseError>),
}

impl<E: Display> Debug for ConfigError<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl<T: Serialize> Signed<T> {
    pub fn new<R, F>(payload: &T, sign: F) -> Result<Self, ConfigError<R::Error>>
    where
        R: IntoResult<Signature>,
        F: FnOnce(&[u8]) -> R,
    {
        let payload = serde_json::to_vec(&payload)?.into_boxed_slice();

        let signature = sign(&payload)
            .into_result()
            .map_err(ConfigError::DerivationError)?;

        Ok(Self {
            payload: Repr::new(payload),
            signature: Repr::new(signature),
            _priv: PhantomData,
        })
    }
}

pub trait IntoResult<T> {
    type Error;

    fn into_result(self) -> Result<T, Self::Error>;
}

impl<T> IntoResult<T> for T {
    type Error = Infallible;

    fn into_result(self) -> Result<T, Self::Error> {
        Ok(self)
    }
}

impl<T, E> IntoResult<T> for Result<T, E> {
    type Error = E;

    fn into_result(self) -> Result<T, Self::Error> {
        self
    }
}

impl<'a, T: Deserialize<'a>> Signed<T> {
    pub fn parse<R, F>(&'a self, f: F) -> Result<T, ConfigError<R::Error>>
    where
        R: IntoResult<SignerId>,
        F: FnOnce(&T) -> R,
    {
        let parsed = serde_json::from_slice(&self.payload)?;

        let bytes = f(&parsed)
            .into_result()
            .map_err(ConfigError::DerivationError)?;

        let key = bytes
            .rt::<VerifyingKey>()
            .map_err(ConfigError::VerificationKeyParseError)?;

        key.verify(&self.payload, &self.signature)
            .map_or(Err(ConfigError::InvalidSignature), |()| Ok(parsed))
    }
}

/// The structure represents an open invitation payload that allows any party to claim it.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Deserialize, Serialize)]
pub struct InvitationFromMember {
    /// The identity of the inviter (public key). The inviter should be a member of the context
    /// that he is inviting to.
    pub inviter_identity: ContextIdentity,
    /// Context ID for the invitation.
    pub context_id: ContextId,
    /// Unix timestamp (seconds) at which the invitation expires.
    pub expiration_timestamp: ExpirationTimestamp,
    /// Secret salt.
    pub secret_salt: [u8; 32],
}

/// A container for an open invitation and the inviter's signature over it.
/// This is the object that an existing member (Alice) would generate and send
/// to a new member (Bob).
/// The fields below `inviter_signature` are **not** covered by the signature.
/// They are populated by the inviter from local state so the joiner can
/// bootstrap the application without an external source of truth.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SignedOpenInvitation {
    /// An open invitation to the context
    pub invitation: InvitationFromMember,
    /// Inviter's signature for the invitation payload (hex-encoded)
    pub inviter_signature: String,
    /// Application ID for the context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<[u8; 32]>,
    /// Bytecode blob ID for the application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<[u8; 32]>,
    /// Application source URL (registry URL, HTTP URL, or calimero:// stub).
    /// Enables the joiner to re-download from the registry if blob sharing fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Group ID that owns this context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<[u8; 32]>,
}

// The full payload Bob reveals in the second transaction
#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct RevealPayloadData {
    /// Signed open invitation
    pub signed_open_invitation: SignedOpenInvitation,
    /// The identity of the member that is going to be invited (invitee).
    /// The owner of the identity should insert this field himself and then
    /// sign the whole structure, and wrapping it into `SignedRevealPayload`.
    pub new_member_identity: ContextIdentity,
}

// This is the final object submitted to the `reveal` method.
#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct SignedRevealPayload {
    /// The data that is needed to join the context.
    pub data: RevealPayloadData,
    /// The invitee's signature over the `data` (`RevealPayloadData`).
    pub invitee_signature: String,
}

/// An open invitation payload for joining a context group.
/// Created and signed by a group admin.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Deserialize, Serialize)]
pub struct GroupInvitationFromAdmin {
    /// The identity of the inviter (group admin public key).
    pub inviter_identity: SignerId,
    /// The group being invited to.
    pub group_id: ContextGroupId,
    /// Unix timestamp (seconds) at which the invitation expires.
    pub expiration_timestamp: ExpirationTimestamp,
    /// Secret salt for MEV protection.
    pub secret_salt: [u8; 32],
    /// The role the invitee should be granted (0 = Admin, 1 = Member,
    /// 2 = ReadOnly). Covered by the admin's signature so the joiner
    /// cannot escalate. Defaults to 1 (Member) for backward compat.
    #[serde(default = "default_invited_role")]
    pub invited_role: u8,
}

fn default_invited_role() -> u8 {
    1 // Member
}

/// A container for a group invitation and the admin's signature over it.
///
/// The fields below `inviter_signature` are **not** covered by the signature.
/// They are populated by the inviter from local state so the joiner can
/// pre-populate `GroupMetaValue` with the correct values instead of
/// zero placeholders. Without this, peers that join via invitation end
/// up with `target_application_id = ZERO` for the group, while the
/// originator has the real value — causing `compute_group_state_hash`
/// to diverge persistently between peers.
///
/// # Wire-format compatibility (lockstep assumption)
///
/// The `#[serde(default, skip_serializing_if = ...)]` on the trailing
/// `Option` fields makes them **JSON**-backwards-compatible only — that
/// is the JSON-RPC path (`calimero-client-py`). Borsh ignores serde
/// attributes: appending a field to a borsh struct is NOT wire-compatible
/// (an old peer decoding a new payload errors on trailing bytes; a new
/// peer decoding an old payload hits EOF on the option tag). This struct
/// also rides the **binary P2P wire** — `create_recursive_invitations`
/// embeds it in a `SignedNamespaceOp`. So mixed-version peers exchanging
/// these namespace ops over borsh will reject each other's gossip. That
/// is an **accepted lockstep-upgrade assumption** (the same one that
/// applied when `application_id` was added the same way): a namespace
/// must run a single core version across its peers during a migration.
/// If true rolling-version compat is ever needed, give this struct an
/// explicit version tag rather than relying on serde defaults.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Deserialize, Serialize)]
pub struct SignedGroupOpenInvitation {
    /// The open invitation to the group.
    pub invitation: GroupInvitationFromAdmin,
    /// Admin's signature for the invitation payload (hex-encoded).
    pub inviter_signature: String,
    /// Application ID for the group (unsigned bootstrap field).
    /// `None` for backwards compatibility with invitations created before
    /// this field was added; joiners fall back to zero when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<[u8; 32]>,
    /// `GroupMeta.app_key` for the group (unsigned bootstrap field).
    ///
    /// The inviter has already derived its local
    /// `app_key = blob_id(app_meta.bytecode)` at `create_group` time;
    /// shipping that value lets the joiner pre-populate its local
    /// namespace-root meta with the same value the cascade predicate
    /// (`from_app_key == descendant.app_key`) checks. Without this, the
    /// joiner's pre-populated `app_key` is `[0u8; 32]` and any
    /// `CascadeTargetApplicationSet` op the joiner applies locally
    /// silently skips the subtree — divergence between originator
    /// (cascade applied) and joiner (cascade no-op'd). Sibling of
    /// `application_id` above; same null-safety: `None` for
    /// backwards compatibility with pre-field invitations, joiners
    /// fall back to zero + the existing self-heal path.
    ///
    /// # Trust model
    ///
    /// This field is **unsigned** (it sits below `inviter_signature`), so
    /// the inviter is trusted to pin it honestly. A malicious or buggy
    /// inviter sending `Some(wrong_value)` is accepted by the joiner
    /// without verification — the local-bytecode re-derivation in
    /// `join_group` only fires for the `None` case, not to override a
    /// present-but-wrong value. The blast radius is bounded: a wrong
    /// `app_key` makes the `from_app_key` cascade predicate skip the
    /// subtree for that one joiner's view (a cascade **DoS** for that
    /// peer), NOT state corruption or an auth bypass. Anyone adding
    /// further cascade-gated semantics keyed on `app_key` must keep this
    /// in mind — or move to always re-deriving locally and ignoring the
    /// wire value, which would close the forge vector entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_key: Option<[u8; 32]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smoke_invitation_borsh_roundtrip() {
        let inviter_identity_bytes = [1u8; 32];
        let context_id_bytes = [2u8; 32];
        let salt = [3u8; 32];

        let invitation = InvitationFromMember {
            inviter_identity: inviter_identity_bytes.into(),
            context_id: context_id_bytes.into(),
            expiration_timestamp: 1000,
            secret_salt: salt,
        };
        let invitation_borsh =
            borsh::to_vec(&invitation).expect("Failed to Borsh serialize the invitation");
        let invitation_deserialized: InvitationFromMember = borsh::from_slice(&invitation_borsh)
            .expect("Failed to Borsh deserialize the invitation");

        assert_eq!(
            invitation.inviter_identity,
            invitation_deserialized.inviter_identity
        );
        assert_eq!(invitation.context_id, invitation_deserialized.context_id);
        assert_eq!(
            invitation.expiration_timestamp,
            invitation_deserialized.expiration_timestamp
        );
        assert_eq!(invitation.secret_salt, invitation_deserialized.secret_salt);
    }

    #[test]
    fn signed_open_invitation_serde_roundtrip_with_app_fields() {
        let invitation = InvitationFromMember {
            inviter_identity: [0x11; 32].into(),
            context_id: [0x22; 32].into(),
            expiration_timestamp: 1_700_000_000,
            secret_salt: [0x33; 32],
        };

        let signed = SignedOpenInvitation {
            invitation,
            inviter_signature: "deadbeef".to_string(),
            application_id: Some([0x44; 32]),
            blob_id: Some([0x55; 32]),
            source: Some("https://registry.example.com/apps/my-app".to_string()),
            group_id: Some([0x66; 32]),
        };

        let json = serde_json::to_string(&signed).expect("serialize");
        let decoded: SignedOpenInvitation = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.inviter_signature, "deadbeef");
        assert_eq!(decoded.application_id, Some([0x44; 32]));
        assert_eq!(decoded.blob_id, Some([0x55; 32]));
        assert_eq!(
            decoded.source.as_deref(),
            Some("https://registry.example.com/apps/my-app")
        );
    }

    #[test]
    fn signed_open_invitation_serde_backward_compat() {
        let json = r#"{
            "invitation": {
                "inviter_identity": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                "context_id": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                "expiration_timestamp": 1000,
                "secret_salt": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
            },
            "inviter_signature": "abc123"
        }"#;

        let decoded: SignedOpenInvitation =
            serde_json::from_str(json).expect("deserialize old format");
        assert_eq!(decoded.inviter_signature, "abc123");
        assert_eq!(decoded.application_id, None);
        assert_eq!(decoded.blob_id, None);
        assert_eq!(decoded.source, None);
    }

    #[test]
    fn governance_edge_borsh_roundtrip() {
        let edge = GovernanceParentEdge::new(vec![[0x01; 32], [0x02; 32], [0x03; 32]])
            .expect("within MAX_GOVERNANCE_DAG_HEADS");

        let bytes = borsh::to_vec(&edge).expect("borsh serialize");
        let decoded: GovernanceParentEdge = borsh::from_slice(&bytes).expect("borsh deserialize");

        assert_eq!(edge.governance_dag_heads, decoded.governance_dag_heads);
    }

    #[test]
    fn governance_edge_empty_heads_means_genesis() {
        let edge = GovernanceParentEdge::new(Vec::new()).expect("empty heads accepted");

        let bytes = borsh::to_vec(&edge).expect("borsh serialize");
        let decoded: GovernanceParentEdge = borsh::from_slice(&bytes).expect("borsh deserialize");

        assert!(decoded.governance_dag_heads.is_empty());
    }

    #[test]
    fn governance_edge_new_rejects_oversized_heads() {
        // Symmetric to the borsh-decode bound: catching this at construction
        // time prevents the asymmetric encode/decode bug where a sender
        // builds an oversized edge locally and ships it for the receiver to
        // reject.
        let oversized: Vec<[u8; 32]> = (0..MAX_GOVERNANCE_DAG_HEADS + 1)
            .map(|i| {
                let mut h = [0u8; 32];
                h[0] = i as u8;
                h
            })
            .collect();
        let err = GovernanceParentEdge::new(oversized).expect_err("oversized must be rejected");
        match err {
            GovernanceParentEdgeError::TooManyHeads { len, max } => {
                assert_eq!(max, MAX_GOVERNANCE_DAG_HEADS);
                assert_eq!(len, MAX_GOVERNANCE_DAG_HEADS + 1);
            }
            other => panic!("expected TooManyHeads, got {other:?}"),
        }
    }

    #[test]
    fn governance_edge_new_rejects_duplicate_heads() {
        // A valid head set has unique entries by construction. Catching
        // duplicates at the constructor mirrors the borsh + serde decode
        // checks: same invariant, enforced at every entry point.
        let h = [0xAB; 32];
        let err =
            GovernanceParentEdge::new(vec![h, h]).expect_err("duplicate heads must be rejected");
        assert!(matches!(err, GovernanceParentEdgeError::DuplicateHeads));
    }

    #[test]
    fn governance_edge_borsh_rejects_duplicate_heads() {
        // Hand-encode a wire payload with two identical heads to confirm
        // BorshDeserialize rejects after reading. (Sender-side construction
        // can't produce this via `new()`, but a malicious peer can hand-roll
        // bytes.)
        let h = [0xAB; 32];
        let mut payload = Vec::new();
        payload.extend_from_slice(&2u32.to_le_bytes()); // heads len = 2
        payload.extend_from_slice(&h);
        payload.extend_from_slice(&h);

        let err = borsh::from_slice::<GovernanceParentEdge>(&payload)
            .expect_err("duplicate heads must be rejected");
        assert!(
            format!("{err}").contains("duplicate"),
            "expected duplicate error, got: {err}"
        );
    }

    #[test]
    fn governance_edge_serde_rejects_duplicate_heads() {
        // JSON path mirrors borsh — duplicates rejected at decode time.
        let h = (0..32).map(|_| "171").collect::<Vec<_>>().join(",");
        let json = format!(r#"{{"governance_dag_heads":[[{h}],[{h}]]}}"#,);
        let err = serde_json::from_str::<GovernanceParentEdge>(&json)
            .expect_err("duplicate heads must reject");
        assert!(
            format!("{err}").contains("duplicate"),
            "expected duplicate error, got: {err}"
        );
    }

    #[test]
    fn governance_edge_borsh_rejects_oversized_heads() {
        // Construct a wire-format payload that claims a heads-vector
        // longer than MAX_GOVERNANCE_DAG_HEADS. The manual BorshDeserialize
        // impl must reject it before allocating, so we encode by hand
        // (a legitimately constructed and serialized value would be at
        // most MAX entries — we want to simulate a hostile peer).
        let mut payload = Vec::new();
        // heads length: u32 LE, just over the bound
        let oversized_len = (MAX_GOVERNANCE_DAG_HEADS + 1) as u32;
        payload.extend_from_slice(&oversized_len.to_le_bytes());
        // We do NOT need to append the head bytes — the bound check
        // fires before we try to read them. Confirms we reject early.

        let err = borsh::from_slice::<GovernanceParentEdge>(&payload)
            .expect_err("oversized heads must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("MAX_GOVERNANCE_DAG_HEADS"),
            "error should mention MAX_GOVERNANCE_DAG_HEADS, got: {msg}"
        );
    }

    #[test]
    fn governance_edge_borsh_accepts_at_bound() {
        // Boundary case: exactly MAX_GOVERNANCE_DAG_HEADS entries must
        // round-trip cleanly.
        let heads: Vec<[u8; 32]> = (0..MAX_GOVERNANCE_DAG_HEADS)
            .map(|i| {
                let mut h = [0u8; 32];
                h[0] = i as u8;
                h
            })
            .collect();
        let edge = GovernanceParentEdge::new(heads.clone()).expect("at-bound accepted");

        let bytes = borsh::to_vec(&edge).expect("serialize");
        let decoded: GovernanceParentEdge = borsh::from_slice(&bytes).expect("deserialize");
        assert_eq!(decoded.governance_dag_heads.len(), MAX_GOVERNANCE_DAG_HEADS);
        assert_eq!(decoded.governance_dag_heads, heads);
    }

    #[test]
    fn governance_edge_serde_rejects_oversized_heads() {
        // Serde-derived Deserialize must enforce the same bound as
        // BorshDeserialize — without this, a malicious JSON payload
        // could bypass the wire-format size limit.
        let json = format!(
            r#"{{"governance_dag_heads":[{}]}}"#,
            (0..(MAX_GOVERNANCE_DAG_HEADS + 1))
                .map(|i| format!(
                    "[{}]",
                    (0..32)
                        .map(|j| if j == 0 {
                            (i % 256).to_string()
                        } else {
                            "0".to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                ))
                .collect::<Vec<_>>()
                .join(","),
        );

        let err =
            serde_json::from_str::<GovernanceParentEdge>(&json).expect_err("oversized must reject");
        assert!(
            format!("{err}").contains("MAX_GOVERNANCE_DAG_HEADS"),
            "expected MAX_GOVERNANCE_DAG_HEADS error, got: {err}"
        );
    }

    #[test]
    fn governance_edge_serde_roundtrip() {
        let edge = GovernanceParentEdge::new(vec![[0xAA; 32]]).expect("within bound");

        let json = serde_json::to_string(&edge).expect("serialize");
        let decoded: GovernanceParentEdge = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(edge.governance_dag_heads, decoded.governance_dag_heads);
    }

    #[test]
    fn signed_open_invitation_none_fields_omitted_in_json() {
        let invitation = InvitationFromMember {
            inviter_identity: [0; 32].into(),
            context_id: [0; 32].into(),
            expiration_timestamp: 0,
            secret_salt: [0; 32],
        };

        let signed = SignedOpenInvitation {
            invitation,
            inviter_signature: "sig".to_string(),
            application_id: None,
            blob_id: None,
            source: None,
            group_id: None,
        };

        let json = serde_json::to_string(&signed).expect("serialize");
        assert!(!json.contains("application_id"));
        assert!(!json.contains("blob_id"));
        assert!(!json.contains("source"));
    }
}
