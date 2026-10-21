use actix::{ActorFutureExt, ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{DeleteNamespaceRequest, DeleteNamespaceResponse};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    member_account_in_namespace, ForgottenNamespaces, MembershipRepository, MetaRepository,
    MetadataRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_primitives::application::ZERO_APPLICATION_ID;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
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
        // A local teardown, no DAG op: the account stays a member and peers see nothing.
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

            let account_namespace =
                NodeDeviceRepository::new(&self.datastore).account_namespace()?;
            if account_namespace == Some(namespace_id) {
                bail!(crate::error::ContextError::AccountNamespaceNotRemovable {
                    namespace_id: hex::encode(namespace_id.to_bytes()),
                });
            }

            // The marker is the restore proof, so only a member gets one; written
            // first, so a purge cut short is not followed again.
            if is_member(&self.datastore, &namespace_id, &signer)? {
                let application = MetaRepository::new(&self.datastore)
                    .load(&namespace_id)?
                    .map(|meta| meta.target.application_id)
                    .filter(|application| *application != ZERO_APPLICATION_ID);
                // Read before the purge, which drops the namespace's metadata row.
                let name = MetadataRepository::new(&self.datastore)
                    .group_metadata(&namespace_id)?
                    .and_then(|record| record.name);
                ForgottenNamespaces::new(&self.datastore).mark(
                    &namespace_id,
                    application,
                    now_ms(),
                    name,
                )?;
            }

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

            let outcome = purge_namespace(
                &datastore,
                &node_client,
                search.as_deref(),
                &namespace_id,
                &payload,
            )
            .await;

            if outcome.is_ok() {
                info!(
                    ?namespace_id,
                    %signer,
                    total_groups = payload.descendant_groups.len() + 1,
                    total_contexts = payload.contexts.len(),
                    "removed namespace and subtree from this node"
                );
            }

            (
                payload.contexts,
                outcome.map(|()| DeleteNamespaceResponse { deleted: true }),
            )
        };

        // Dropped on failure too: a purge cut short leaves cached state over missing rows.
        ActorResponse::r#async(task.into_actor(self).map(|(contexts, outcome), act, _ctx| {
            for context in contexts {
                let _ignored = act.contexts.remove(&context);
            }
            outcome
        }))
    }
}

fn is_member(store: &Store, namespace: &ContextGroupId, signer: &PublicKey) -> eyre::Result<bool> {
    let Some(account) = member_account_in_namespace(store, namespace, signer)? else {
        return Ok(false);
    };
    Ok(MembershipRepository::new(store)
        .role_of(namespace, &account)?
        .is_some())
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
    use calimero_primitives::metadata::MetadataRecord;
    use calimero_store::db::{Column, InMemoryDB};
    use calimero_store::Store;

    use super::*;
    use crate::test_support::actor;

    /// A namespace this node founded, with this node's own account then made a
    /// plain member of it; returns the namespace and that account.
    async fn a_namespace_this_node_only_belongs_to(
        harness: &actor::Harness,
        store: &Store,
    ) -> (calimero_context_config::types::ContextGroupId, AccountId) {
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
        (namespace, account)
    }

    /// A plain member may remove the namespace from its own device, and leaves the
    /// forgotten marker as the proof the account was a member.
    #[actix::test]
    async fn a_plain_member_may_remove_the_namespace_from_this_device() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the founder's account root");
        let harness = actor::over(store.clone()).await;
        let (namespace, _account) = a_namespace_this_node_only_belongs_to(&harness, &store).await;

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

    /// The restore names the namespace by what the user saw, not by its id.
    #[actix::test]
    async fn the_marker_keeps_the_namespace_name_the_purge_drops() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the founder's account root");
        let harness = actor::over(store.clone()).await;
        let (namespace, _account) = a_namespace_this_node_only_belongs_to(&harness, &store).await;
        MetadataRepository::new(&store)
            .set_group(
                &namespace,
                &MetadataRecord {
                    name: Some("Team chat".into()),
                    data: Default::default(),
                    updated_at: 1,
                    updated_by: PublicKey::from([1; 32]),
                },
            )
            .expect("name the namespace");

        harness
            .manager
            .send(DeleteNamespaceRequest {
                namespace_id: namespace,
            })
            .await
            .expect("the manager answers")
            .expect("remove the namespace");

        assert!(MetadataRepository::new(&store)
            .group_metadata(&namespace)
            .expect("read the metadata")
            .is_none());
        let marker = ForgottenNamespaces::new(&store).list().expect("list");
        assert_eq!(marker[0].name.as_deref(), Some("Team chat"));
    }

    /// A node that soft-left keeps its residue, and may clear it. It is no
    /// longer a member, so there is nothing for a later restore to come back to.
    #[actix::test]
    async fn a_node_that_soft_left_clears_its_residue_and_gets_no_marker() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the founder's account root");
        let harness = actor::over(store.clone()).await;
        let (namespace, account) = a_namespace_this_node_only_belongs_to(&harness, &store).await;
        MembershipRepository::new(&store)
            .remove_member(&namespace, &account)
            .expect("the account left, leaving this node's identity behind");

        let deleted = harness
            .manager
            .send(DeleteNamespaceRequest {
                namespace_id: namespace,
            })
            .await
            .expect("the manager answers")
            .expect("a soft-left node may clear what the leave kept");

        assert!(deleted.deleted);
        assert!(MetaRepository::new(&store)
            .load(&namespace)
            .expect("read the meta")
            .is_none());
        assert!(!ForgottenNamespaces::new(&store)
            .is_forgotten(&namespace)
            .expect("read the marker"));
    }

    /// The purge keeps the namespace identity to the end, so asking again finishes a
    /// purge that went down after the group rows did.
    #[actix::test]
    async fn a_purge_cut_short_is_finished_by_asking_again() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let harness = actor::over(store.clone()).await;
        let namespace = ContextGroupId::from([0xA3; 32]);
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&namespace)
            .expect("this node takes part");
        let leftover = [namespace.to_bytes().to_vec(), vec![0x07; 32]].concat();
        store
            .raw_put(Column::UnifiedOp, &leftover, b"op")
            .expect("leave an op-log row behind");

        harness
            .manager
            .send(DeleteNamespaceRequest {
                namespace_id: namespace,
            })
            .await
            .expect("the manager answers")
            .expect("the rerun finishes the purge");

        assert!(store
            .raw_get(Column::UnifiedOp, &leftover)
            .expect("read")
            .is_none());
        assert!(NamespaceRepository::new(&store)
            .participating_namespaces()
            .expect("read")
            .is_empty());
    }

    /// The account namespace stays: on a holder the next start would re-found it.
    #[actix::test]
    async fn the_account_namespace_cannot_be_removed() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let root = NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the account root");
        let harness = actor::over(store.clone()).await;
        let namespace = root.account_namespace();
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&namespace)
            .expect("this node takes part");

        let err = harness
            .manager
            .send(DeleteNamespaceRequest {
                namespace_id: namespace,
            })
            .await
            .expect("the manager answers")
            .expect_err("the account namespace stays");

        assert!(matches!(
            err.downcast_ref::<crate::error::ContextError>(),
            Some(crate::error::ContextError::AccountNamespaceNotRemovable { .. })
        ));
        assert_eq!(
            NamespaceRepository::new(&store)
                .participating_namespaces()
                .expect("read"),
            vec![namespace]
        );
    }

    /// A paired device records its account namespace instead of deriving it from a
    /// root, and is held to the same refusal.
    #[actix::test]
    async fn a_paired_devices_account_namespace_cannot_be_removed_either() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let namespace = ContextGroupId::from([0xA7; 32]);
        NodeDeviceRepository::new(&store)
            .store_account_namespace(&namespace)
            .expect("record what the pairing named");
        let harness = actor::over(store.clone()).await;
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&namespace)
            .expect("this node takes part");

        let err = harness
            .manager
            .send(DeleteNamespaceRequest {
                namespace_id: namespace,
            })
            .await
            .expect("the manager answers")
            .expect_err("the account namespace stays");

        assert!(matches!(
            err.downcast_ref::<crate::error::ContextError>(),
            Some(crate::error::ContextError::AccountNamespaceNotRemovable { .. })
        ));
    }
}
