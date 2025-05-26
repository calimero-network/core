use std::sync::Arc;

use actix::Message;
use calimero_primitives::blobs::BlobId;

#[derive(Copy, Clone, Debug)]
pub struct GetBlobBytesRequest {
    pub blob_id: BlobId,
}

impl Message for GetBlobBytesRequest {
    type Result = eyre::Result<GetBlobBytesResponse>;
}

#[derive(Clone, Debug)]
pub struct GetBlobBytesResponse {
    pub bytes: Option<Arc<[u8]>>,
}
