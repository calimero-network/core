use calimero_governance_store::{MetaRepository, NamespaceRepository};
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{DeleteGroupRequest, DeleteGroupResponse};
use calimero_context_client::local_governance::{NamespaceOp, RootOp};
use calimero_primitives::identity::PrivateKey;
use eyre::bail;
use tracing::info;

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;

impl Handler<DeleteGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <DeleteGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        DeleteGroupRequest {
            group_id,
            requester,
        }: DeleteGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Strict-tree refactor: delete now cascades. The handler builds the
        // full subtree payload (descendants + contained contexts), then emits
        // a single RootOp::GroupDeleted on the namespace DAG. The dispatch
        // arm in execute_group_deleted re-enumerates locally and rejects
        // payload mismatches (deterministic-application check across peers).
        // See spec docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md
        let node_identity = self.node_namespace_identity(&group_id);

        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured namespace identity"
                    )))
                }
            },
        };

        // Resolve namespace identity for signing the RootOp. With the strict
        // tree invariant, every group except the root has a reachable
        // namespace, so this always succeeds for valid (non-root) targets.
        let namespace_identity =
            match NamespaceRepository::new(&self.datastore).resolve_identity(&group_id) {
                Ok(Some((pk, sk, _sender))) => (pk, sk),
                Ok(None) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                "no local namespace identity for group '{group_id:?}': cannot sign cascade delete"
            )))
                }
                Err(err) => return ActorResponse::reply(Err(err)),
            };

        // Sync validation + cascade payload pre-computation.
        let validated =
            (|| -> eyre::Result<(calimero_governance_store::CascadePayload, [u8; 32])> {
                let Some(meta) = MetaRepository::new(&self.datastore).load(&group_id)? else {
                    bail!("group '{group_id:?}' not found");
                };

                // Reject the namespace root explicitly; it has no parent edge to
                // identify it as a subtree, and the namespace-deletion path is
                // separate (delete_namespace).
                let namespace_id = NamespaceRepository::new(&self.datastore).resolve(&group_id)?;
                if namespace_id == group_id {
                    bail!(
                    "cannot delete the namespace root '{group_id:?}': use delete_namespace instead"
                );
                }

                // Authorization (re-checked on every peer in `execute_group_deleted`):
                // the subgroup's owner, an admin of the namespace root, or a
                // namespace member holding CAN_DELETE_SUBGROUP. The non-owner case
                // routes through `PermissionChecker` to stay in step with the
                // create / set-visibility handlers.
                if meta.owner_identity != requester {
                    calimero_governance_store::PermissionChecker::new(
                        &self.datastore,
                        namespace_id,
                    )
                    .require_can_delete_subgroup(&requester)
                    .map_err(|e| {
                        eyre::eyre!("deleting subgroup '{group_id:?}': {e} (or be its owner)")
                    })?;
                }

                let payload = NamespaceRepository::new(&self.datastore)
                    .collect_subtree_for_cascade(&group_id)?;
                Ok((payload, namespace_id.to_bytes()))
            })();

        let (cascade_payload, namespace_id_bytes) = match validated {
            Ok(v) => v,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let cascade_group_ids: Vec<[u8; 32]> = cascade_payload
            .descendant_groups
            .iter()
            .map(|g| g.to_bytes())
            .collect();
        let cascade_context_ids: Vec<[u8; 32]> =
            cascade_payload.contexts.iter().map(|c| **c).collect();
        let total_groups = cascade_group_ids.len() + 1;
        let total_contexts = cascade_context_ids.len();

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let group_id_bytes = group_id.to_bytes();
        let (_, signer_sk_bytes) = namespace_identity;

        ActorResponse::r#async(
            async move {
                let signer_sk = PrivateKey::from(signer_sk_bytes);
                let op = NamespaceOp::Root(RootOp::GroupDeleted {
                    root_group_id: group_id_bytes,
                    cascade_group_ids,
                    cascade_context_ids,
                });

                let report = calimero_governance_store::sign_apply_and_publish_namespace_op(
                    &datastore,
                    &node_client,
                    &ack_router,
                    namespace_id_bytes,
                    &signer_sk,
                    op,
                )
                .await?;
                report.observe("delete_group", "GroupDeleted");
                // C3 Stage 1: mirror the locally-authored GroupDeleted into the op-store.
                crate::scope_projection::ScopeProjections::persist_namespace_head_ops(
                    &datastore,
                    namespace_id_bytes,
                );

                // Best-effort unsubscribe — the group is gone now, no point
                // staying on its topic. Subscriptions for descendants likewise.
                let _ = node_client.unsubscribe_namespace(group_id_bytes).await;

                info!(
                    ?group_id,
                    %requester,
                    total_groups,
                    total_contexts,
                    "cascade-deleted group subtree"
                );

                Ok(DeleteGroupResponse { deleted: true })
            }
            .into_actor(self),
        )
    }
}
