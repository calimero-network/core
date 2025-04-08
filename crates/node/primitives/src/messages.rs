use actix::Message;
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
}
