use core::error::Error;

use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_primitives::messages::delete_context::{
    DeleteContextRequest, DeleteContextResponse,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_store::key::Key;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::{key, Store};
use either::Either;
use eyre::bail;

use crate::ContextManager;

impl Handler<DeleteContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <DeleteContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        DeleteContextRequest { context_id }: DeleteContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let guard = self.get_context_exclusive(&context_id);

        if guard.is_none() {
            match self.context_client.has_context(&context_id) {
                Ok(true) => {}
                Ok(false) => {
                    return ActorResponse::reply(Ok(DeleteContextResponse { deleted: false }))
                }
                Err(err) => return ActorResponse::reply(Err(err)),
            }
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();

        let task = async move {
            let _guard = match guard {
                Some(Either::Left(guard)) => Some(guard),
                Some(Either::Right(task)) => Some(task.await),
                None => None,
            };

            delete_context(datastore, node_client, context_id).await?;

            Ok(DeleteContextResponse { deleted: true })
        };

        ActorResponse::r#async(task.into_actor(self).map_ok(move |res, act, _ctx| {
            let _ignored = act.contexts.remove(&context_id);

            res
        }))
    }
}

async fn delete_context(
    datastore: Store,
    node_client: NodeClient,
    context_id: ContextId,
) -> eyre::Result<()> {
    node_client.unsubscribe(&context_id).await?;

    let mut handle = datastore.handle();

    let key = key::ContextMeta::new(context_id);

    handle.delete(&key)?;
    handle.delete(&key::ContextConfig::new(context_id))?;

    // fixme! store.handle() is prolematic here for lifetime reasons
    let mut datastore = handle.into_inner();

    delete_context_scoped::<key::ContextIdentity, 32>(&mut datastore, &context_id, [0; 32], None)?;

    delete_context_scoped::<key::ContextState, 32>(&mut datastore, &context_id, [0; 32], None)?;

    Ok(())
}

#[expect(clippy::unwrap_in_result, reason = "pre-validated")]
fn delete_context_scoped<K, const N: usize>(
    datastore: &mut Store,
    context_id: &ContextId,
    offset: [u8; N],
    end: Option<[u8; N]>,
) -> eyre::Result<()>
where
    K: key::FromKeyParts<Error: Error + Send + Sync>,
{
    let expected_length = Key::<K::Components>::len();

    if context_id.len().saturating_add(N) != expected_length {
        bail!(
            "key length mismatch, expected: {}, got: {}",
            Key::<K::Components>::len(),
            N
        )
    }

    let mut keys = vec![];

    let mut key = context_id.to_vec();

    let end = end
        .map(|end| {
            key.extend_from_slice(&end);

            let end = Key::<K::Components>::try_from_slice(&key).expect("length pre-matched");

            K::try_from_parts(end)
        })
        .transpose()?;

    'outer: loop {
        key.truncate(context_id.len());
        key.extend_from_slice(&offset);

        let offset = Key::<K::Components>::try_from_slice(&key).expect("length pre-matched");

        let mut iter = datastore.iter()?;

        let first = iter.seek(K::try_from_parts(offset)?).transpose();

        if first.is_none() {
            break;
        }

        for k in first.into_iter().chain(iter.keys()) {
            let k = k?;

            let key = k.as_key();

            if let Some(end) = end {
                if key == end.as_key() {
                    break 'outer;
                }
            }

            if !key.as_bytes().starts_with(&**context_id) {
                break 'outer;
            }

            keys.push(k);

            if keys.len() == 100 {
                break;
            }
        }

        drop(iter);

        #[expect(clippy::iter_with_drain, reason = "reallocation would be a bad idea")]
        for k in keys.drain(..) {
            datastore.delete(&k)?;
        }
    }

    for k in keys {
        datastore.delete(&k)?;
    }

    Ok(())
}
