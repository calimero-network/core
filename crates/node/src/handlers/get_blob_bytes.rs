use std::sync::Arc;

use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
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
        if let bytes @ Some(_) = self.blob_cache.get(&blob_id).cloned() {
            return ActorResponse::reply(Ok(GetBlobBytesResponse { bytes }));
        }

        let maybe_blob = match self.blobstore.get(blob_id) {
            Ok(res) => res,
            Err(err) => return ActorResponse::reply(Err(err.into())),
        };

        let Some(blob) = maybe_blob else {
            return ActorResponse::reply(Ok(GetBlobBytesResponse { bytes: None }));
        };

        let fut = Box::pin(async {
            let mut blob = blob.map_err(io::Error::other).into_async_read();

            let mut bytes = Vec::new();

            let _ignored = io::copy(&mut blob, &mut bytes).await?;

            Ok(bytes.into())
        });

        ActorResponse::r#async(fut.into_actor(self).map_ok(move |bytes, act, _ctx| {
            // blob bytes are content-addressed, so if it previously existed, it will be the same thing making it
            // safe to discard the new one in favor of the cached one. Though this should only happen if some other
            // task was already in progress of reading the blob while this request was made which already resolved
            let bytes = act.blob_cache.entry(blob_id).or_insert(bytes).clone();

            GetBlobBytesResponse { bytes: Some(bytes) }
        }))
    }
}
