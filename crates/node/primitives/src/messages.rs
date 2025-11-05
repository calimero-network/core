use actix::Message;
use calimero_primitives::context::ContextId;
use tokio::sync::oneshot;

pub mod get_blob_bytes;

use get_blob_bytes::GetBlobBytesRequest;

#[derive(Debug, Message)]
#[rtype("()")]
pub enum NodeMessage {
    GetBlobBytes {
        request: GetBlobBytesRequest,
        outcome: oneshot::Sender<<GetBlobBytesRequest as Message>::Result>,
    },
    AddLocalDelta {
        context_id: ContextId,
        delta_id: [u8; 32],
        parents: Vec<[u8; 32]>,
        actions: Vec<calimero_storage::interface::Action>,
        hlc: calimero_storage::logical_clock::HybridTimestamp,
        expected_root_hash: [u8; 32],
    },
}
