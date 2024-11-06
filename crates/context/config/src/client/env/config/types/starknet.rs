use hex;
use starknet::core::codec::{Encode, Error, FeltWriter};
use starknet::core::types::Felt;

use crate::repr::{Repr, ReprBytes};
use crate::types::SignerId;

#[derive(Debug, Clone, Encode)]
pub struct ContextId {
    pub high: Felt,
    pub low: Felt,
}

impl From<crate::ContextId> for ContextId {
    fn from(value: crate::ContextId) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ContextId {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

// Add this implementation for Repr wrapped type
impl From<Repr<crate::ContextId>> for ContextId {
    fn from(value: Repr<crate::ContextId>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ContextId {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

// Context Member ID
#[derive(Debug, Clone, Encode)]
pub struct ContextIdentity {
    pub high: Felt,
    pub low: Felt,
}

impl From<crate::ContextIdentity> for ContextIdentity {
    fn from(value: crate::ContextIdentity) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ContextIdentity {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

// Add this implementation for Repr wrapped type
impl From<Repr<crate::ContextIdentity>> for ContextIdentity {
    fn from(value: Repr<crate::ContextIdentity>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ContextIdentity {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<SignerId> for ContextIdentity {
    fn from(value: SignerId) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ContextIdentity {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<Repr<SignerId>> for ContextIdentity {
    fn from(value: Repr<SignerId>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ContextIdentity {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

#[derive(Debug, Encode)]
pub enum RequestKind {
    Context(ContextRequest),
}

impl From<crate::RequestKind<'_>> for RequestKind {
    fn from(value: crate::RequestKind<'_>) -> Self {
        match value {
            crate::RequestKind::Context(ctx_req) => RequestKind::Context(ctx_req.to_owned().into()),
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
            context_id: value.context_id.into(),
            kind: value.kind.to_owned().into(),
        }
    }
}

#[derive(Debug, Encode)]
pub struct Signed {
    pub payload: Vec<Felt>,
    pub signature_r: Felt,
    pub signature_s: Felt,
}

impl<T> From<crate::types::Signed<T>> for Signed {
    fn from(value: crate::types::Signed<T>) -> Self {
        // Convert the payload bytes to Felts
        let payload = value
            .payload
            .into_inner()
            .chunks_exact(32)
            .map(|chunk| {
                let chunk_array: [u8; 32] = chunk.try_into().expect("chunk should be 32 bytes");
                Felt::from_bytes_be(&chunk_array)
            })
            .collect();
        // Extract r and s from the signature
        let sig_bytes = value.signature.as_bytes();
        let (r_bytes, s_bytes) = sig_bytes.split_at(32);
        // Convert slices to fixed arrays
        let r_array: [u8; 32] = r_bytes.try_into().expect("r should be 32 bytes");
        let s_array: [u8; 32] = s_bytes.try_into().expect("s should be 32 bytes");

        Signed {
            payload,
            signature_r: Felt::from_bytes_be(&r_array),
            signature_s: Felt::from_bytes_be(&s_array),
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

impl From<crate::Request<'_>> for Request {
    fn from(value: crate::Request<'_>) -> Self {
        Request {
            kind: value.kind.into(),
            signer_id: value.signer_id.into(),
            user_id: ContextIdentity {
                high: Felt::ZERO,
                low: Felt::ZERO,
            },
            nonce: 0,
        }
    }
}

#[derive(Debug, Encode)]
pub enum Capability {
    ManageApplication,
    ManageMembers,
}

impl From<&crate::Capability> for Capability {
    fn from(value: &crate::Capability) -> Self {
        match value {
            crate::Capability::ManageApplication => Capability::ManageApplication,
            crate::Capability::ManageMembers => Capability::ManageMembers,
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
}

impl From<crate::ContextRequestKind<'_>> for ContextRequestKind {
    fn from(value: crate::ContextRequestKind<'_>) -> Self {
        match value {
            crate::ContextRequestKind::Add {
                author_id,
                application,
            } => ContextRequestKind::Add(author_id.into(), application.into()),
            crate::ContextRequestKind::UpdateApplication { application } => {
                ContextRequestKind::UpdateApplication(application.into())
            }
            crate::ContextRequestKind::AddMembers { members } => {
                ContextRequestKind::AddMembers(members.into_iter().map(|m| m.into()).collect())
            }
            crate::ContextRequestKind::RemoveMembers { members } => {
                ContextRequestKind::RemoveMembers(members.into_iter().map(|m| m.into()).collect())
            }
            crate::ContextRequestKind::Grant { capabilities } => ContextRequestKind::Grant(
                capabilities
                    .into_iter()
                    .map(|(id, cap)| CapabilityAssignment {
                        member: id.into(),
                        capability: cap.into(),
                    })
                    .collect(),
            ),
            crate::ContextRequestKind::Revoke { capabilities } => ContextRequestKind::Revoke(
                capabilities
                    .into_iter()
                    .map(|(id, cap)| CapabilityAssignment {
                        member: id.into(),
                        capability: cap.into(),
                    })
                    .collect(),
            ),
        }
    }
}

#[derive(Debug, Clone, Encode)]
pub struct ApplicationId {
    pub high: Felt,
    pub low: Felt,
}

#[derive(Debug, Clone, Encode)]
pub struct ApplicationBlob {
    pub high: Felt,
    pub low: Felt,
}

#[derive(Debug, Clone, Encode)]
pub struct Application {
    pub id: ApplicationId,
    pub blob: ApplicationBlob,
    pub size: u64,
    pub source: EncodableString,
    pub metadata: EncodableString,
}

impl From<crate::Application<'_>> for Application {
    fn from(value: crate::Application<'_>) -> Self {
        let id_bytes = value.id.as_bytes();
        let (id_high, id_low) = id_bytes.split_at(id_bytes.len() / 2);

        let blob_bytes = value.blob.as_bytes();
        let (blob_high, blob_low) = blob_bytes.split_at(blob_bytes.len() / 2);

        Application {
            id: ApplicationId {
                high: Felt::from_bytes_be_slice(id_high),
                low: Felt::from_bytes_be_slice(id_low),
            },
            blob: ApplicationBlob {
                high: Felt::from_bytes_be_slice(blob_high),
                low: Felt::from_bytes_be_slice(blob_low),
            },
            size: value.size,
            source: value.source.into(),
            metadata: value.metadata.into(),
        }
    }
}

impl From<crate::types::ApplicationId> for ApplicationId {
    fn from(value: crate::types::ApplicationId) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ApplicationId {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<Repr<crate::types::ApplicationId>> for ApplicationId {
    fn from(value: Repr<crate::types::ApplicationId>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ApplicationId {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<&Repr<crate::types::ApplicationId>> for ApplicationId {
    fn from(value: &Repr<crate::types::ApplicationId>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ApplicationId {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

// Similar implementations for ApplicationBlob
impl From<crate::types::BlobId> for ApplicationBlob {
    fn from(value: crate::types::BlobId) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ApplicationBlob {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<Repr<crate::types::BlobId>> for ApplicationBlob {
    fn from(value: Repr<crate::types::BlobId>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ApplicationBlob {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<&Repr<crate::types::BlobId>> for ApplicationBlob {
    fn from(value: &Repr<crate::types::BlobId>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ApplicationBlob {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
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

// Optional: Add this if you need to convert from string references
impl From<&str> for EncodableString {
    fn from(value: &str) -> Self {
        EncodableString(value.to_string())
    }
}

impl Encode for EncodableString {
    fn encode<W: FeltWriter>(&self, writer: &mut W) -> Result<(), Error> {
        const WORD_SIZE: usize = 31;
        let bytes = self.0.as_bytes();

        // Calculate full words and pending word
        let full_words_count = bytes.len() / WORD_SIZE;
        let pending_len = bytes.len() % WORD_SIZE;

        // Write number of full words
        writer.write(Felt::from(full_words_count));

        // Write full words (31 chars each)
        for i in 0..full_words_count {
            let start = i * WORD_SIZE;
            let word_bytes = &bytes[start..start + WORD_SIZE];
            let word_hex = hex::encode(word_bytes);
            writer.write(Felt::from_hex(&format!("0x{}", word_hex)).unwrap());
        }

        // Write pending word if exists
        if pending_len > 0 {
            let pending_bytes = &bytes[full_words_count * WORD_SIZE..];
            let pending_hex = hex::encode(pending_bytes);
            writer.write(Felt::from_hex(&format!("0x{}", pending_hex)).unwrap());
        } else {
            writer.write(Felt::ZERO);
        }

        // Write pending word length
        writer.write(Felt::from(pending_len));

        Ok(())
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
            context_id: value.context_id.into(),
            offset: value.offset as u32,
            length: value.length as u32,
        }
    }
}

// Add these implementations for reference types
impl From<&Repr<crate::ContextIdentity>> for ContextIdentity {
    fn from(value: &Repr<crate::ContextIdentity>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ContextIdentity {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<&Repr<SignerId>> for ContextIdentity {
    fn from(value: &Repr<SignerId>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ContextIdentity {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<&Repr<crate::ContextId>> for ContextId {
    fn from(value: &Repr<crate::ContextId>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        ContextId {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

// Add this new struct
#[derive(Debug, Encode)]
pub struct StarknetApplicationRevisionRequest {
    pub context_id: ContextId,
}

// Add From implementation
impl From<crate::client::env::config::query::application_revision::ApplicationRevisionRequest>
    for StarknetApplicationRevisionRequest
{
    fn from(
        value: crate::client::env::config::query::application_revision::ApplicationRevisionRequest,
    ) -> Self {
        StarknetApplicationRevisionRequest {
            context_id: value.context_id.into(),
        }
    }
}
