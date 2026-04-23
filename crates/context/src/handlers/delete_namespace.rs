use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{DeleteNamespaceRequest, DeleteNamespaceResponse};
use eyre::bail;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<DeleteNamespaceRequest> for ContextManager {
    type Result = ActorResponse<Self, <DeleteNamespaceRequest as Message>::Result>;

    fn handle(
        &mut self,
        DeleteNamespaceRequest {
            namespace_id,
            requester,
        }: DeleteNamespaceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Namespace deletion is a purely local teardown — no DAG-replicated
        // op, symmetric with namespace creation. `RootOp::GroupDeleted`
        // explicitly rejects the namespace root (see `execute_group_deleted`),
        // so we can't fall through `delete_group` for this.
        //
        // Each node that wishes to tear down its local namespace state calls
        // this handler; peers continue to hold their own namespace state
        // until they do the same.
        let requester = match requester {
            Some(pk) => pk,
            None => match self.node_namespace_identity(&namespace_id) {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured namespace identity"
                    )))
                }
            },
        };

        let result = (|| -> eyre::Result<(usize, usize)> {
            // Only a namespace root is a valid target — resolve and compare.
            let resolved = group_store::resolve_namespace(&self.datastore, &namespace_id)?;
            if resolved != namespace_id {
                bail!(
                    "group '{namespace_id:?}' is not a namespace root; \
                     resolved namespace is '{resolved:?}' — use delete_group instead"
                );
            }

            if group_store::load_group_meta(&self.datastore, &namespace_id)?.is_none() {
                bail!("namespace '{namespace_id:?}' not found");
            }

            // Admin authorization against the namespace root.
            group_store::require_group_admin(&self.datastore, &namespace_id, &requester)?;

            // Enumerate the full subtree so we can tear down children-first.
            let payload = group_store::collect_subtree_for_cascade(&self.datastore, &namespace_id)?;
            let total_groups = payload.descendant_groups.len() + 1;
            let total_contexts = payload.contexts.len();

            // Children-first: every descendant, then the namespace root itself.
            // For each group: unregister contexts, delete_group_local_rows
            // (members, signing keys, caps, aliases, upgrades, op-log + head,
            // meta, nonces, member-context joins), then remove its parent
            // edge + child-index entry on the parent. Mirrors the cascade
            // arm in `execute_group_deleted`.
            let all_groups_iter = payload
                .descendant_groups
                .iter()
                .copied()
                .chain(std::iter::once(namespace_id));
            for gid in all_groups_iter {
                for ctx in
                    group_store::enumerate_group_contexts(&self.datastore, &gid, 0, usize::MAX)?
                {
                    group_store::unregister_context_from_group(&self.datastore, &gid, &ctx)?;
                }
                let parent_for_cleanup = group_store::get_parent_group(&self.datastore, &gid)?;
                group_store::delete_group_local_rows(&self.datastore, &gid)?;
                if let Some(parent) = parent_for_cleanup {
                    let mut handle = self.datastore.handle();
                    handle.delete(&calimero_store::key::GroupParentRef::new(gid.to_bytes()))?;
                    handle.delete(&calimero_store::key::GroupChildIndex::new(
                        parent.to_bytes(),
                        gid.to_bytes(),
                    ))?;
                }
            }

            // Namespace-level rows: identity, DAG head, and every stored
            // governance op for this namespace.
            group_store::delete_namespace_local_state(&self.datastore, &namespace_id)?;

            Ok((total_groups, total_contexts))
        })();

        let (total_groups, total_contexts) = match result {
            Ok(v) => v,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let node_client = self.node_client.clone();
        let namespace_id_bytes = namespace_id.to_bytes();

        ActorResponse::r#async(
            async move {
                // Best-effort unsubscribe — the namespace is gone locally,
                // no point staying on its governance topic.
                let _ = node_client.unsubscribe_namespace(namespace_id_bytes).await;

                info!(
                    ?namespace_id,
                    %requester,
                    total_groups,
                    total_contexts,
                    "deleted namespace and subtree"
                );

                Ok(DeleteNamespaceResponse { deleted: true })
            }
            .into_actor(self),
        )
    }
}
