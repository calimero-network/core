//! Specialized Node Invitation Protocol Types
//!
//! This module defines the request-response protocol types for specialized node invitation.
//! The protocol allows specialized nodes (e.g., read-only TEE nodes) to receive context
//! invitations after verification.

use std::io;

use async_trait::async_trait;
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;
use futures_util::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::request_response::Codec;
use libp2p::StreamProtocol;

/// Protocol identifier for specialized node invitation request-response
pub const CALIMERO_SPECIALIZED_NODE_INVITE_PROTOCOL: StreamProtocol =
    StreamProtocol::new("/calimero/specialized-node-invite/1.0.0");

/// Maximum size of a specialized node invite message (1MB should be sufficient for attestation + invitation)
pub const MAX_SPECIALIZED_NODE_INVITE_MESSAGE_SIZE: u64 = 1024 * 1024;

/// Type of specialized node being invited
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum SpecializedNodeType {
    /// Read-only node - receives state updates but cannot execute transactions
    ReadOnly,
}

/// Verification request sent by specialized node to inviting node
///
/// After receiving a discovery message via pubsub, the specialized node sends this
/// request containing its verification data and public key.
///
/// Note: context_id is NOT included - the requesting node tracks it internally
/// using the nonce as the lookup key.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum VerificationRequest {
    /// TEE attestation verification
    TeeAttestation {
        /// Nonce from the discovery message (binds attestation to request)
        nonce: [u8; 32],
        /// TDX/TEE attestation quote bytes
        quote_bytes: Vec<u8>,
        /// Specialized node's identity public key for invitation
        public_key: PublicKey,
    },
    // Future variants:
    // HardwareToken { ... },
    // TrustedCertificate { ... },
}

impl VerificationRequest {
    /// Get the nonce from the verification request
    #[must_use]
    pub fn nonce(&self) -> &[u8; 32] {
        match self {
            Self::TeeAttestation { nonce, .. } => nonce,
        }
    }

    /// Get the public key from the verification request
    #[must_use]
    pub fn public_key(&self) -> &PublicKey {
        match self {
            Self::TeeAttestation { public_key, .. } => public_key,
        }
    }
}

/// Response sent by inviting node containing the invitation (or error)
///
/// After verifying the specialized node, the inviting node creates an invitation
/// for the node's public key and sends it back.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct SpecializedNodeInvitationResponse {
    /// The nonce from the original request (for confirmation broadcast)
    pub nonce: [u8; 32],
    /// Serialized ContextInvitationPayload (if verification succeeded)
    /// We use bytes here to avoid circular dependency with context-config crate
    pub invitation_bytes: Option<Vec<u8>>,
    /// Error message if verification failed
    pub error: Option<String>,
}

impl SpecializedNodeInvitationResponse {
    /// Create a successful response with an invitation
    #[must_use]
    pub fn success(nonce: [u8; 32], invitation_bytes: Vec<u8>) -> Self {
        Self {
            nonce,
            invitation_bytes: Some(invitation_bytes),
            error: None,
        }
    }

    /// Create an error response
    #[must_use]
    pub fn error(nonce: [u8; 32], message: impl Into<String>) -> Self {
        Self {
            nonce,
            invitation_bytes: None,
            error: Some(message.into()),
        }
    }
}

/// Codec for specialized node invite request-response protocol
#[derive(Debug, Clone, Default)]
pub struct SpecializedNodeInviteCodec;

#[async_trait]
impl Codec for SpecializedNodeInviteCodec {
    type Protocol = StreamProtocol;
    type Request = VerificationRequest;
    type Response = SpecializedNodeInvitationResponse;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        // Read length prefix (4 bytes, big endian)
        let mut len_buf = [0u8; 4];
        io.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        if len as u64 > MAX_SPECIALIZED_NODE_INVITE_MESSAGE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request too large",
            ));
        }

        // Read the message bytes
        let mut buf = vec![0u8; len];
        io.read_exact(&mut buf).await?;

        // Deserialize
        borsh::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        // Read length prefix (4 bytes, big endian)
        let mut len_buf = [0u8; 4];
        io.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        if len as u64 > MAX_SPECIALIZED_NODE_INVITE_MESSAGE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response too large",
            ));
        }

        // Read the message bytes
        let mut buf = vec![0u8; len];
        io.read_exact(&mut buf).await?;

        // Deserialize
        borsh::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        // Serialize
        let buf = borsh::to_vec(&req).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Write length prefix
        let len = buf.len() as u32;
        io.write_all(&len.to_be_bytes()).await?;

        // Write message
        io.write_all(&buf).await?;
        io.flush().await?;

        Ok(())
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        // Serialize
        let buf = borsh::to_vec(&res).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Write length prefix
        let len = buf.len() as u32;
        io.write_all(&len.to_be_bytes()).await?;

        // Write message
        io.write_all(&buf).await?;
        io.flush().await?;

        Ok(())
    }
}
