//! Publish a governance op on a member's behalf, under their signed warrant.
//!
//! The member — an account with no node — signs the op in its delegable form;
//! this node, the relay, completes what only a publisher can (a removal's
//! post-state hashes, a cascade delete's enumerated subtree), wraps it in
//! `OnBehalf`, and publishes it with its own key. Every peer then applies the
//! inner op as the member, so the member's authority decides and this node's
//! does not.
//!
//! What this node adds beyond publishing is what the node that performs an op
//! always adds, because it is the one holding the keys: the group key goes to a
//! member it adds, and a subgroup it creates gets a key of its own.

use std::collections::BTreeMap;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{
    DelegatedGovernanceOp, GovernOnBehalfRequest, GovernOnBehalfResponse,
};
use calimero_context_client::local_governance::{GroupOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{AccountBindingRepository, GroupKeyring, NamespaceRepository};
use calimero_primitives::identity::PrivateKey;
use rand::RngExt as _;
use tracing::info;

use crate::ContextManager;

impl Handler<GovernOnBehalfRequest> for ContextManager {
    type Result = ActorResponse<Self, <GovernOnBehalfRequest as Message>::Result>;

    fn handle(
        &mut self,
        GovernOnBehalfRequest { delegation, op }: GovernOnBehalfRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let scope = ContextGroupId::from(delegation.warrant.scope);
        // The relay signs as its identity in the namespace, which is the key
        // the warrant's executor certificate has to name.
        let Some((_pk, sk_bytes)) = self.node_signing_key(&scope) else {
            return ActorResponse::reply(Err(crate::error::ContextError::NotAGroupMember {
                group_id: scope.to_string(),
            }
            .into()));
        };
        let sk = PrivateKey::from(sk_bytes);
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                match op {
                    DelegatedGovernanceOp::Group { group_id, op } => {
                        if group_id != scope {
                            eyre::bail!(
                                "the governance warrant is for group {scope}, not {group_id}"
                            );
                        }
                        let added = match &op {
                            GroupOp::MemberAdded { member, .. } => Some(*member),
                            _ => None,
                        };
                        let label = op.op_kind_label();
                        let report = calimero_governance_store::sign_apply_and_publish_on_behalf(
                            &datastore,
                            &node_client,
                            &ack_router,
                            &group_id,
                            &sk,
                            op,
                            delegation,
                        )
                        .await?;
                        report.observe("govern_on_behalf", label);

                        if let Some(member) = added {
                            let ns_id = NamespaceRepository::new(&datastore).resolve(&group_id)?;
                            let devices: BTreeMap<_, _> = AccountBindingRepository::new(&datastore)
                                .live_devices_by_account(&ns_id)?;
                            super::add_group_members::deliver_group_key(
                                &datastore,
                                &node_client,
                                &ack_router,
                                &ns_id,
                                &group_id,
                                &sk,
                                member,
                                None,
                                &devices,
                            )
                            .await?;
                        }
                        info!(%group_id, op = label, "published a governance op on a member's behalf");
                        Ok(GovernOnBehalfResponse { group_id })
                    }
                    DelegatedGovernanceOp::Root { op } => {
                        let namespace_id = NamespaceRepository::new(&datastore).resolve(&scope)?;
                        if namespace_id != scope {
                            eyre::bail!(
                                "a root op is published on the namespace, not on subgroup {scope}"
                            );
                        }
                        let (op, target) = match op {
                            // The subtree is this node's to enumerate; every peer
                            // re-enumerates and refuses a mismatch.
                            RootOp::GroupDeleted { root_group_id, .. } => {
                                let payload = NamespaceRepository::new(&datastore)
                                    .collect_subtree_for_cascade(&root_group_id)?;
                                (
                                    RootOp::GroupDeleted {
                                        root_group_id,
                                        cascade_group_ids: payload.descendant_groups,
                                        cascade_context_ids: payload.contexts,
                                    },
                                    root_group_id,
                                )
                            }
                            RootOp::GroupCreated { group_id, .. }
                            | RootOp::GroupReparented {
                                child_group_id: group_id,
                                ..
                            } => (op, group_id),
                            other => (other, scope),
                        };
                        let created = matches!(op, RootOp::GroupCreated { .. });
                        // Refused on apply too; checked first so no key is minted
                        // into an existing group's keyring.
                        if created
                            && calimero_governance_store::MetaRepository::new(&datastore)
                                .load(&target)?
                                .is_some()
                        {
                            return Err(calimero_governance_store::delegation_gate::DelegationRefusal::GroupAlreadyExists(
                                target.to_string(),
                            )
                            .into());
                        }
                        // A new subgroup's key, minted by this node — the one
                        // creating it, since the member holds no node — and stored
                        // BEFORE the creation applies, as a self-created subgroup's
                        // is: the apply fires `SubgroupCreated`, and the TEE
                        // admission that reacts to it runs only on a node holding
                        // the subgroup's key.
                        let minted = if created {
                            let group_key: [u8; 32] = rand::rng().random();
                            Some(GroupKeyring::new(&datastore, target).store_key(&group_key)?)
                        } else {
                            None
                        };
                        let sealed = calimero_governance_store::seal_root_op_for_publish(
                            &datastore,
                            namespace_id.to_bytes().into(),
                            RootOp::OnBehalf {
                                op: Box::new(op),
                                delegation: Box::new(delegation),
                            },
                        )?;
                        let published = calimero_governance_store::sign_apply_and_publish_namespace_op(
                            &datastore,
                            &node_client,
                            &ack_router,
                            namespace_id.to_bytes().into(),
                            &sk,
                            sealed,
                        )
                        .await;
                        let report = match published {
                            Ok(report) => report,
                            Err(err) => {
                                // Refused: drop the key minted for a subgroup that
                                // does not exist, so a retry starts clean.
                                if let Some(key_id) = minted {
                                    let _ignored = GroupKeyring::new(&datastore, target)
                                        .delete_key_by_id(&key_id);
                                }
                                return Err(err);
                            }
                        };
                        report.observe("govern_on_behalf", "RootOnBehalf");
                        info!(group_id = %target, "published a root op on a member's behalf");
                        Ok(GovernOnBehalfResponse { group_id: target })
                    }
                }
            }
            .into_actor(self),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::{
        AccountId, GovernanceDelegation, GovernanceOpKind, GovernanceTerms, GovernanceWarrant,
    };
    use calimero_context_client::group::{DelegatedGovernanceOp, GovernOnBehalfRequest};
    use calimero_context_client::local_governance::{GroupOp, RootOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::MemberCapabilities;
    use calimero_governance_store::{
        CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository,
        NamespaceRepository, NodeDeviceRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupTarget};
    use calimero_store::Store;

    use crate::test_support::{actor, credential, enrol, enrol_holder};

    const NS: [u8; 32] = [0x7A; 32];

    struct Fixture {
        harness: actor::Harness,
        store: Store,
        ns: ContextGroupId,
        author_sk: PrivateKey,
        author: AccountId,
        relay_pk: calimero_primitives::identity::PublicKey,
        relay: AccountId,
        other: AccountId,
        nonce: std::cell::Cell<u64>,
    }

    async fn fixture(author_caps: MemberCapabilities) -> Fixture {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("account root");
        let harness = actor::over(store.clone()).await;
        let ns = ContextGroupId::from(NS);

        let admin = enrol(&store, &ns, &PrivateKey::from([0x11; 32]).public_key());
        MetaRepository::new(&store)
            .save(
                &ns,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: [0xAA; 32].into(),
                        bytecode_id: [0xBB; 32],
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 1_700_000_000,
                    admin_identity: admin,
                    owner_identity: admin,
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("meta");
        let _ = GroupKeyring::new(&store, ns)
            .store_key(&[0x33; 32])
            .expect("namespace key");

        let membership = MembershipRepository::new(&store);
        membership
            .add_member(&ns, &admin, GroupMemberRole::Admin)
            .expect("admin");

        let author_sk = PrivateKey::from([0x44; 32]);
        let author = enrol(&store, &ns, &author_sk.public_key());
        membership
            .add_member(&ns, &author, GroupMemberRole::Member)
            .expect("author");
        CapabilitiesRepository::new(&store)
            .set_member_capability(&ns, &author, author_caps.bits())
            .expect("author caps");

        let other = enrol(&store, &ns, &PrivateKey::from([0x55; 32]).public_key());
        membership
            .add_member(&ns, &other, GroupMemberRole::Member)
            .expect("other");

        let relay_sk = PrivateKey::from([0x22; 32]);
        let relay_pk = relay_sk.public_key();
        NamespaceRepository::new(&store)
            .replace_identity(&ns, &relay_pk, relay_sk.as_bytes())
            .expect("relay identity");
        let relay = enrol_holder(&store, &ns, &relay_pk);
        membership
            .add_member(&ns, &relay, GroupMemberRole::Member)
            .expect("relay");
        CapabilitiesRepository::new(&store)
            .set_member_capability(&ns, &relay, MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits())
            .expect("relay caps");

        Fixture {
            harness,
            store,
            ns,
            author_sk,
            author,
            relay_pk,
            relay,
            other,
            nonce: std::cell::Cell::new(0),
        }
    }

    impl Fixture {
        fn delegation(
            &self,
            scope: ContextGroupId,
            kind: GovernanceOpKind,
            form: &[u8],
        ) -> GovernanceDelegation {
            self.nonce.set(self.nonce.get() + 1);
            GovernanceDelegation {
                warrant: Box::new(
                    GovernanceWarrant::sign(
                        &self.author_sk,
                        GovernanceTerms {
                            scope: scope.to_bytes(),
                            kind,
                            author_account: self.author,
                            executor: self.relay,
                            op_hash: GovernanceWarrant::op_hash(kind, form),
                            account_heads: vec![],
                            governance_floor: vec![],
                            nonce: self.nonce.get(),
                            not_after: u64::MAX,
                        },
                    )
                    .expect("sign"),
                ),
                author_proof: credential(&self.author_sk.public_key()),
                executor_proof: crate::join_credential::build(
                    &self.store,
                    &self.ns,
                    &self.relay_pk,
                )
                .expect("relay credential"),
                executor_key: self.relay_pk,
            }
        }

        async fn create_subgroup(&self, id: [u8; 32]) -> eyre::Result<ContextGroupId> {
            let op = RootOp::GroupCreated {
                group_id: id.into(),
                parent_id: NS.into(),
                restricted: true,
                admin: self.author,
            };
            let form = borsh::to_vec(&op).expect("encode");
            let response = self
                .harness
                .context_client
                .govern_on_behalf(GovernOnBehalfRequest {
                    delegation: self.delegation(self.ns, GovernanceOpKind::Root, &form),
                    op: DelegatedGovernanceOp::Root { op },
                })
                .await?;
            Ok(response.group_id)
        }

        async fn add(&self, group: ContextGroupId, member: AccountId) -> eyre::Result<()> {
            let op = GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            };
            let form = borsh::to_vec(&op).expect("encode");
            let _ = self
                .harness
                .context_client
                .govern_on_behalf(GovernOnBehalfRequest {
                    delegation: self.delegation(group, GovernanceOpKind::Group, &form),
                    op: DelegatedGovernanceOp::Group {
                        group_id: group,
                        op,
                    },
                })
                .await?;
            Ok(())
        }
    }

    /// The whole DM flow through the relay: create a Restricted subgroup owned
    /// by the member, with the relay seated to serve it and holding its key;
    /// then add the other person.
    #[actix::test]
    async fn a_dm_is_created_and_populated_through_the_relay() {
        let mut fx = fixture(MemberCapabilities::CAN_CREATE_SUBGROUP).await;
        let dm = fx.create_subgroup([0xD1; 32]).await.expect("create the DM");
        assert_eq!(dm, ContextGroupId::from([0xD1; 32]));

        let meta = MetaRepository::new(&fx.store)
            .load(&dm)
            .expect("read")
            .expect("created");
        assert_eq!(meta.owner_identity, fx.author, "the member owns it");
        assert!(MembershipRepository::new(&fx.store)
            .is_admin(&dm, &fx.author)
            .expect("read"));
        assert_eq!(
            MembershipRepository::new(&fx.store)
                .role_of(&dm, &fx.relay)
                .expect("read"),
            Some(GroupMemberRole::Member),
            "the relay is seated to serve it"
        );
        assert!(
            GroupKeyring::new(&fx.store, dm)
                .load_current_key()
                .expect("read")
                .is_some(),
            "and holds the subgroup's key"
        );

        fx.add(dm, fx.other).await.expect("add the other person");
        assert!(MembershipRepository::new(&fx.store)
            .is_member(&dm, &fx.other)
            .expect("read"));
        assert!(
            !fx.harness.broadcast_topics().is_empty(),
            "published for peers, not only applied here"
        );
    }

    /// A refused creation leaves no key behind for a subgroup that does not
    /// exist.
    #[actix::test]
    async fn a_refused_creation_leaves_nothing_behind() {
        let fx = fixture(MemberCapabilities::empty()).await;
        let gid = ContextGroupId::from([0xD2; 32]);
        let _refused = fx
            .create_subgroup([0xD2; 32])
            .await
            .expect_err("the member may not create subgroups");
        assert!(MetaRepository::new(&fx.store)
            .load(&gid)
            .expect("read")
            .is_none());
        assert!(
            GroupKeyring::new(&fx.store, gid)
                .load_current_key()
                .expect("read")
                .is_none(),
            "the minted key is dropped"
        );
    }

    /// The member's own authority decides: a member with no MANAGE_MEMBERS in
    /// the namespace cannot add someone to it through the relay.
    #[actix::test]
    async fn adding_needs_the_members_own_right() {
        let fx = fixture(MemberCapabilities::empty()).await;
        let newcomer = crate::test_support::account_for(&PrivateKey::from([0x66; 32]).public_key());
        let _refused = fx
            .add(fx.ns, newcomer)
            .await
            .expect_err("no MANAGE_MEMBERS");
        assert!(!MembershipRepository::new(&fx.store)
            .is_member(&fx.ns, &newcomer)
            .expect("read"));
    }

    #[actix::test]
    async fn a_warrant_for_one_group_is_not_spent_in_another() {
        let fx = fixture(MemberCapabilities::MANAGE_MEMBERS).await;
        let op = GroupOp::MemberAdded {
            member: fx.other,
            role: GroupMemberRole::Member,
        };
        let form = borsh::to_vec(&op).expect("encode");
        let err = fx
            .harness
            .context_client
            .govern_on_behalf(GovernOnBehalfRequest {
                delegation: fx.delegation(fx.ns, GovernanceOpKind::Group, &form),
                op: DelegatedGovernanceOp::Group {
                    group_id: ContextGroupId::from([0x99; 32]),
                    op,
                },
            })
            .await
            .expect_err("refused");
        assert!(
            err.to_string().contains("governance warrant is for group"),
            "{err}"
        );
    }
}
