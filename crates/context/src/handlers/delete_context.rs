use std::collections::{btree_map, BTreeMap};
use std::mem;
use std::sync::Arc;

use actix::fut::wrap_future;
use actix::{ActorResponse, ActorTryFutureExt, Handler, Message};
use calimero_context_primitives::messages::delete_context::{
    DeleteContextRequest, DeleteContextResponse,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::layer::ReadLayer;
use calimero_store::{key, types, Store};
use eyre::{bail, OptionExt};
use rand::rngs::StdRng;
use rand::SeedableRng;
use tokio::sync::{Mutex, OwnedMutexGuard};

use super::execute::execute;
use super::execute::storage::ContextStorage;
use crate::{ContextManager, ContextMeta};

impl Handler<DeleteContextRequest> for ContextManager {
    type Result = <DeleteContextResponse as Message>::Result;

    fn handle(
        &mut self,
        DeleteContextRequest { context_id }: DeleteContextResponse,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let mut handle = self.datastore.handle();

        let key = key::ContextMeta::new(context_id);

        // todo! perhaps we shouldn't bother checking?
        if !handle.has(&key)? {
            return Ok(DeleteContextResponse { deleted: false });
        }

        handle.delete(&key)?;
        handle.delete(&key::ContextConfig::new(context_id))?;

        delete_context_scoped::<key::ContextIdentity, 32>(
            self.datastore.clone(),
            &context_id,
            [0; 32],
            None,
        )?;
        delete_context_scoped::<key::ContextState, 32>(
            self.datastore.clone(),
            &context_id,
            [0; 32],
            None,
        )?;

        Ok(DeleteContextResponse { deleted: true })
    }
}

#[expect(clippy::unwrap_in_result, reason = "pre-validated")]
fn delete_context_scoped<K, const N: usize>(
    datastore: Store,
    context_id: &ContextId,
    offset: [u8; N],
    end: Option<[u8; N]>,
) -> EyreResult<()>
where
    K: FromKeyParts<Error: Error + Send + Sync>,
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
