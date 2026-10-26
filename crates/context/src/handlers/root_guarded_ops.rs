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

use calimero_governance_store::private_space::Tree;

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
        if let Err(err) = Tree::live(&self.datastore, &group_id)
            .and_then(|tree| tree.require_may_delete(&group_id))
        {
            return ActorResponse::reply(Err(err));
        }
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
    use std::collections::HashSet;

    use calimero_account::AccountId;
    use calimero_context_client::group::{
        AddGroupMembersRequest, CreateGroupInvitationRequest, CreateGroupRequest,
        LeaveNamespaceRequest, ListGroupMembersRequest, ListNamespacesRequest,
        OwnerDeleteGroupRequest, SetSubgroupVisibilityRequest, SetTeeAdmissionPolicyRequest,
        SetTeeAuthoringPolicyRequest, SignedReleaseTrust,
    };
    use calimero_context_client::local_governance::{SignedNamespaceOp, TeeAdmissionMode};
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::VisibilityMode;
    use calimero_dag::DeltaApplier;
    use calimero_governance_store::{
        get_op_head, register_context_in_group, GroupKeyring, KeyringError, MembershipRepository,
        MetaError, MetaRepository, NamespaceGovernance, NamespaceOpLogService,
        NodeDeviceRepository,
    };
    use calimero_op::OpPayload;
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::identity::MemberIdentity;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::*;
    use crate::governance_dag::{signed_namespace_op_to_delta, NamespaceGovernanceApplier};
    use crate::scope_projection::ScopeProjections;
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

    fn release_policy(
        group_id: ContextGroupId,
        mode: TeeAdmissionMode,
    ) -> SetTeeAdmissionPolicyRequest {
        SetTeeAdmissionPolicyRequest {
            group_id,
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
            mode,
            root_proof: None,
        }
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
        let (store, mut harness, namespace, _space) = owned_namespace().await;

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
    async fn an_owner_delete_of_a_folder_holding_a_space_publishes_nothing() {
        let (store, mut harness, namespace, _unused) = owned_namespace().await;
        let new_group = async |parent, restricted| {
            harness
                .manager
                .send(CreateGroupRequest {
                    salt: None,
                    bytecode_id: None,
                    application_id: Some(ApplicationId::from(APP)),
                    name: None,
                    parent_group_id: Some(parent),
                    restricted,
                })
                .await
                .expect("the manager answers")
                .expect("the group is created")
                .group_id
        };
        let folder = new_group(namespace, false).await;
        let space = new_group(folder, true).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _created = harness.published_topics();

        let err = owner_delete(&harness, folder).await.unwrap_err();

        assert!(
            matches!(
                err.downcast_ref::<calimero_governance_store::PrivateSpaceError>(),
                Some(calimero_governance_store::PrivateSpaceError::HoldsSpace(_))
            ),
            "expected HoldsSpace, got {err:?}"
        );
        assert!(MetaRepository::new(&store).load(&space).unwrap().is_some());
        assert!(
            harness.published_topics().is_empty(),
            "a refused delete reached the wire"
        );
    }

    fn refused_by_private_space(err: &eyre::Report) -> bool {
        err.downcast_ref::<calimero_governance_store::PrivateSpaceError>()
            .is_some()
    }

    #[actix::test]
    async fn a_request_that_breaks_the_space_structure_is_refused_before_it_is_signed() {
        let (_store, mut harness, namespace, space) = owned_namespace().await;
        let folder = harness
            .manager
            .send(CreateGroupRequest {
                salt: None,
                bytecode_id: None,
                application_id: Some(ApplicationId::from(APP)),
                name: None,
                parent_group_id: Some(space),
                restricted: false,
            })
            .await
            .expect("the manager answers")
            .expect("a folder inside the space")
            .group_id;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _created = harness.published_topics();

        let nested = harness
            .manager
            .send(CreateGroupRequest {
                salt: None,
                bytecode_id: None,
                application_id: Some(ApplicationId::from(APP)),
                name: None,
                parent_group_id: Some(space),
                restricted: true,
            })
            .await
            .expect("the manager answers");
        let private = harness
            .manager
            .send(SetSubgroupVisibilityRequest {
                group_id: folder,
                subgroup_visibility: VisibilityMode::Restricted,
            })
            .await
            .expect("the manager answers");
        let member = harness
            .manager
            .send(AddGroupMembersRequest {
                group_id: folder,
                members: vec![(
                    MemberIdentity::from(AccountId::from([0xB0; 32])),
                    GroupMemberRole::Member,
                )],
            })
            .await
            .expect("the manager answers");
        let invitation = harness
            .manager
            .send(CreateGroupInvitationRequest {
                group_id: space,
                expiration_timestamp: None,
                admitters: vec![],
                admitter_addrs: vec![],
            })
            .await
            .expect("the manager answers");

        assert!(refused_by_private_space(&nested.unwrap_err()));
        assert!(refused_by_private_space(&private.unwrap_err()));
        assert!(refused_by_private_space(&member.unwrap_err()));
        assert!(refused_by_private_space(&invitation.unwrap_err()));
        assert!(
            harness.published_topics().is_empty(),
            "a refused request reached the wire for {namespace:?}"
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
            .send(release_policy(namespace, TeeAdmissionMode::Replica))
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
    async fn a_mode_switch_leaves_a_tee_row_in_a_subgroup_it_cannot_seal_for() {
        let (store, mut harness, namespace, subgroup) = owned_namespace().await;
        let tee = AccountId::from([0xE3; 32]);
        let membership = MembershipRepository::new(&store);
        for group in [namespace, subgroup] {
            membership
                .add_member(&group, &tee, GroupMemberRole::ReadOnlyTee)
                .unwrap();
        }
        drop_keys(&store, subgroup);

        harness
            .manager
            .send(release_policy(namespace, TeeAdmissionMode::Relay))
            .await
            .expect("the manager answers")
            .expect("the policy is set");

        let role = |group| membership.role_of(&group, &tee).unwrap();
        assert_eq!(role(namespace), Some(GroupMemberRole::RelayTee));
        assert_eq!(
            role(subgroup),
            Some(GroupMemberRole::ReadOnlyTee),
            "a conversion nobody could be sent was applied here"
        );
        assert!(!harness.published_topics().is_empty());
    }

    /// The founder's node after she added Bob as an admin and handed him the namespace.
    struct Transferred {
        harness: actor::Harness,
        store: Store,
        namespace: ContextGroupId,
        alice: AccountId,
        bob: AccountId,
    }

    async fn transferred() -> Transferred {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let alice = NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the founder's root")
            .account();
        let bob = AccountId::from([0xB0; 32]);
        let harness = actor::over(store.clone()).await;
        let namespace = harness
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
            .expect("the namespace is created")
            .group_id;
        harness
            .manager
            .send(AddGroupMembersRequest {
                group_id: namespace,
                members: vec![(MemberIdentity::from(bob), GroupMemberRole::Admin)],
            })
            .await
            .expect("the manager answers")
            .expect("bob is added as an admin");
        harness
            .manager
            .send(TransferOwnershipRequest {
                group_id: namespace,
                new_owner: bob,
                root_proof: None,
            })
            .await
            .expect("the manager answers")
            .expect("the founder transfers ownership");
        Transferred {
            harness,
            store,
            namespace,
            alice,
            bob,
        }
    }

    /// A peer holding the namespace key that received every op of `from`, parents first.
    async fn replayed_on_a_peer(from: &Store, namespace: ContextGroupId) -> Store {
        let peer = Store::new(Arc::new(InMemoryDB::owned()));
        let (_, key) = GroupKeyring::new(from, namespace)
            .load_current_key()
            .expect("read the keyring")
            .expect("the namespace is keyed");
        let _ = GroupKeyring::new(&peer, namespace)
            .store_key(&key)
            .expect("the peer holds the key");
        let log = NamespaceOpLogService::new(from, namespace.to_bytes().into());
        let mut pending: Vec<SignedNamespaceOp> =
            ScopeProjections::ops_for_namespace(from, namespace.to_bytes())
                .expect("the founder's ops")
                .iter()
                .map(|op| {
                    log.get_signed_op(op.id())
                        .expect("read the op log")
                        .expect("every folded op is logged")
                })
                .collect();
        let applier = NamespaceGovernanceApplier::new(peer.clone());
        let mut applied = HashSet::new();
        while !pending.is_empty() {
            let (ready, rest): (Vec<_>, Vec<_>) = pending
                .into_iter()
                .partition(|op| op.parent_op_hashes.iter().all(|p| applied.contains(p)));
            assert!(!ready.is_empty(), "the log has a gap");
            for op in ready {
                let delta = signed_namespace_op_to_delta(&op).expect("build the delta");
                applier.apply(&delta).await.expect("the peer applies it");
                let _ = applied.insert(delta.id);
            }
            pending = rest;
        }
        peer
    }

    /// The founder hands the namespace to another admin and stays one, as the
    /// apply leaves her Admin row: she still lists its members and may leave it.
    #[actix::test]
    async fn a_founder_who_transfers_ownership_stays_an_admin() {
        let Transferred {
            harness,
            namespace,
            alice,
            bob,
            ..
        } = transferred().await;

        let members = harness
            .manager
            .send(ListGroupMembersRequest {
                group_id: namespace,
                offset: 0,
                limit: 10,
            })
            .await
            .expect("the manager answers")
            .expect("the former owner is still a member")
            .members;
        let roles: Vec<_> = members.into_iter().map(|m| (m.identity, m.role)).collect();
        assert!(
            roles.contains(&(alice, GroupMemberRole::Admin))
                && roles.contains(&(bob, GroupMemberRole::Admin)),
            "both stay admins after the transfer, got {roles:?}"
        );
        let listed = harness
            .manager
            .send(ListNamespacesRequest {
                offset: 0,
                limit: 10,
            })
            .await
            .expect("the manager answers")
            .expect("the namespaces list");
        assert!(listed.iter().any(|ns| ns.namespace_id == namespace));

        let _left = harness
            .manager
            .send(LeaveNamespaceRequest {
                namespace_id: namespace,
                purge: false,
            })
            .await
            .expect("the manager answers")
            .expect("the former owner may leave");
    }

    /// The new owner's node folds the same ops to the same members.
    #[actix::test]
    async fn a_peer_sees_the_former_owner_as_an_admin() {
        let Transferred {
            store,
            namespace,
            alice,
            bob,
            ..
        } = transferred().await;
        let peer = replayed_on_a_peer(&store, namespace).await;

        let (proj, ns, heads) = ScopeProjections::ephemeral_projection(&peer, &namespace)
            .expect("the peer folds the namespace");
        let mut members = proj
            .member_entries_with(&peer, ns, &namespace, &heads)
            .expect("the peer's cut is complete");
        members.sort_by_key(|(account, _)| *account);
        let mut expected = vec![
            (alice, GroupMemberRole::Admin),
            (bob, GroupMemberRole::Admin),
        ];
        expected.sort_by_key(|(account, _)| *account);
        assert_eq!(members, expected);
    }

    /// A founder who left cannot seat herself again by signing a second genesis: the
    /// apply logs it as a no-op on the established namespace, and the fold must too.
    #[actix::test]
    async fn a_founder_who_left_cannot_reseat_herself() {
        let Transferred {
            harness,
            store,
            namespace,
            alice,
            ..
        } = transferred().await;
        let (_, sk) = NamespaceRepository::new(&store)
            .identity(&namespace)
            .expect("read the identity")
            .expect("the founder signs here");
        let _left = harness
            .manager
            .send(LeaveNamespaceRequest {
                purge: false,
                namespace_id: namespace,
            })
            .await
            .expect("the manager answers")
            .expect("the former owner leaves");

        let log = NamespaceOpLogService::new(&store, namespace.to_bytes().into());
        let genesis = ScopeProjections::ops_for_namespace(&store, namespace.to_bytes())
            .expect("the namespace's ops")
            .into_iter()
            .find(|op| op.parents.is_empty())
            .and_then(|op| log.get_signed_op(op.id()).expect("read the op log"))
            .expect("the genesis is logged");
        let leave = ScopeProjections::ops_for_namespace(&store, namespace.to_bytes())
            .expect("the namespace's ops")
            .into_iter()
            .find(|op| matches!(op.payload, OpPayload::MemberLeft { .. }))
            .expect("the leave is logged")
            .id();
        // Governance ops tie on clock and generation in the streaming fold, so the op id
        // decides; a founder can grind the nonce until hers outranks the leave.
        let mut nonce = NamespaceGovernance::new(&store, namespace.to_bytes().into())
            .read_head_record()
            .expect("read the head")
            .next_nonce;
        let second = loop {
            let signed = SignedNamespaceOp::sign(
                &PrivateKey::from(sk),
                namespace.to_bytes().into(),
                vec![],
                nonce,
                genesis.op.clone(),
            )
            .expect("sign a second genesis");
            if signed_namespace_op_to_delta(&signed)
                .expect("build the delta")
                .id
                > leave
            {
                break signed;
            }
            nonce += 1;
        };
        NamespaceGovernanceApplier::new(store.clone())
            .apply(&signed_namespace_op_to_delta(&second).expect("build the delta"))
            .await
            .expect("an established namespace takes a second genesis as a no-op");

        let (proj, ns, heads) =
            ScopeProjections::ephemeral_projection(&store, &namespace).expect("fold the namespace");
        let members = proj
            .member_entries_with(&store, ns, &namespace, &heads)
            .expect("the cut is complete");
        assert!(
            members.iter().all(|(account, _)| *account != alice),
            "the leaver is seated again at the cut: {members:?}"
        );
        let ops = ScopeProjections::ops_for_namespace(&store, namespace.to_bytes())
            .expect("the namespace's ops");
        assert_eq!(ops.iter().filter(|op| op.parents.is_empty()).count(), 2);
        let streamed = calimero_projection::ScopeState::from_ops(&ops).acl_view();
        assert_eq!(
            streamed
                .groups
                .get(&namespace)
                .and_then(|rows| rows.get(&alice)),
            None,
            "the leaver is seated again in the streaming fold"
        );
    }
}
