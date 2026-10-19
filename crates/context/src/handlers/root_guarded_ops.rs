//! Handlers for the owner-level ops that need the account root's proof:
//! transferring a group, repointing a namespace's admin, and the owner-only group
//! deletion. Each builds the bare op, wraps it in its `RootGuarded` form with a
//! proof (see [`crate::root_guard`]), then signs, applies and publishes it like
//! any other governance op.
//!
//! The local apply runs before the publish, so a refusal (not the owner, a
//! target who is not an admin, a stale counter) reaches the caller and nothing
//! is published. An op this node cannot seal for peers is refused unapplied.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{
    ChangeNamespaceAdminRequest, OwnerDeleteGroupRequest, TransferOwnershipRequest,
};
use calimero_context_client::local_governance::{GroupOp, RootOp};
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::NamespaceRepository;
use calimero_primitives::identity::PrivateKey;
use tracing::info;

use crate::ContextManager;

impl ContextManager {
    /// Wrap `op` for `group_id` with its root proof, then sign, apply and
    /// publish it. `label` names the op in metrics and logs.
    fn publish_guarded_group_op(
        &mut self,
        group_id: calimero_context_config::types::ContextGroupId,
        op: GroupOp,
        root_proof: Option<calimero_account::SignedOwnerOp>,
        label: &'static str,
    ) -> ActorResponse<Self, eyre::Result<()>> {
        // No admin precheck: ownership is the apply's to decide, and the local
        // apply below runs before anything is published.
        let preflight = match self.governance_preflight(&group_id, false) {
            Ok(preflight) => preflight,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let op = match crate::root_guard::guard_group_op(
            &preflight.datastore,
            &group_id,
            &preflight.signer,
            op,
            root_proof,
        ) {
            Ok(op) => op,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let sk = preflight.signing_key;
        let datastore = preflight.datastore;
        let node_client = preflight.node_client;
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                let report = calimero_governance_store::sign_apply_and_publish_required(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    op,
                )
                .await?;
                report.observe(label, "RootGuarded");
                info!(?group_id, op = label, "published a root-guarded owner op");
                Ok(())
            }
            .into_actor(self),
        )
    }
}

impl Handler<TransferOwnershipRequest> for ContextManager {
    type Result = ActorResponse<Self, <TransferOwnershipRequest as Message>::Result>;

    fn handle(
        &mut self,
        TransferOwnershipRequest {
            group_id,
            new_owner,
            root_proof,
        }: TransferOwnershipRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.publish_guarded_group_op(
            group_id,
            GroupOp::TransferOwnership { new_owner },
            root_proof,
            "transfer_ownership",
        )
    }
}

impl Handler<OwnerDeleteGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <OwnerDeleteGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        OwnerDeleteGroupRequest {
            group_id,
            root_proof,
        }: OwnerDeleteGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.publish_guarded_group_op(
            group_id,
            GroupOp::GroupDelete,
            root_proof,
            "owner_delete_group",
        )
    }
}

impl Handler<ChangeNamespaceAdminRequest> for ContextManager {
    type Result = ActorResponse<Self, <ChangeNamespaceAdminRequest as Message>::Result>;

    fn handle(
        &mut self,
        ChangeNamespaceAdminRequest {
            namespace_id,
            new_admin,
            root_proof,
        }: ChangeNamespaceAdminRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let store = self.datastore.clone();
        let prepared = (|| -> eyre::Result<(RootOp, [u8; 32])> {
            let resolved = NamespaceRepository::new(&store).resolve(&namespace_id)?;
            if resolved != namespace_id {
                return Err(crate::error::ContextError::NamespaceNotFound {
                    namespace_id: namespace_id.to_string(),
                }
                .into());
            }
            let Some((signer, sk)) =
                NamespaceRepository::new(&store).resolve_identity(&namespace_id)?
            else {
                eyre::bail!(
                    "no local namespace identity for '{namespace_id:?}': cannot sign the admin change"
                );
            };
            let op = crate::root_guard::guard_root_op(
                &store,
                &namespace_id,
                &signer,
                RootOp::AdminChanged { new_admin },
                root_proof,
            )?;
            Ok((op, sk))
        })();
        let (op, sk_bytes) = match prepared {
            Ok(prepared) => prepared,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                let signer_sk = PrivateKey::from(sk_bytes);
                // Sealed: a root op whose publisher holds the namespace key, like
                // every admin-published root op (`root_op_is_sealable`).
                let op = calimero_governance_store::seal_root_op_for_publish(
                    &store,
                    namespace_id.to_bytes().into(),
                    op,
                )?;
                let report = calimero_governance_store::sign_apply_and_publish_namespace_op(
                    &store,
                    &node_client,
                    &ack_router,
                    namespace_id.to_bytes().into(),
                    &signer_sk,
                    op,
                )
                .await?;
                report.observe("change_namespace_admin", "RootGuarded");
                info!(?namespace_id, %new_admin, "namespace admin changed");
                Ok(())
            }
            .into_actor(self),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_client::group::{
        CreateGroupRequest, OwnerDeleteGroupRequest, SetTeeAdmissionPolicyRequest,
        SetTeeAuthoringPolicyRequest, SignedReleaseTrust,
    };
    use calimero_context_client::local_governance::TeeAdmissionMode;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        get_op_head, register_context_in_group, GroupKeyring, KeyringError, MetaError,
        MetaRepository, NodeDeviceRepository,
    };
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::ContextId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use crate::test_support::{actor, install_application};

    const APP: [u8; 32] = [0xE1; 32];

    /// A namespace this node founded and owns, with a Restricted subgroup, which
    /// seals its ops under its own key.
    async fn owned_namespace() -> (Store, actor::Harness, ContextGroupId, ContextGroupId) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        install_application(&store, ApplicationId::from(APP));
        let _root = NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the account root that signs the owner's proofs");
        let mut harness = actor::over(store.clone()).await;
        let create = |parent_group_id| CreateGroupRequest {
            salt: None,
            bytecode_id: None,
            application_id: Some(ApplicationId::from(APP)),
            name: None,
            parent_group_id,
            restricted: parent_group_id.is_some(),
        };
        let namespace = harness
            .manager
            .send(create(None))
            .await
            .expect("the manager answers")
            .expect("the namespace is created")
            .group_id;
        let subgroup = harness
            .manager
            .send(create(Some(namespace)))
            .await
            .expect("the manager answers")
            .expect("the subgroup is created")
            .group_id;
        let _created = harness.published_topics();
        (store, harness, namespace, subgroup)
    }

    fn topic(namespace: &ContextGroupId) -> String {
        format!("ns/{}", hex::encode(namespace.to_bytes()))
    }

    /// Leave this node holding no key for `group`, so nothing it signs there can be sealed.
    fn drop_keys(store: &Store, group: ContextGroupId) {
        let keyring = GroupKeyring::new(store, group);
        while let Some(key) = keyring.load_current_key_record().unwrap() {
            keyring.delete_key_by_id(&key.key_id).unwrap();
        }
    }

    fn sequence(store: &Store, group: &ContextGroupId) -> Option<u64> {
        get_op_head(store, group).unwrap().map(|head| head.sequence)
    }

    fn assert_cannot_seal(err: &eyre::Report) {
        assert!(
            matches!(
                err.downcast_ref::<KeyringError>(),
                Some(KeyringError::NoGroupKey(_))
            ),
            "expected NoGroupKey, got {err:?}"
        );
    }

    async fn owner_delete(harness: &actor::Harness, group_id: ContextGroupId) -> eyre::Result<()> {
        harness
            .manager
            .send(OwnerDeleteGroupRequest {
                group_id,
                root_proof: None,
            })
            .await
            .expect("the manager answers")
    }

    #[actix::test]
    async fn an_owner_delete_of_a_subgroup_is_published() {
        let (store, mut harness, namespace, subgroup) = owned_namespace().await;

        owner_delete(&harness, subgroup)
            .await
            .expect("the owner may delete");

        assert!(MetaRepository::new(&store)
            .load(&subgroup)
            .unwrap()
            .is_none());
        assert!(
            harness.published_topics().contains(&topic(&namespace)),
            "the delete never left this node"
        );
    }

    #[actix::test]
    async fn an_owner_delete_of_the_namespace_is_published() {
        let (store, mut harness, namespace, _subgroup) = owned_namespace().await;

        owner_delete(&harness, namespace)
            .await
            .expect("the owner may delete");

        assert!(MetaRepository::new(&store)
            .load(&namespace)
            .unwrap()
            .is_none());
        assert!(
            harness.published_topics().contains(&topic(&namespace)),
            "the delete never left this node"
        );
    }

    #[actix::test]
    async fn an_owner_delete_refused_for_a_context_publishes_nothing() {
        let (store, mut harness, namespace, subgroup) = owned_namespace().await;
        register_context_in_group(&store, &subgroup, &ContextId::from([0xE2; 32])).unwrap();

        let err = owner_delete(&harness, subgroup).await.unwrap_err();

        assert!(
            matches!(
                err.downcast_ref::<MetaError>(),
                Some(MetaError::HasRegisteredContexts)
            ),
            "expected HasRegisteredContexts, got {err:?}"
        );
        assert!(MetaRepository::new(&store)
            .load(&subgroup)
            .unwrap()
            .is_some());
        assert!(!harness.published_topics().contains(&topic(&namespace)));
    }

    #[actix::test]
    async fn an_owner_delete_this_node_cannot_seal_is_refused_unapplied() {
        let (store, mut harness, namespace, subgroup) = owned_namespace().await;
        drop_keys(&store, subgroup);

        let err = owner_delete(&harness, subgroup).await.unwrap_err();

        assert_cannot_seal(&err);
        assert!(
            MetaRepository::new(&store)
                .load(&subgroup)
                .unwrap()
                .is_some(),
            "a delete peers never see was applied here"
        );
        assert!(!harness.published_topics().contains(&topic(&namespace)));
    }

    #[actix::test]
    async fn a_tee_authoring_policy_this_node_cannot_seal_is_refused_unapplied() {
        let (store, mut harness, namespace, _subgroup) = owned_namespace().await;
        drop_keys(&store, namespace);
        let before = sequence(&store, &namespace);

        let err = harness
            .manager
            .send(SetTeeAuthoringPolicyRequest {
                group_id: namespace,
                allowed_mrtd: vec!["aa".to_owned()],
                root_proof: None,
            })
            .await
            .expect("the manager answers")
            .unwrap_err();

        assert_cannot_seal(&err);
        assert_eq!(
            sequence(&store, &namespace),
            before,
            "the policy was applied"
        );
        assert!(!harness.published_topics().contains(&topic(&namespace)));
    }

    #[actix::test]
    async fn a_tee_admission_policy_this_node_cannot_seal_is_refused_unapplied() {
        let (store, mut harness, namespace, _subgroup) = owned_namespace().await;
        drop_keys(&store, namespace);
        let before = sequence(&store, &namespace);

        let err = harness
            .manager
            .send(SetTeeAdmissionPolicyRequest {
                group_id: namespace,
                allowed_mrtd: vec![],
                allowed_rtmr0: vec![],
                allowed_rtmr1: vec![],
                allowed_rtmr2: vec![],
                allowed_rtmr3: vec![],
                allowed_tcb_statuses: vec![],
                accept_mock: false,
                signed_release: Some(SignedReleaseTrust {
                    allowed_profiles: vec!["locked-read-only".to_owned()],
                    min_release_version: None,
                }),
                mode: TeeAdmissionMode::Replica,
                root_proof: None,
            })
            .await
            .expect("the manager answers")
            .unwrap_err();

        assert_cannot_seal(&err);
        assert_eq!(
            sequence(&store, &namespace),
            before,
            "the policy was applied"
        );
        assert!(!harness.published_topics().contains(&topic(&namespace)));
    }
}
