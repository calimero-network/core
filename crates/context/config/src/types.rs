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
}

/// A container for a group invitation and the admin's signature over it.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Deserialize, Serialize)]
pub struct SignedGroupOpenInvitation {
    /// The open invitation to the group.
    pub invitation: GroupInvitationFromAdmin,
    /// Admin's signature for the invitation payload (hex-encoded).
    pub inviter_signature: String,
}

/// The full payload the joiner reveals in the second transaction.
#[derive(BorshSerialize, BorshDeserialize, Debug, Deserialize, Clone, Serialize)]
pub struct GroupRevealPayloadData {
    /// The signed open invitation from the admin.
    pub signed_open_invitation: SignedGroupOpenInvitation,
    /// The identity of the new member joining the group.
    pub new_member_identity: SignerId,
}

/// The final object submitted to the `reveal_group_invitation` method.
#[derive(BorshSerialize, BorshDeserialize, Debug, Deserialize, Clone, Serialize)]
pub struct SignedGroupRevealPayload {
    /// The data needed to join the group.
    pub data: GroupRevealPayloadData,
    /// The joiner's signature over the `data`.
    pub invitee_signature: String,
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
