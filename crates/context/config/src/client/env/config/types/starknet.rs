use std::collections::BTreeMap;

use starknet::core::codec::{Decode, Encode, Error, FeltWriter};
use starknet::core::types::Felt;

use crate::repr::{Repr, ReprBytes, ReprTransmute};
use crate::types::SignerId;

// Base type for all Starknet Felt pairs
#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct FeltPair {
    pub high: Felt,
    pub low: Felt,
}

// Newtype pattern following types.rs style
#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct ContextId(FeltPair);

#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct ContextIdentity(FeltPair);

#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct ApplicationId(FeltPair);

#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct ApplicationBlob(FeltPair);

// Single conversion trait
pub trait IntoFeltPair {
    fn into_felt_pair(self) -> (Felt, Felt);
}

impl From<(Felt, Felt)> for FeltPair {
    fn from(value: (Felt, Felt)) -> Self {
        FeltPair {
            high: value.0,
            low: value.1,
        }
    }
}

#[derive(Default, Debug)]
pub struct CallData(pub Vec<u8>);

impl FeltWriter for CallData {
    fn write(&mut self, felt: Felt) {
        self.0.extend(felt.to_bytes_be())
    }
}

// Implement for our base types
impl IntoFeltPair for crate::types::ContextId {
    fn into_felt_pair(self) -> (Felt, Felt) {
        let bytes = self.as_bytes();
        let mid_point = bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);
        (
            Felt::from_bytes_be_slice(high_bytes),
            Felt::from_bytes_be_slice(low_bytes),
        )
    }
}

impl IntoFeltPair for crate::types::ContextIdentity {
    fn into_felt_pair(self) -> (Felt, Felt) {
        let bytes = self.as_bytes();
        let mid_point = bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);
        (
            Felt::from_bytes_be_slice(high_bytes),
            Felt::from_bytes_be_slice(low_bytes),
        )
    }
}

impl IntoFeltPair for crate::types::ApplicationId {
    fn into_felt_pair(self) -> (Felt, Felt) {
        let bytes = self.as_bytes();
        let mid_point = bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);
        (
            Felt::from_bytes_be_slice(high_bytes),
            Felt::from_bytes_be_slice(low_bytes),
        )
    }
}

impl IntoFeltPair for crate::types::BlobId {
    fn into_felt_pair(self) -> (Felt, Felt) {
        let bytes = self.as_bytes();
        let mid_point = bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);
        (
            Felt::from_bytes_be_slice(high_bytes),
            Felt::from_bytes_be_slice(low_bytes),
        )
    }
}

// Add IntoFeltPair implementation for SignerId
impl IntoFeltPair for SignerId {
    fn into_felt_pair(self) -> (Felt, Felt) {
        let bytes = self.as_bytes();
        let mid_point = bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);
        (
            Felt::from_bytes_be_slice(high_bytes),
            Felt::from_bytes_be_slice(low_bytes),
        )
    }
}

// Add From implementations for FeltPair
impl From<crate::types::ContextId> for FeltPair {
    fn from(value: crate::types::ContextId) -> Self {
        value.into_felt_pair().into()
    }
}

impl From<crate::types::ContextIdentity> for FeltPair {
    fn from(value: crate::types::ContextIdentity) -> Self {
        value.into_felt_pair().into()
    }
}

impl From<crate::types::ApplicationId> for FeltPair {
    fn from(value: crate::types::ApplicationId) -> Self {
        value.into_felt_pair().into()
    }
}

impl From<crate::types::BlobId> for FeltPair {
    fn from(value: crate::types::BlobId) -> Self {
        value.into_felt_pair().into()
    }
}

// Simplify the existing From implementations
impl From<crate::types::ContextId> for ContextId {
    fn from(value: crate::types::ContextId) -> Self {
        Self(value.into())
    }
}

impl From<crate::types::ContextIdentity> for ContextIdentity {
    fn from(value: crate::types::ContextIdentity) -> Self {
        Self(value.into())
    }
}

impl From<crate::types::ApplicationId> for ApplicationId {
    fn from(value: crate::types::ApplicationId) -> Self {
        Self(value.into())
    }
}

impl From<crate::types::BlobId> for ApplicationBlob {
    fn from(value: crate::types::BlobId) -> Self {
        Self(value.into())
    }
}

// Add From<SignerId> for ContextIdentity
impl From<SignerId> for ContextIdentity {
    fn from(value: SignerId) -> Self {
        let (high, low) = value.into_felt_pair();
        Self(FeltPair { high, low })
    }
}

// Add From<Repr<ContextIdentity>> for ContextIdentity
impl From<Repr<crate::types::ContextIdentity>> for ContextIdentity {
    fn from(value: Repr<crate::types::ContextIdentity>) -> Self {
        let (high, low) = value.into_inner().into_felt_pair();
        Self(FeltPair { high, low })
    }
}

#[derive(Debug, Encode)]
pub enum RequestKind {
    Context(ContextRequest),
}

impl From<crate::RequestKind<'_>> for RequestKind {
    fn from(value: crate::RequestKind<'_>) -> Self {
        match value {
            crate::RequestKind::Context(ctx_req) => RequestKind::Context(ctx_req.into()),
        }
    }
}

#[derive(Debug, Encode)]
pub struct ContextRequest {
    pub context_id: ContextId,
    pub kind: ContextRequestKind,
}

impl From<crate::ContextRequest<'_>> for ContextRequest {
    fn from(value: crate::ContextRequest<'_>) -> Self {
        ContextRequest {
            context_id: (*value.context_id).into(),
            kind: value.kind.into(),
        }
    }
}

#[derive(Debug, Encode)]
pub struct Request {
    pub kind: RequestKind,
    pub signer_id: ContextIdentity,
    pub user_id: ContextIdentity,
    pub nonce: u64,
}

#[derive(Debug, Encode, Decode, Copy, Clone)]
pub enum Capability {
    ManageApplication,
    ManageMembers,
    ProxyCode,
}

impl From<&crate::Capability> for Capability {
    fn from(value: &crate::Capability) -> Self {
        match value {
            crate::Capability::ManageApplication => Capability::ManageApplication,
            crate::Capability::ManageMembers => Capability::ManageMembers,
            crate::Capability::Proxy => Capability::ProxyCode,
        }
    }
}

#[derive(Debug, Encode)]
pub struct CapabilityAssignment {
    pub member: ContextIdentity,
    pub capability: Capability,
}

#[derive(Debug, Encode)]
pub enum ContextRequestKind {
    Add(ContextIdentity, Application),
    UpdateApplication(Application),
    AddMembers(Vec<ContextIdentity>),
    RemoveMembers(Vec<ContextIdentity>),
    Grant(Vec<CapabilityAssignment>),
    Revoke(Vec<CapabilityAssignment>),
    UpdateProxyContract,
}

impl From<crate::ContextRequestKind<'_>> for ContextRequestKind {
    fn from(value: crate::ContextRequestKind<'_>) -> Self {
        match value {
            crate::ContextRequestKind::Add {
                author_id,
                application,
            } => ContextRequestKind::Add(author_id.into_inner().into(), application.into()),
            crate::ContextRequestKind::UpdateApplication { application } => {
                ContextRequestKind::UpdateApplication(application.into())
            }
            crate::ContextRequestKind::AddMembers { members } => ContextRequestKind::AddMembers(
                members.into_iter().map(|m| m.into_inner().into()).collect(),
            ),
            crate::ContextRequestKind::RemoveMembers { members } => {
                ContextRequestKind::RemoveMembers(
                    members.into_iter().map(|m| m.into_inner().into()).collect(),
                )
            }
            crate::ContextRequestKind::Grant { capabilities } => ContextRequestKind::Grant(
                capabilities
                    .into_iter()
                    .map(|(id, cap)| CapabilityAssignment {
                        member: id.into_inner().into(),
                        capability: cap.into(),
                    })
                    .collect(),
            ),
            crate::ContextRequestKind::Revoke { capabilities } => ContextRequestKind::Revoke(
                capabilities
                    .into_iter()
                    .map(|(id, cap)| CapabilityAssignment {
                        member: id.into_inner().into(),
                        capability: cap.into(),
                    })
                    .collect(),
            ),
            crate::ContextRequestKind::UpdateProxyContract => {
                ContextRequestKind::UpdateProxyContract
            }
        }
    }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Application {
    pub id: ApplicationId,
    pub blob: ApplicationBlob,
    pub size: u64,
    pub source: EncodableString,
    pub metadata: EncodableString,
}

impl From<crate::Application<'_>> for Application {
    fn from(value: crate::Application<'_>) -> Self {
        Application {
            id: (*value.id).into(),
            blob: (*value.blob).into(),
            size: value.size,
            source: value.source.into(),
            metadata: value.metadata.into(),
        }
    }
}

impl<'a> From<Application> for crate::Application<'a> {
    fn from(value: Application) -> Self {
        crate::Application {
            id: Repr::new(value.id.into()),
            blob: Repr::new(value.blob.into()),
            size: value.size,
            source: value.source.into(),
            metadata: value.metadata.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EncodableString(pub String);

impl From<crate::types::ApplicationSource<'_>> for EncodableString {
    fn from(value: crate::types::ApplicationSource<'_>) -> Self {
        EncodableString(value.0.into_owned())
    }
}

impl From<crate::types::ApplicationMetadata<'_>> for EncodableString {
    fn from(value: crate::types::ApplicationMetadata<'_>) -> Self {
        EncodableString(String::from_utf8_lossy(&value.0.into_inner()).into_owned())
    }
}

impl From<&str> for EncodableString {
    fn from(value: &str) -> Self {
        EncodableString(value.to_owned())
    }
}

impl Encode for EncodableString {
    fn encode<W: FeltWriter>(&self, writer: &mut W) -> Result<(), Error> {
        const WORD_SIZE: usize = 31;
        let bytes = self.0.as_bytes();

        // Calculate full words and pending word
        #[allow(clippy::integer_division, reason = "Not harmful here")]
        let full_words_count = bytes.len() / WORD_SIZE;
        let pending_len = bytes.len() % WORD_SIZE;

        // Write number of full words
        writer.write(Felt::from(full_words_count));

        // Write full words (31 chars each)
        for i in 0..full_words_count {
            let start = i * WORD_SIZE;
            let word_bytes = &bytes[start..start + WORD_SIZE];
            let word_hex = hex::encode(word_bytes);
            let felt = Felt::from_hex(&format!("0x{}", word_hex))
                .map_err(|e| Error::custom(&format!("Invalid word hex: {}", e)))?;
            writer.write(felt);
        }

        // Write pending word if exists
        if pending_len > 0 {
            let pending_bytes = &bytes[full_words_count * WORD_SIZE..];
            let pending_hex = hex::encode(pending_bytes);
            let felt = Felt::from_hex(&format!("0x{}", pending_hex))
                .map_err(|e| Error::custom(&format!("Invalid pending hex: {}", e)))?;
            writer.write(felt);
        } else {
            writer.write(Felt::ZERO);
        }

        // Write pending word length
        writer.write(Felt::from(pending_len));

        Ok(())
    }
}

impl<'a> Decode<'a> for EncodableString {
    fn decode_iter<T>(iter: &mut T) -> Result<Self, Error>
    where
        T: Iterator<Item = &'a Felt>,
    {
        const WORD_SIZE: usize = 31;

        // Get number of full words
        let first_felt = iter.next().ok_or_else(Error::input_exhausted)?;

        let full_words_count = first_felt.to_bytes_be()[31] as usize;

        let mut bytes = Vec::new();

        // Read full words
        for _ in 0..full_words_count {
            let word = iter.next().ok_or_else(Error::input_exhausted)?;
            let word_bytes = word.to_bytes_be();
            bytes.extend_from_slice(&word_bytes[1..WORD_SIZE + 1]);
        }

        // Read pending word
        let pending_word = iter.next().ok_or_else(Error::input_exhausted)?;
        let pending_bytes = pending_word.to_bytes_be();

        // Read pending length
        let pending_len = iter
            .next()
            .ok_or_else(Error::input_exhausted)?
            .to_bytes_be()[31] as usize;

        // Handle pending bytes - find first non-zero byte and take all remaining bytes
        if pending_len > 0 {
            let start = pending_bytes.iter().position(|&x| x != 0).unwrap_or(1);
            bytes.extend_from_slice(&pending_bytes[start..]);
        }

        let string = String::from_utf8(bytes).map_err(|_| Error::custom("Invalid UTF-8"))?;

        Ok(EncodableString(string))
    }
}

#[derive(Debug, Encode)]
pub struct StarknetMembersRequest {
    pub context_id: ContextId,
    pub offset: u32,
    pub length: u32,
}

impl From<crate::client::env::config::query::members::MembersRequest> for StarknetMembersRequest {
    fn from(value: crate::client::env::config::query::members::MembersRequest) -> Self {
        StarknetMembersRequest {
            context_id: (*value.context_id).into(),
            offset: value.offset as u32,
            length: value.length as u32,
        }
    }
}

#[derive(Debug, Encode)]
pub struct StarknetApplicationRevisionRequest {
    pub context_id: ContextId,
}

impl From<crate::client::env::config::query::application_revision::ApplicationRevisionRequest>
    for StarknetApplicationRevisionRequest
{
    fn from(
        value: crate::client::env::config::query::application_revision::ApplicationRevisionRequest,
    ) -> Self {
        StarknetApplicationRevisionRequest {
            context_id: (*value.context_id).into(),
        }
    }
}

#[derive(Debug, Encode)]
pub struct Signed {
    pub payload: Vec<Felt>,
    pub signature_r: Felt,
    pub signature_s: Felt,
}

// Add reverse conversions for IDs
impl From<ApplicationId> for crate::types::ApplicationId {
    fn from(value: ApplicationId) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

impl From<ContextId> for crate::types::ContextId {
    fn from(value: ContextId) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

impl From<ApplicationBlob> for crate::types::BlobId {
    fn from(value: ApplicationBlob) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

impl<'a> From<EncodableString> for crate::types::ApplicationSource<'a> {
    fn from(value: EncodableString) -> Self {
        crate::types::ApplicationSource(std::borrow::Cow::Owned(value.0))
    }
}

impl<'a> From<EncodableString> for crate::types::ApplicationMetadata<'a> {
    fn from(value: EncodableString) -> Self {
        crate::types::ApplicationMetadata(Repr::new(std::borrow::Cow::Owned(value.0.into_bytes())))
    }
}

#[derive(Debug, Decode)]
pub struct StarknetPrivilegeEntry {
    pub identity: ContextIdentity,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Decode)]
pub struct StarknetPrivileges {
    pub privileges: Vec<StarknetPrivilegeEntry>,
}

impl From<StarknetPrivileges> for BTreeMap<SignerId, Vec<crate::Capability>> {
    fn from(value: StarknetPrivileges) -> Self {
        value
            .privileges
            .into_iter()
            .map(|entry| {
                (
                    entry.identity.into(),
                    entry.capabilities.into_iter().map(Into::into).collect(),
                )
            })
            .collect()
    }
}

// Add conversion from Starknet Capability to domain Capability
impl From<Capability> for crate::Capability {
    fn from(value: Capability) -> Self {
        match value {
            Capability::ManageApplication => crate::Capability::ManageApplication,
            Capability::ManageMembers => crate::Capability::ManageMembers,
            Capability::ProxyCode => crate::Capability::Proxy,
        }
    }
}

// Add conversion from Starknet ContextIdentity to SignerId
impl From<ContextIdentity> for SignerId {
    fn from(value: ContextIdentity) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

#[derive(Debug, Decode)]
pub struct StarknetMembers {
    pub members: Vec<ContextIdentity>,
}

impl From<StarknetMembers> for Vec<crate::types::ContextIdentity> {
    fn from(value: StarknetMembers) -> Self {
        value.members.into_iter().map(|id| id.into()).collect()
    }
}

// Add conversion from Starknet ContextIdentity to domain ContextIdentity
impl From<ContextIdentity> for crate::types::ContextIdentity {
    fn from(value: ContextIdentity) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

// Add From implementation for Repr<ContextId>
impl From<Repr<crate::types::ContextId>> for FeltPair {
    fn from(value: Repr<crate::types::ContextId>) -> Self {
        value.into_inner().into()
    }
}

// Add From implementation for Repr<ContextIdentity>
impl From<Repr<crate::types::ContextIdentity>> for FeltPair {
    fn from(value: Repr<crate::types::ContextIdentity>) -> Self {
        value.into_inner().into()
    }
}
