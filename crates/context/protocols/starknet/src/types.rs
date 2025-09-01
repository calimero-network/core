use std::borrow::Cow;
use std::collections::BTreeMap;

use starknet::core::codec::{Decode, Encode, Error, FeltWriter};
use starknet::core::types::Felt;

use calimero_context_config_core::repr::{Repr, ReprBytes, ReprTransmute};
use calimero_context_config_core::types::{ApplicationId, BlobId, ContextId, ContextIdentity, SignerId};

// Base type for all Starknet Felt pairs
#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct FeltPair {
    pub high: Felt,
    pub low: Felt,
}

// Newtype pattern following types.rs style
#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct StarknetContextId(FeltPair);

#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct StarknetContextIdentity(FeltPair);

#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct StarknetApplicationId(FeltPair);

#[derive(Debug, Clone, Copy, Encode, Decode)]
pub struct StarknetApplicationBlob(FeltPair);

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
impl IntoFeltPair for ContextId {
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

impl IntoFeltPair for ContextIdentity {
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

impl IntoFeltPair for ApplicationId {
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

impl IntoFeltPair for BlobId {
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
impl From<ContextId> for FeltPair {
    fn from(value: ContextId) -> Self {
        value.into_felt_pair().into()
    }
}

impl From<ContextIdentity> for FeltPair {
    fn from(value: ContextIdentity) -> Self {
        value.into_felt_pair().into()
    }
}

impl From<ApplicationId> for FeltPair {
    fn from(value: ApplicationId) -> Self {
        value.into_felt_pair().into()
    }
}

impl From<BlobId> for FeltPair {
    fn from(value: BlobId) -> Self {
        value.into_felt_pair().into()
    }
}

// Simplify the existing From implementations
impl From<ContextId> for StarknetContextId {
    fn from(value: ContextId) -> Self {
        Self(value.into())
    }
}

impl From<ContextIdentity> for StarknetContextIdentity {
    fn from(value: ContextIdentity) -> Self {
        Self(value.into())
    }
}

impl From<ApplicationId> for StarknetApplicationId {
    fn from(value: ApplicationId) -> Self {
        Self(value.into())
    }
}

impl From<BlobId> for StarknetApplicationBlob {
    fn from(value: BlobId) -> Self {
        Self(value.into())
    }
}

// Add From<SignerId> for StarknetContextIdentity
impl From<SignerId> for StarknetContextIdentity {
    fn from(value: SignerId) -> Self {
        let (high, low) = value.into_felt_pair();
        Self(FeltPair { high, low })
    }
}

// Add From<Repr<ContextIdentity>> for StarknetContextIdentity
impl From<Repr<ContextIdentity>> for StarknetContextIdentity {
    fn from(value: Repr<ContextIdentity>) -> Self {
        let (high, low) = value.into_inner().into_felt_pair();
        Self(FeltPair { high, low })
    }
}

#[derive(Debug, Encode)]
pub enum RequestKind {
    Context(ContextRequest),
}

#[derive(Debug, Encode)]
pub struct ContextRequest {
    pub context_id: StarknetContextId,
    pub kind: ContextRequestKind,
}

#[derive(Debug, Encode)]
pub struct Request {
    pub kind: RequestKind,
    pub signer_id: StarknetContextIdentity,
    pub user_id: StarknetContextIdentity,
    pub nonce: u64,
}

#[derive(Debug, Encode, Decode, Copy, Clone)]
pub enum Capability {
    ManageApplication,
    ManageMembers,
    ProxyCode,
}

#[derive(Debug, Encode)]
pub struct CapabilityAssignment {
    pub member: StarknetContextIdentity,
    pub capability: Capability,
}

#[derive(Debug, Encode)]
pub enum ContextRequestKind {
    Add(StarknetContextIdentity, Application),
    UpdateApplication(Application),
    AddMembers(Vec<StarknetContextIdentity>),
    RemoveMembers(Vec<StarknetContextIdentity>),
    Grant(Vec<CapabilityAssignment>),
    Revoke(Vec<CapabilityAssignment>),
    UpdateProxyContract,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Application {
    pub id: StarknetApplicationId,
    pub blob: StarknetApplicationBlob,
    pub size: u64,
    pub source: EncodableString,
    pub metadata: EncodableString,
}

#[derive(Debug, Clone)]
pub struct EncodableString(pub String);

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
    pub context_id: StarknetContextId,
    pub offset: u32,
    pub length: u32,
}

#[derive(Debug, Encode)]
pub struct StarknetApplicationRevisionRequest {
    pub context_id: StarknetContextId,
}

#[derive(Debug, Decode)]
pub struct StarknetPrivilegeEntry {
    pub identity: StarknetContextIdentity,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Decode)]
pub struct StarknetPrivileges {
    pub privileges: Vec<StarknetPrivilegeEntry>,
}

impl From<StarknetPrivileges> for BTreeMap<SignerId, Vec<calimero_context_config_core::types::Capability>> {
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
impl From<Capability> for calimero_context_config_core::types::Capability {
    fn from(value: Capability) -> Self {
        match value {
            Capability::ManageApplication => calimero_context_config_core::types::Capability::ManageApplication,
            Capability::ManageMembers => calimero_context_config_core::types::Capability::ManageMembers,
            Capability::ProxyCode => calimero_context_config_core::types::Capability::Proxy,
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
impl From<StarknetApplicationId> for ApplicationId {
    fn from(value: StarknetApplicationId) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

impl From<StarknetContextId> for ContextId {
    fn from(value: StarknetContextId) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

impl From<StarknetApplicationBlob> for BlobId {
    fn from(value: StarknetApplicationBlob) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

#[derive(Debug, Decode)]
pub struct StarknetMembers {
    pub members: Vec<StarknetContextIdentity>,
}

// Add conversion from Starknet ContextIdentity to domain ContextIdentity
impl From<StarknetContextIdentity> for ContextIdentity {
    fn from(value: StarknetContextIdentity) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

// Add conversion from Starknet Application to domain Application
impl<'a> From<Application> for calimero_context_config_core::types::Application<'a> {
    fn from(value: Application) -> Self {
        calimero_context_config_core::types::Application::new(
            Repr::new(value.id.into()),
            Repr::new(value.blob.into()),
            value.size,
            value.source.into(),
            value.metadata.into(),
        )
    }
}

// Add conversion from StarknetMembers to Vec<ContextIdentity>
impl From<StarknetMembers> for Vec<ContextIdentity> {
    fn from(value: StarknetMembers) -> Self {
        value.members.into_iter().map(|id| id.into()).collect()
    }
}

// Add conversion from FeltPair to StarknetContextId
impl From<FeltPair> for StarknetContextId {
    fn from(value: FeltPair) -> Self {
        StarknetContextId(value)
    }
}

// Add conversion from EncodableString to ApplicationSource
impl<'a> From<EncodableString> for calimero_context_config_core::types::ApplicationSource<'a> {
    fn from(value: EncodableString) -> Self {
        calimero_context_config_core::types::ApplicationSource(std::borrow::Cow::Owned(value.0))
    }
}

// Add conversion from EncodableString to ApplicationMetadata
impl<'a> From<EncodableString> for calimero_context_config_core::types::ApplicationMetadata<'a> {
    fn from(value: EncodableString) -> Self {
        calimero_context_config_core::types::ApplicationMetadata(Repr::new(std::borrow::Cow::Owned(value.0.into_bytes())))
    }
}

// Add conversion from Starknet ContextIdentity to SignerId
impl From<StarknetContextIdentity> for SignerId {
    fn from(value: StarknetContextIdentity) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

// Add From implementation for Repr<ContextId>
impl From<Repr<ContextId>> for FeltPair {
    fn from(value: Repr<ContextId>) -> Self {
        value.into_inner().into()
    }
}

// Add From implementation for Repr<ContextIdentity>
impl From<Repr<ContextIdentity>> for FeltPair {
    fn from(value: Repr<ContextIdentity>) -> Self {
        value.into_inner().into()
    }
}
