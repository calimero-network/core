use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_node_primitives::messages::get_blob_bytes::{
    GetBlobBytesRequest, GetBlobBytesResponse,
};
use futures_util::{io, TryStreamExt};

use crate::NodeManager;

impl Handler<GetBlobBytesRequest> for NodeManager {
    type Result = ActorResponse<Self, <GetBlobBytesRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetBlobBytesRequest { blob_id }: GetBlobBytesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let blobstore = self.blobstore.clone();

        let task = async move {
            let Some(blob) = blobstore.get(blob_id)? else {
                return Ok(GetBlobBytesResponse { bytes: None });
            };

            let mut blob = blob.map_err(io::Error::other).into_async_read();
            let mut bytes = Vec::new();
            let _ignored = io::copy(&mut blob, &mut bytes).await?;

            let bytes: Arc<[u8]> = bytes.into();
            Ok(GetBlobBytesResponse { bytes: Some(bytes) })
        };

        ActorResponse::r#async(task.into_actor(self))
    }
}
