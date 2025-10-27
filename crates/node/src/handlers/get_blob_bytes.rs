//! GetBlobBytes handler for retrieving blob data.
//!
//! **Purpose**: Handles requests to retrieve blob bytes, with caching.
//! **Caching**: Uses DashMap with 5-minute TTL for performance.

use actix::{ActorFutureExt, ActorResponse, Handler, Message, WrapFuture};
use calimero_node_primitives::messages::get_blob_bytes::{
    GetBlobBytesRequest, GetBlobBytesResponse,
};
use futures_util::{io, TryStreamExt};

use crate::{CachedBlob, NodeManager};

impl Handler<GetBlobBytesRequest> for NodeManager {
    type Result = ActorResponse<Self, <GetBlobBytesRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetBlobBytesRequest { blob_id }: GetBlobBytesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Check cache first
        if let Some(mut cached) = self.state.blob_cache.get_mut(&blob_id) {
            cached.touch(); // Update last_accessed
            return ActorResponse::reply(Ok(GetBlobBytesResponse {
                bytes: Some(cached.data.clone()),
            }));
        }

        // Not in cache, load from blobstore
        let blobstore = self.managers.blobstore.clone();
        let blob_cache = self.state.blob_cache.clone();

        let task = async move {
            let Some(blob) = blobstore.get(blob_id)? else {
                return Ok(GetBlobBytesResponse { bytes: None });
            };

            let mut blob = blob.map_err(io::Error::other).into_async_read();

            let mut bytes = Vec::new();
            let _ignored = io::copy(&mut blob, &mut bytes).await?;

            let data: std::sync::Arc<[u8]> = bytes.into();

            // Cache the blob
            let _previous = blob_cache.insert(blob_id, CachedBlob::new(data.clone()));

            Ok(GetBlobBytesResponse { bytes: Some(data) })
        };

        ActorResponse::r#async(task.into_actor(self).map(move |res, act, _ctx| {
            if let Err(_) = &res {
                // On error, remove from cache if it was added
                let _ignored = act.state.blob_cache.remove(&blob_id);
            }
            res
        }))
    }
}
