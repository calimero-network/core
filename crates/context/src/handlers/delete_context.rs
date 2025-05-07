use core::error::Error;

use actix::fut::wrap_future;
use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::messages::delete_context::{
    DeleteContextRequest, DeleteContextResponse,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::{key, Store};
use eyre::bail;
use tracing::info;

use crate::ContextManager;

impl Handler<DeleteContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <DeleteContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        DeleteContextRequest { context_id }: DeleteContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let task = delete_context(self.datastore.clone(), self.node_client.clone(), context_id);

        ActorResponse::r#async(wrap_future::<_, Self>(Box::pin(task)))
    }
}

async fn delete_context(
    datastore: Store,
    node_client: NodeClient,
    context_id: ContextId,
) -> eyre::Result<DeleteContextResponse> {
    let mut handle = datastore.handle();

    let key = key::ContextMeta::new(context_id);

    // todo! perhaps we shouldn't bother checking?
    if !handle.has(&key)? {
        return Ok(DeleteContextResponse { deleted: false });
    }

    handle.delete(&key)?;
    handle.delete(&key::ContextConfig::new(context_id))?;

    delete_context_scoped::<key::ContextIdentity, 32>(
        datastore.clone(),
        &context_id,
        [0; 32],
        None,
    )?;
    delete_context_scoped::<key::ContextState, 32>(datastore.clone(), &context_id, [0; 32], None)?;

    unsubscribe(&node_client, &context_id).await?;

    Ok(DeleteContextResponse { deleted: true })
}

#[expect(clippy::unwrap_in_result, reason = "pre-validated")]
fn delete_context_scoped<K, const N: usize>(
    mut datastore: Store,
    context_id: &ContextId,
    offset: [u8; N],
    end: Option<[u8; N]>,
) -> eyre::Result<()>
where
    K: key::FromKeyParts<Error: Error + Send + Sync>,
{
    let expected_length = key::Key::<K::Components>::len();

    if context_id.len().saturating_add(N) != expected_length {
        bail!(
            "key length mismatch, expected: {}, got: {}",
            key::Key::<K::Components>::len(),
            N
        )
    }

    let mut keys = vec![];

    let mut key = context_id.to_vec();

    let end = end
        .map(|end| {
            key.extend_from_slice(&end);

            let end = key::Key::<K::Components>::try_from_slice(&key).expect("length pre-matched");

            K::try_from_parts(end)
        })
        .transpose()?;

    'outer: loop {
        key.truncate(context_id.len());
        key.extend_from_slice(&offset);

        let offset = key::Key::<K::Components>::try_from_slice(&key).expect("length pre-matched");

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

    Ok(())
}

async fn unsubscribe(node_client: &NodeClient, context_id: &ContextId) -> eyre::Result<()> {
    node_client.unsubscribe(context_id).await?;

    info!(%context_id, "Unsubscribed from context");

    Ok(())
}
