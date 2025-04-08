use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_node_primitives::messages::get_blob_bytes::{
    GetBlobBytesRequest, GetBlobBytesResponse,
};
use either::Either;
use futures_util::{io, TryStreamExt};

use crate::NodeManager;

impl Handler<GetBlobBytesRequest> for NodeManager {
    type Result = ActorResponse<Self, <GetBlobBytesRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetBlobBytesRequest { blob_id }: GetBlobBytesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let blob = self.blob_cache.entry(blob_id).or_default();

        let guard = match blob.clone().try_lock_owned() {
            Ok(guard) => {
                if let bytes @ Some(_) = guard.clone() {
                    return ActorResponse::reply(Ok(GetBlobBytesResponse { bytes }));
                }

                Either::Left(guard)
            }
            Err(_) => Either::Right(blob.clone().lock_owned()),
        };

        let blobstore = self.blobstore.clone();

        let task = Box::pin(async move {
            let mut guard = match guard {
                Either::Left(guard) => guard,
                Either::Right(task) => {
                    let guard = task.await;

                    if let Some(bytes) = guard.clone() {
                        return Ok(Some(bytes));
                    }

                    guard
                }
            };

            let Some(blob) = blobstore.get(blob_id)? else {
                return Ok(None);
            };

            let mut blob = blob.map_err(io::Error::other).into_async_read();

            let mut bytes = Vec::new();

            let _ignored = io::copy(&mut blob, &mut bytes).await?;

            *guard = Some(bytes.into());

            Ok(guard.clone())
        });

        ActorResponse::r#async(
            task.into_actor(self)
                .map_ok(move |bytes, _act, _ctx| GetBlobBytesResponse { bytes })
                .map_err(move |err, act, _ctx| {
                    let _ignored = act.blob_cache.remove(&blob_id);

                    err
                }),
        )
    }
}
