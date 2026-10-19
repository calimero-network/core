use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_client::group::{DeleteNamespaceRequest, DeleteNamespaceResponse};
use calimero_governance_store::{ForgottenNamespaces, MetaRepository, NamespaceRepository};
use calimero_primitives::application::ZERO_APPLICATION_ID;
use either::Either;
use eyre::bail;
use tracing::info;

use crate::handlers::purge_namespace::purge_namespace;
use crate::ContextManager;

impl Handler<DeleteNamespaceRequest> for ContextManager {
    type Result = ActorResponse<Self, <DeleteNamespaceRequest as Message>::Result>;

    fn handle(
        &mut self,
        DeleteNamespaceRequest { namespace_id }: DeleteNamespaceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Removing a namespace from this device is a purely local teardown - no
        // DAG-replicated op. The account stays a member: peers and its other
        // devices see no change, and the node refuses to follow the namespace
        // again until the user joins it explicitly.
        let (signer, _) = match self.require_namespace_signing_key(&namespace_id) {
            Ok(key) => key,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let result = (|| {
            // Only a namespace root is a valid target - resolve and compare.
            let resolved = NamespaceRepository::new(&self.datastore).resolve(&namespace_id)?;
            if resolved != namespace_id {
                bail!(
                    "group '{namespace_id:?}' is not a namespace root; \
                     resolved namespace is '{resolved:?}' - use delete_group instead"
                );
            }

            let Some(meta) = MetaRepository::new(&self.datastore).load(&namespace_id)? else {
                bail!(crate::error::ContextError::NamespaceNotFound {
                    namespace_id: namespace_id.to_string(),
                });
            };

            // Before the purge: a purge cut short must not leave a namespace the
            // next start follows again.
            let application = meta.target.application_id;
            ForgottenNamespaces::new(&self.datastore).mark(
                &namespace_id,
                (application != ZERO_APPLICATION_ID).then_some(application),
                now_ms(),
            )?;

            NamespaceRepository::new(&self.datastore).collect_subtree_for_cascade(&namespace_id)
        })();
        let payload = match result {
            Ok(payload) => payload,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // Hold every cached context's lock so no run writes while its rows go.
        let locks: Vec<_> = payload
            .contexts
            .iter()
            .filter_map(|context| self.contexts.get(context))
            .map(|context| context.lock())
            .collect();

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let search = self.search.clone();

        let task = async move {
            let mut _held = Vec::with_capacity(locks.len());
            for lock in locks {
                _held.push(match lock {
                    Either::Left(guard) => guard,
                    Either::Right(task) => task.await,
                });
            }

            let contexts = purge_namespace(
                &datastore,
                &node_client,
                search.as_deref(),
                &namespace_id,
                &payload,
            )
            .await?;

            info!(
                ?namespace_id,
                %signer,
                total_groups = payload.descendant_groups.len() + 1,
                total_contexts = contexts.len(),
                "removed namespace and subtree from this node"
            );

            Ok((contexts, DeleteNamespaceResponse { deleted: true }))
        };

        ActorResponse::r#async(
            task.into_actor(self)
                .map_ok(|(contexts, response), act, _ctx| {
                    for context in contexts {
                        let _ignored = act.contexts.remove(&context);
                    }
                    response
                }),
        )
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::AccountId;
    use calimero_context_client::group::CreateGroupRequest;
    use calimero_governance_store::{
        MembershipRepository, NamespaceRepository, NodeDeviceRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::*;
    use crate::test_support::actor;

    /// A namespace this node founded, with this node's own account then made a
    /// plain member of it.
    async fn a_namespace_this_node_only_belongs_to(
        harness: &actor::Harness,
        store: &Store,
    ) -> calimero_context_config::types::ContextGroupId {
        let created = harness
            .manager
            .send(CreateGroupRequest {
                salt: None,
                bytecode_id: None,
                application_id: None,
                name: None,
                parent_group_id: None,
                restricted: true,
            })
            .await
            .expect("the manager answers")
            .expect("the namespace is created");
        let namespace = created.group_id;
        let (_, own_pk, _) = NamespaceRepository::new(store)
            .participate_in(&namespace)
            .expect("read this node's identity");
        let account = crate::member_account::require(store, &namespace, &own_pk)
            .expect("this node's account");
        MembershipRepository::new(store)
            .set_role(&namespace, &account, GroupMemberRole::Member)
            .expect("demote to a plain member");
        let meta = MetaRepository::new(store);
        let mut row = meta
            .load(&namespace)
            .expect("read the meta")
            .expect("the namespace has one");
        row.admin_identity = AccountId::from([0xF2; 32]);
        row.owner_identity = AccountId::from([0xF2; 32]);
        meta.save(&namespace, &row)
            .expect("hand the namespace over");
        namespace
    }

    /// Removing a namespace from this device is the device's own business: a
    /// plain member may do it, and it leaves the forgotten marker behind.
    #[actix::test]
    async fn a_plain_member_may_remove_the_namespace_from_this_device() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the founder's account root");
        let harness = actor::over(store.clone()).await;
        let namespace = a_namespace_this_node_only_belongs_to(&harness, &store).await;

        let deleted = harness
            .manager
            .send(DeleteNamespaceRequest {
                namespace_id: namespace,
            })
            .await
            .expect("the manager answers")
            .expect("a member may drop the namespace from its own node");

        assert!(deleted.deleted);
        assert!(MetaRepository::new(&store)
            .load(&namespace)
            .expect("read the meta")
            .is_none());
        assert!(ForgottenNamespaces::new(&store)
            .is_forgotten(&namespace)
            .expect("read the marker"));
    }
}
