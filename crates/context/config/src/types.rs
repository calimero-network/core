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

pub type BlockHeight = u64;
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
    Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize, Hash,
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

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ProposalId(Identity);

impl ReprBytes for ProposalId {
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

impl Serialize for ProposalId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let encoded = bs58::encode(self.as_bytes()).into_string();
        serializer.serialize_str(&encoded)
    }
}

impl<'de> Deserialize<'de> for ProposalId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ProposalIdVisitor;

        impl<'de> serde::de::Visitor<'de> for ProposalIdVisitor {
            type Value = ProposalId;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str("a base58-encoded ProposalId string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                ProposalId::from_bytes(|bytes| bs58::decode(value).onto(bytes))
                    .map_err(|e| E::custom(format!("invalid ProposalId: {}", e)))
            }
        }

        deserializer.deserialize_str(ProposalIdVisitor)
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
    /// The height at which the invitation becomes expired.
    pub expiration_height: BlockHeight,
    /// Secret salt.
    pub secret_salt: [u8; 32],
    // The protocol ID to prevent reusing the same invitation on different blockchains.
    // TODO(opt): utilize the integer for the protocol, and possibly a compressed integer
    // representation of protocol+network.
    pub protocol: String,
    // The protocol network (e.g. "mainnet", "testnet", etc.)
    pub network: String,
    // The contract ID for the config context contract on the target protocol.
    pub contract_id: String,
}

/// A container for an open invitation and the inviter's signature over it.
/// This is the object that an existing member (Alice) would generate and send
/// to a new member (Bob).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Deserialize, Serialize)]
pub struct SignedOpenInvitation {
    /// An open invitation to the context
    pub invitation: InvitationFromMember,
    /// Inviter's signature for the invitation payload (hex-encoded)
    pub inviter_signature: String,
}

// The full payload Bob reveals in the second transaction
#[derive(BorshSerialize, BorshDeserialize, Debug, Deserialize, Clone, Serialize)]
pub struct RevealPayloadData {
    /// Signed open invitation
    pub signed_open_invitation: SignedOpenInvitation,
    /// The identity of the member that is going to be invited (invitee).
    /// The owner of the identity should insert this field himself and then
    /// sign the whole structure, and wrapping it into `SignedRevealPayload`.
    pub new_member_identity: ContextIdentity,
}

// This is the final object submitted to the `reveal` method.
#[derive(BorshSerialize, BorshDeserialize, Debug, Deserialize, Clone, Serialize)]
pub struct SignedRevealPayload {
    /// The data that is needed to join the context.
    pub data: RevealPayloadData,
    /// The invitee's signature over the `data` (`RevealPayloadData`).
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
            expiration_height: 1000,
            secret_salt: salt,
            protocol: "near".to_string(),
            network: "devnet".to_string(),
            contract_id: "".to_string(),
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
            invitation.expiration_height,
            invitation_deserialized.expiration_height
        );
        assert_eq!(invitation.secret_salt, invitation_deserialized.secret_salt);
        assert_eq!(invitation.protocol, invitation_deserialized.protocol);
        assert_eq!(invitation.network, invitation_deserialized.network);
        assert_eq!(invitation.contract_id, invitation_deserialized.contract_id);
    }
}
