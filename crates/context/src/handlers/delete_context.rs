use calimero_governance_store::{MembershipRepository, NamespaceRepository};
use std::sync::Arc;

use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_client::local_governance::AckRouter;
use calimero_context_client::messages::{DeleteContextRequest, DeleteContextResponse};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_store::db::Column;
use calimero_store::{key, Store};
use either::Either;

use calimero_primitives::identity::PrivateKey;

use crate::scope_projection::ScopeProjections;
use crate::ContextManager;
use calimero_governance_store::governance_broadcast::ObserveDelivery;

impl Handler<DeleteContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <DeleteContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        DeleteContextRequest { context_id }: DeleteContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let context = self.contexts.get(&context_id);

        let mut guard = None;

        if let Some(context) = context {
            guard = Some(context.lock());
        } else {
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
        let ack_router = Arc::clone(&self.ack_router);
        let search = self.search.clone();

        let group_id_for_context =
            match calimero_governance_store::get_group_for_context(&self.datastore, &context_id) {
                Ok(g) => g,
                Err(err) => return ActorResponse::reply(Err(err)),
            };

        if let Some(group_id) = group_id_for_context {
            // Detach is refused after a purge, so ask as it will be, at the current heads.
            let at_heads = ScopeProjections::namespace_current_heads(&self.datastore, group_id)
                .and_then(|heads| {
                    self.scope_projections
                        .read()
                        .ok()?
                        .context_rotation_group_at_cut(
                            &self.datastore,
                            group_id,
                            &context_id,
                            &heads,
                        )
                });
            let refused = match at_heads {
                Some(Some(group)) => Err(eyre::eyre!(
                    calimero_governance_store::ContextRegistrationError::HasRotatedCells {
                        group_id: hex::encode(group.to_bytes()),
                        context_id: context_id.to_string(),
                    }
                )),
                Some(None) => Ok(()),
                None => calimero_governance_store::require_context_not_rotated(
                    &self.datastore,
                    &context_id,
                ),
            };
            if let Err(err) = refused {
                return ActorResponse::reply(Err(err));
            }
            // The node signs as itself; there is one key and nothing to choose.
            let (signer, _) = match self.resolve_signer(&group_id) {
                Ok(pair) => pair,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
            let signer_account =
                match crate::member_account::require(&self.datastore, &group_id, &signer) {
                    Ok(account) => account,
                    Err(err) => return ActorResponse::reply(Err(err)),
                };
            if let Err(err) =
                MembershipRepository::new(&self.datastore).require_admin(&group_id, &signer_account)
            {
                return ActorResponse::reply(Err(err));
            }
        }

        let task = async move {
            let _guard = match guard {
                Some(Either::Left(guard)) => Some(guard),
                Some(Either::Right(task)) => Some(task.await),
                None => None,
            };

            // Close the context's indexes (their writers and readers) before
            // the purge takes their files away, and drop its search rows. The
            // purge drops them again with the rest, on a node without search
            // too. Should the delete fail after this, the index is only
            // derived data: the next search view rebuilds it.
            if let Some(search) = search {
                search.delete_context(context_id.as_ref())?;
            }
            delete_context(datastore, node_client, ack_router, context_id).await?;

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
    ack_router: Arc<AckRouter>,
    context_id: ContextId,
) -> eyre::Result<()> {
    node_client.unsubscribe(&context_id).await?;

    purge_context_rows(&datastore, &context_id)?;

    if let Some(group_id) =
        calimero_governance_store::get_group_for_context(&datastore, &context_id)?
    {
        // A NON-creating read. `participate_in` notes participation as a
        // side effect, so resolving through it would have a *delete* leave behind
        // a row claiming this node takes part in the group — for a context whose
        // group it may never have joined, which is exactly when a stale
        // registration reaches here.
        let Some((_node_pk, sk)) =
            NamespaceRepository::new(&datastore).resolve_identity(&group_id)?
        else {
            eyre::bail!(crate::error::ContextError::NotAGroupMember {
                group_id: group_id.to_string(),
            });
        };
        let report = calimero_governance_store::sign_apply_and_publish(
            &datastore,
            &node_client,
            &ack_router,
            &group_id,
            &PrivateKey::from(sk),
            calimero_context_client::local_governance::GroupOp::ContextDetached { context_id },
        )
        .await?;
        report.observe("delete_context", "ContextDetached");
    }

    Ok(())
}

/// Removes the rows this node holds for `context_id`: its state, private state,
/// member identities, ordered indexes, full-text index and its dirty log, its
/// blob associations and blob ownership, a snapshot still being installed,
/// and buffered straggler deltas. A blob's bytes stay; its reference count
/// decides when they go.
///
/// Each column is cleared with one range delete over the context's key prefix
/// rather than one point delete per row, so a large context leaves a single
/// tombstone per column instead of one per entity.
///
/// Kept on purpose:
/// - `Delta`: deltas are part of the distributed DAG and must stay servable to
///   peers syncing missing parents; context deletion is a soft delete of it.
/// - `ContextWarrantNonce`: the per-author, per-executor replay ledger.
///   Dropping it would let a warrant already spent here be accepted again if
///   the context returns.
/// - `ContextLocal` and the single-row migration markers: tiny, and they record
///   decisions (a `leave_context`, a pinned bytecode) that must not be undone
///   by accident.
fn purge_context_rows(datastore: &Store, context_id: &ContextId) -> eyre::Result<()> {
    let mut handle = datastore.handle();
    handle.delete(&key::ContextMeta::new(*context_id))?;
    handle.delete(&key::ContextConfig::new(*context_id))?;

    // Every key in these columns starts with the context id: synced state, its
    // node-local private half, member identities, the node-local columns
    // derived from state (the ordered indexes, the full-text index and its
    // dirty log), the blobs held for it and a half-installed snapshot. The search rows go even on a node
    // that runs search off, so a context that returns later never meets a stale index.
    for column in [
        Column::State,
        Column::PrivateState,
        Column::Identity,
        Column::SortedIndex,
        Column::SortedIndexMeta,
        Column::SearchIndex,
        Column::SearchDirty,
        Column::ContextBlob,
        Column::BlobOwner,
        Column::SnapshotStage,
    ] {
        datastore.raw_delete_prefix(column, context_id.as_ref())?;
    }

    // Straggler deltas buffered for a schema this binary could not yet read.
    let mut absorbed = vec![key::ABSORB_BUFFER_PREFIX];
    absorbed.extend_from_slice(context_id.as_ref());
    datastore.raw_delete_prefix(Column::AbsorbBuffer, &absorbed)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_store::db::{Column, InMemoryDB};
    use calimero_store::{key, Store};

    use super::purge_context_rows;

    /// Adjacent ids, so a range that overshot the deleted context's prefix by
    /// one would visibly eat into its neighbour.
    const DELETED: [u8; 32] = [0x11; 32];
    const KEPT: [u8; 32] = [0x12; 32];

    fn prefixed(context: [u8; 32], tail: &[u8]) -> Vec<u8> {
        let mut key = context.to_vec();
        key.extend_from_slice(tail);
        key
    }

    fn absorbed(context: [u8; 32]) -> Vec<u8> {
        let mut key = vec![key::ABSORB_BUFFER_PREFIX];
        key.extend_from_slice(&context);
        key.extend_from_slice(&[0xAB; 64]);
        key
    }

    /// One row per context-scoped column, for `context`.
    fn rows(context: [u8; 32]) -> Vec<(Column, Vec<u8>)> {
        vec![
            (Column::Meta, context.to_vec()),
            (Column::Config, context.to_vec()),
            (Column::State, prefixed(context, &[0x01; 32])),
            (Column::PrivateState, prefixed(context, &[0x02; 32])),
            (Column::Identity, prefixed(context, &[0x03; 32])),
            // Ordered-index keys are variable length: collection ‖ order key.
            (Column::SortedIndex, prefixed(context, &[0x04; 45])),
            (Column::SortedIndexMeta, prefixed(context, &[0x05; 32])),
            // Index file chunks: context ‖ index name ‖ file ‖ chunk; dirty
            // rows: context ‖ seq, and the bare context id (the counter).
            (Column::SearchIndex, prefixed(context, &[0x08; 20])),
            (Column::SearchDirty, prefixed(context, &[0x09; 8])),
            (Column::SearchDirty, context.to_vec()),
            // Blob associations: context ‖ blob id.
            (Column::ContextBlob, prefixed(context, &[0x0a; 32])),
            (Column::AbsorbBuffer, absorbed(context)),
            (Column::Delta, prefixed(context, &[0x06; 32])),
            (Column::ContextWarrantNonce, prefixed(context, &[0x07; 64])),
            (Column::BlobOwner, prefixed(context, &[0x08; 32])),
            (Column::SnapshotStage, prefixed(context, &[0x0b; 33])),
        ]
    }

    fn present(store: &Store, (column, key): &(Column, Vec<u8>)) -> bool {
        store
            .raw_get(*column, key)
            .expect("read should succeed")
            .is_some()
    }

    #[test]
    fn purge_removes_the_contexts_node_local_rows_and_keeps_its_dag() {
        // Context deletion used to remove Meta, Config, Identity and State
        // only, one point delete per row, and left PrivateState, SortedIndex,
        // SortedIndexMeta and AbsorbBuffer behind for good: nothing else ever
        // reads rows of a context that no longer exists.
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        for row in rows(DELETED).iter().chain(rows(KEPT).iter()) {
            store
                .raw_put(row.0, &row.1, b"value")
                .expect("write should succeed");
        }

        purge_context_rows(&store, &DELETED.into()).expect("purge should succeed");

        for row in rows(DELETED) {
            let expected = matches!(row.0, Column::Delta | Column::ContextWarrantNonce);
            assert_eq!(
                present(&store, &row),
                expected,
                "{:?} row of the deleted context: expected present = {expected}",
                row.0
            );
        }
        for row in rows(KEPT) {
            assert!(
                present(&store, &row),
                "{:?} row of a neighbouring context must survive",
                row.0
            );
        }
    }
}
