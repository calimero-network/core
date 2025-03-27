use alloy::primitives::{Bytes, FixedBytes, B256};
use alloy::sol;
use alloy_sol_types::SolValue;

use crate::repr::{Repr, ReprBytes, ReprTransmute};
use crate::types::{Application, ApplicationMetadata, ApplicationSource, Capability};
use crate::{ContextRequest, ContextRequestKind, RequestKind};

sol! {
  struct SolApplication {
      bytes32 id;
      bytes32 blob;
      uint64 size;
      string source;
      bytes metadata;
  }

  struct SolUserCapabilities {
      bytes32 userId;
      SolCapability[] capabilities;
  }

  enum SolContextRequestKind {
      Add,
      AddMembers,
      RemoveMembers,
      AddCapability,
      RevokeCapability,
      UpdateApplication,
      UpdateProxyContract
  }

  enum SolRequestKind {
      Context
  }

  enum SolCapability {
      ManageApplication,
      ManageMembers,
      Proxy
  }

  struct SolContextRequest {
      bytes32 contextId;
      SolContextRequestKind kind;
      bytes data;
  }

  struct SolRequest {
      bytes32 signerId;
      bytes32 userId;
      uint64 nonce;
      SolRequestKind kind;
      bytes data;
  }

  struct SolSignedRequest {
      SolRequest payload;
      bytes32 r;
      bytes32 s;
      uint8 v;
  }
}
pub trait ToSol<T> {
    fn to_sol(&self) -> T;
}

// Implementation for converting Calimero Application to SolApplication
impl<'a> ToSol<SolApplication> for Application<'a> {
    fn to_sol(&self) -> SolApplication {
        let id: [u8; 32] = self.id.rt().expect("infallible conversion");
        let id_sol = B256::from_slice(&id);
        let blob: [u8; 32] = self.blob.rt().expect("infallible conversion");
        let blob_sol = B256::from_slice(&blob);
        let source = self.source.0.to_string();
        let metadata = self.metadata.0.to_owned().to_vec();
        SolApplication {
            id: id_sol,
            blob: blob_sol,
            size: self.size,
            source,
            metadata: Bytes::from(metadata),
        }
    }
}

impl From<SolApplication> for Application<'static> {
    fn from(sol_app: SolApplication) -> Self {
        Self {
            id: sol_app.id.rt().expect("infallible conversion"),
            blob: sol_app.blob.rt().expect("infallible conversion"),
            size: sol_app.size,
            source: ApplicationSource(std::borrow::Cow::Owned(sol_app.source)),
            metadata: ApplicationMetadata(Repr::new(std::borrow::Cow::Owned(
                sol_app.metadata.to_vec(),
            ))),
        }
    }
}

// Implementation for converting Calimero Capability to SolCapability
impl ToSol<SolCapability> for Capability {
    fn to_sol(&self) -> SolCapability {
        match self {
            Capability::ManageApplication => SolCapability::ManageApplication,
            Capability::ManageMembers => SolCapability::ManageMembers,
            Capability::Proxy => SolCapability::Proxy,
        }
    }
}

// Implementation for converting Calimero ContextRequestKind to SolContextRequestKind
impl<'a> ToSol<SolContextRequestKind> for ContextRequestKind<'a> {
    fn to_sol(&self) -> SolContextRequestKind {
        match self {
            ContextRequestKind::Add { .. } => SolContextRequestKind::Add,
            ContextRequestKind::AddMembers { .. } => SolContextRequestKind::AddMembers,
            ContextRequestKind::RemoveMembers { .. } => SolContextRequestKind::RemoveMembers,
            ContextRequestKind::Grant { .. } => SolContextRequestKind::AddCapability,
            ContextRequestKind::Revoke { .. } => SolContextRequestKind::RevokeCapability,
            ContextRequestKind::UpdateApplication { .. } => {
                SolContextRequestKind::UpdateApplication
            }
            ContextRequestKind::UpdateProxyContract { .. } => {
                SolContextRequestKind::UpdateProxyContract
            }
        }
    }
}

// Implementation for converting Calimero ContextRequest to SolContextRequest
impl<'a> ToSol<SolContextRequest> for ContextRequest<'a> {
    fn to_sol(&self) -> SolContextRequest {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        let data = encode_context_request_data(&self.kind);

        SolContextRequest {
            contextId: B256::from_slice(&context_id),
            kind: self.kind.to_sol(),
            data: Bytes::from(data),
        }
    }
}

// Helper function to encode the data field based on the ContextRequestKind
fn encode_context_request_data<'a>(kind: &ContextRequestKind<'a>) -> Vec<u8> {
    match kind {
        ContextRequestKind::Add {
            author_id,
            application,
        } => {
            // For Add, we need to encode (bytes32 authorId, Application application)
            // First, convert the application to SolApplication
            let sol_app = application.to_sol();

            let author_id: [u8; 32] = author_id.rt().expect("infallible conversion");
            let author_id_sol = B256::from_slice(&author_id);

            let data_encode = (author_id_sol, sol_app).abi_encode();
            data_encode[32..].to_vec()
        }
        ContextRequestKind::AddMembers { members } => {
            // For AddMembers, we need to encode bytes32[] members
            // Convert the members to a Vec of FixedBytes
            let sol_members: Vec<FixedBytes<32>> = members
                .iter()
                .map(|m| FixedBytes::from(m.as_bytes()))
                .collect();

            // Encode the members array
            sol_members.abi_encode()
        }
        ContextRequestKind::RemoveMembers { members } => {
            // For RemoveMembers, we need to encode bytes32[] members
            // Convert the members to a Vec of FixedBytes
            let sol_members: Vec<FixedBytes<32>> = members
                .iter()
                .map(|m| FixedBytes::from(m.as_bytes()))
                .collect();

            // Encode the members array
            sol_members.abi_encode()
        }
        ContextRequestKind::Grant { capabilities } => {
            // For Grant, we need to encode the list of (member, capability) pairs
            // This depends on how your contract expects the data

            // If your contract expects an array of members and an array of capabilities:
            let members: Vec<FixedBytes<32>> = capabilities
                .iter()
                .map(|(member, _)| FixedBytes::from(member.as_bytes()))
                .collect();

            let capability_values: Vec<u8> = capabilities
                .iter()
                .map(|(_, capability)| capability.to_sol() as u8)
                .collect();

            // Encode as (bytes32[] members, uint8[] capabilities)
            // This requires manual ABI encoding

            // First, encode each array separately
            let encoded_members = members.abi_encode();
            let encoded_capabilities = capability_values.abi_encode();

            // Now construct the tuple encoding
            let mut data = Vec::new();

            // Add offset to first array (32 bytes)
            let mut offset1_bytes = [0u8; 32];
            offset1_bytes[31] = 64; // Offset is 64 bytes (2 * 32)
            data.extend_from_slice(&offset1_bytes);

            // Add offset to second array (32 bytes)
            let mut offset2_bytes = [0u8; 32];
            // The offset to the second array is 64 (for the two offsets) plus the length of the first array
            let offset2 = 64 + encoded_members.len();
            // We need to encode this as a 32-byte big-endian integer
            for i in 0..4 {
                // Assuming the length won't exceed 2^32
                offset2_bytes[28 + i] = ((offset2 >> (8 * (3 - i))) & 0xFF) as u8;
            }
            data.extend_from_slice(&offset2_bytes);

            // Add encoded arrays
            data.extend_from_slice(&encoded_members);
            data.extend_from_slice(&encoded_capabilities);

            data
        }
        ContextRequestKind::Revoke { capabilities } => {
            // Same encoding as Grant
            // For Revoke, we need to encode the list of (member, capability) pairs
            // This depends on how your contract expects the data

            // If your contract expects an array of members and an array of capabilities:
            let members: Vec<FixedBytes<32>> = capabilities
                .iter()
                .map(|(member, _)| FixedBytes::from(member.as_bytes()))
                .collect();

            let capability_values: Vec<u8> = capabilities
                .iter()
                .map(|(_, capability)| capability.to_sol() as u8)
                .collect();

            // Encode as (bytes32[] members, uint8[] capabilities)
            // This requires manual ABI encoding

            // First, encode each array separately
            let encoded_members = members.abi_encode();
            let encoded_capabilities = capability_values.abi_encode();

            // Now construct the tuple encoding
            let mut data = Vec::new();

            // Add offset to first array (32 bytes)
            let mut offset1_bytes = [0u8; 32];
            offset1_bytes[31] = 64; // Offset is 64 bytes (2 * 32)
            data.extend_from_slice(&offset1_bytes);

            // Add offset to second array (32 bytes)
            let mut offset2_bytes = [0u8; 32];
            // The offset to the second array is 64 (for the two offsets) plus the length of the first array
            let offset2 = 64 + encoded_members.len();
            // We need to encode this as a 32-byte big-endian integer
            for i in 0..4 {
                // Assuming the length won't exceed 2^32
                offset2_bytes[28 + i] = ((offset2 >> (8 * (3 - i))) & 0xFF) as u8;
            }
            data.extend_from_slice(&offset2_bytes);

            // Add encoded arrays
            data.extend_from_slice(&encoded_members);
            data.extend_from_slice(&encoded_capabilities);

            data
        }
        ContextRequestKind::UpdateApplication { application } => {
            let sol_app = application.to_sol();
            sol_app.abi_encode()
        }
        ContextRequestKind::UpdateProxyContract => Vec::new(),
    }
}

// Implementation for converting Calimero RequestKind to SolRequestKind
impl<'a> ToSol<SolRequestKind> for RequestKind<'a> {
    fn to_sol(&self) -> SolRequestKind {
        match self {
            RequestKind::Context(_) => SolRequestKind::Context,
            // Add other variants if they exist in the future
        }
    }
}
