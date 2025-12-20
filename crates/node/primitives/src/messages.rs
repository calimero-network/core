use actix::Message;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tokio::sync::oneshot;

pub mod get_blob_bytes;

use get_blob_bytes::GetBlobBytesRequest;

/// Request to register a pending specialized node invite in the node's state.
#[derive(Clone, Debug)]
pub struct RegisterPendingSpecializedNodeInvite {
    /// The nonce from the specialized node invite broadcast
    pub nonce: [u8; 32],
    /// The context to invite specialized nodes to
    pub context_id: ContextId,
    /// The identity performing the invitation
    pub inviter_id: PublicKey,
}

/// Request to remove a pending specialized node invite from the node's state.
/// Used to clean up if broadcast fails after registration.
#[derive(Clone, Debug)]
pub struct RemovePendingSpecializedNodeInvite {
    /// The nonce to remove
    pub nonce: [u8; 32],
}

#[derive(Debug, Message)]
#[rtype("()")]
pub enum NodeMessage {
    GetBlobBytes {
        request: GetBlobBytesRequest,
        outcome: oneshot::Sender<<GetBlobBytesRequest as Message>::Result>,
    },
    RegisterPendingSpecializedNodeInvite {
        request: RegisterPendingSpecializedNodeInvite,
    },
    RemovePendingSpecializedNodeInvite {
        request: RemovePendingSpecializedNodeInvite,
    },
}
