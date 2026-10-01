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
    AttestFoundingRelayRequest, DelegatedGovernanceOp, GovernOnBehalfRequest,
    GovernOnBehalfResponse,
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
                        // Founding a namespace: this node mints its key, as the
                        // founder's node would, and listens on its topic before
                        // the genesis goes out.
                        let founding = matches!(op, RootOp::NamespaceCreatedV2 { .. });
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
                        if founding {
                            if let Err(err) =
                                node_client.subscribe_namespace(scope.to_bytes()).await
                            {
                                tracing::warn!(?err, %scope, "could not subscribe to the namespace being founded");
                            }
                        }
                        let minted = if created || founding {
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

impl Handler<AttestFoundingRelayRequest> for ContextManager {
    type Result = ActorResponse<Self, <AttestFoundingRelayRequest as Message>::Result>;

    fn handle(
        &mut self,
        AttestFoundingRelayRequest {
            namespace_id,
            account,
            evidence,
            release_version,
        }: AttestFoundingRelayRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let Some((_pk, sk_bytes)) = self.node_signing_key(&namespace_id) else {
            return ActorResponse::reply(Err(crate::error::ContextError::NotAGroupMember {
                group_id: namespace_id.to_string(),
            }
            .into()));
        };
        let sk = PrivateKey::from(sk_bytes);
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        ActorResponse::r#async(
            async move {
                let (profile, mock) =
                    founding_profile(&sk.public_key(), &evidence, &release_version).await?;
                let report = calimero_governance_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &namespace_id,
                    &sk,
                    GroupOp::FoundingRelayAttested {
                        account,
                        quote: evidence.quote,
                        collateral: evidence.collateral,
                        attested_at: evidence.attested_at,
                        release_version,
                        profile,
                        mock,
                    },
                )
                .await?;
                report.observe("attest_founding_relay", "FoundingRelayAttested");
                info!(%namespace_id, "admitted this relay as the founded namespace's first TEE");
                Ok(())
            }
            .into_actor(self),
        )
    }
}

/// The profile a mock quote is recorded under. Its registers match no published
/// release, and a policy judges mock quotes on `accept_mock` alone, so the name
/// only labels the policy.
const MOCK_PROFILE: &str = "locked-read-only";

/// Which profile of `release_version` the quote's measurements match, and
/// whether it is a mock — read the way an admitter reads any TEE's claim, from
/// the signed release. Runs on the context actor, where the release fetch's
/// non-`Send` verifier may be awaited.
async fn founding_profile(
    identity: &calimero_primitives::identity::PublicKey,
    evidence: &calimero_context_client::group::TeeAuthorityEvidencePayload,
    release_version: &str,
) -> eyre::Result<(String, bool)> {
    use sha2::{Digest, Sha256};

    let key_hash: [u8; 32] = Sha256::digest(**identity).into();
    let collateral = evidence
        .collateral
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()?;
    let verdict = calimero_tee_attestation::verify_evidence(
        &evidence.quote,
        collateral.as_ref(),
        evidence.attested_at,
        &key_hash,
    )
    .map_err(|err| eyre::eyre!("this node's quote does not verify: {err}"))?;
    if verdict.is_mock {
        return Ok((MOCK_PROFILE.to_owned(), true));
    }
    let release = calimero_tee_release::fetch_node_release(release_version)
        .await
        .map_err(|err| eyre::eyre!("could not read signed release {release_version}: {err:#}"))?;
    let names: Vec<String> = release.profiles.keys().cloned().collect();
    let profile = release
        .matching_profile(
            &names,
            &verdict.mrtd,
            &verdict.rtmr0,
            &verdict.rtmr1,
            &verdict.rtmr2,
            &verdict.rtmr3,
        )
        .ok_or_else(|| {
            eyre::eyre!("this node's measurements match no profile of release {release_version}")
        })?;
    Ok((profile.to_owned(), false))
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
        fixture_with(author_caps, false).await
    }

    /// With `relay_joins`, the relay enters the namespace the way a real one
    /// does — by publishing an invitation join — so the governance projection
    /// knows its device and its root membership, rather than only the rows.
    async fn fixture_with(author_caps: MemberCapabilities, relay_joins: bool) -> Fixture {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("account root");
        let harness = actor::over(store.clone()).await;
        let ns = ContextGroupId::from(NS);

        let admin_sk = PrivateKey::from([0x11; 32]);
        let admin = enrol(&store, &ns, &admin_sk.public_key());
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
        let relay = if relay_joins {
            join_namespace(&store, &ns, &admin_sk, admin, &relay_sk)
        } else {
            let relay = enrol_holder(&store, &ns, &relay_pk);
            membership
                .add_member(&ns, &relay, GroupMemberRole::Member)
                .expect("relay");
            relay
        };
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

    /// `joiner_sk`'s node joins `ns` as a `Member` by an invitation `admin_sk`
    /// signed, through the governance apply, and the account it joined as.
    fn join_namespace(
        store: &Store,
        ns: &ContextGroupId,
        admin_sk: &PrivateKey,
        admin: AccountId,
        joiner_sk: &PrivateKey,
    ) -> AccountId {
        use calimero_context_client::local_governance::SignedNamespaceOp;
        use calimero_context_config::types::{
            GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
        };
        use sha2::{Digest, Sha256};

        let nonce = [0x42; 32];
        let invitation = GroupInvitationFromAdmin {
            inviter_identity: SignerId::from(*admin_sk.public_key().digest()),
            group_id: *ns,
            expiration_timestamp: 0,
            invitation_nonce: nonce,
            invited_role: 1,
            admitters: vec![admin],
        };
        let signature = admin_sk
            .sign(&Sha256::digest(
                borsh::to_vec(&invitation).expect("encode invitation"),
            ))
            .expect("sign invitation");
        let account = crate::join_credential::build(store, ns, &joiner_sk.public_key())
            .expect("the joiner's credential");
        let member = account.statement.account;
        let join = RootOp::MemberJoinedAt {
            member,
            signed_invitation: SignedGroupOpenInvitation {
                inviter_account: None,
                invitation,
                inviter_signature: hex::encode(signature.to_bytes()),
                application_id: None,
                bytecode_id: None,
                admitter_addrs: Vec::new(),
            },
            joined_at: 1,
            account,
        };
        let parents =
            calimero_governance_store::NamespaceDagService::new(store, ns.to_bytes().into())
                .read_head_record()
                .expect("read the governance head")
                .parent_hashes;
        let mut signed = SignedNamespaceOp::sign(
            joiner_sk,
            ns.to_bytes().into(),
            parents,
            1,
            crate::test_support::published_join(store, ns, join),
        )
        .expect("sign the join");
        signed.admitter_endorsement = Some(Box::new(
            calimero_governance_types::AdmitterEndorsement::sign(
                admin_sk,
                &ns.to_bytes(),
                &member,
                &nonce,
            )
            .expect("endorse the join"),
        ));
        let _ = calimero_governance_store::apply_signed_namespace_op(store, &signed)
            .expect("the join applies");
        member
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

        /// The author creates a Restricted subgroup under the root, salted
        /// `[tag; 32]`, with the id that create derives.
        async fn create_subgroup(&self, tag: u8) -> eyre::Result<ContextGroupId> {
            let salt = [tag; 32];
            let op = RootOp::GroupCreated {
                group_id: calimero_account::created_subgroup_id(&self.author, &NS, true, &salt)
                    .into(),
                parent_id: NS.into(),
                restricted: true,
                admin: self.author,
                salt,
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
        let dm = fx.create_subgroup(0xD1).await.expect("create the DM");
        assert_eq!(
            dm,
            ContextGroupId::from(calimero_account::created_subgroup_id(
                &fx.author,
                &NS,
                true,
                &[0xD1; 32],
            )),
            "the id is the one the author's create derives"
        );

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

    /// The relay reads a subgroup it was seated in by creating it.
    ///
    /// The read gates answer from the governance projection, so the seat has to
    /// be in the fold as well as in the rows: with the member's own additions
    /// folded for the subgroup, a projection that never learned of the seat
    /// answered "node is not a member of group" to the node that holds `512`
    /// there.
    #[actix::test]
    async fn the_relay_reads_a_subgroup_it_created_for_a_member() {
        let fx = fixture_with(MemberCapabilities::CAN_CREATE_SUBGROUP, true).await;
        let dm = fx.create_subgroup(0xD3).await.expect("create the DM");
        fx.add(dm, fx.other).await.expect("add the other person");

        let members = fx
            .harness
            .manager
            .send(calimero_context_client::group::ListGroupMembersRequest {
                group_id: dm,
                offset: 0,
                limit: 10,
            })
            .await
            .expect("the manager answers")
            .expect("the seated relay may list the members");
        // Only the relay entered the namespace by an op here; the author and the
        // other person are rows alone, which the projection's enumeration does
        // not see. The relay's own seat is the point.
        assert!(
            members
                .members
                .iter()
                .any(|entry| entry.identity == fx.relay && entry.role == GroupMemberRole::Member),
            "the relay is listed as the member it was seated as"
        );

        let _info = fx
            .harness
            .manager
            .send(calimero_context_client::group::GetGroupInfoRequest { group_id: dm })
            .await
            .expect("the manager answers")
            .expect("the seated relay may read the group");
    }

    /// A refused creation leaves no key behind for a subgroup that does not
    /// exist.
    #[actix::test]
    async fn a_refused_creation_leaves_nothing_behind() {
        let fx = fixture(MemberCapabilities::empty()).await;
        let gid = ContextGroupId::from(calimero_account::created_subgroup_id(
            &fx.author,
            &NS,
            true,
            &[0xD2; 32],
        ));
        let _refused = fx
            .create_subgroup(0xD2)
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

    /// The founding relay reads the namespace it was seated in, once the
    /// namespace has members of its own folded — the same gap as a subgroup's
    /// creating relay, for the seat the founding gives.
    #[actix::test]
    async fn the_founding_relay_reads_the_namespace_it_founded() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("account root");
        let harness = actor::over(store.clone()).await;

        let author_sk = PrivateKey::from([0x44; 32]);
        let author_proof = credential(&author_sk.public_key());
        let author = author_proof.statement.account;
        let salt = [0x5D; 32];
        let ns = ContextGroupId::from(calimero_account::founded_namespace_id(&author, &salt));
        let (_ns, relay_pk, _sk) = NamespaceRepository::new(&store)
            .participate_in(&ns)
            .expect("the relay takes an identity in the new namespace");
        let executor_proof =
            crate::join_credential::build(&store, &ns, &relay_pk).expect("relay credential");
        let relay = executor_proof.statement.account;

        let delegation = |kind, form: &[u8], nonce| GovernanceDelegation {
            warrant: Box::new(
                GovernanceWarrant::sign(
                    &author_sk,
                    GovernanceTerms {
                        scope: ns.to_bytes(),
                        kind,
                        author_account: author,
                        executor: relay,
                        op_hash: GovernanceWarrant::op_hash(kind, form),
                        account_heads: vec![],
                        governance_floor: vec![],
                        nonce,
                        not_after: u64::MAX,
                    },
                )
                .expect("sign"),
            ),
            author_proof: author_proof.clone(),
            executor_proof: executor_proof.clone(),
            executor_key: relay_pk,
        };

        let genesis = RootOp::NamespaceCreatedV2 {
            founder: author,
            account: author_proof.clone(),
            salt,
        };
        let form = borsh::to_vec(&genesis).expect("encode");
        let _ = harness
            .context_client
            .govern_on_behalf(GovernOnBehalfRequest {
                delegation: delegation(GovernanceOpKind::Root, &form, 0),
                op: DelegatedGovernanceOp::Root { op: genesis },
            })
            .await
            .expect("founded");

        let other = crate::test_support::account_for(&PrivateKey::from([0x55; 32]).public_key());
        let add = GroupOp::MemberAdded {
            member: other,
            role: GroupMemberRole::Member,
        };
        let form = borsh::to_vec(&add).expect("encode");
        let _ = harness
            .context_client
            .govern_on_behalf(GovernOnBehalfRequest {
                delegation: delegation(GovernanceOpKind::Group, &form, 1),
                op: DelegatedGovernanceOp::Group {
                    group_id: ns,
                    op: add,
                },
            })
            .await
            .expect("the founder adds a member");

        let members = harness
            .manager
            .send(calimero_context_client::group::ListGroupMembersRequest {
                group_id: ns,
                offset: 0,
                limit: 10,
            })
            .await
            .expect("the manager answers")
            .expect("the founding relay may list the members");
        let role_of = |who: &AccountId| {
            members
                .members
                .iter()
                .find(|entry| entry.identity == *who)
                .map(|entry| entry.role.clone())
        };
        assert_eq!(role_of(&relay), Some(GroupMemberRole::Member));
        assert_eq!(role_of(&other), Some(GroupMemberRole::Member));

        let _info = harness
            .manager
            .send(calimero_context_client::group::GetGroupInfoRequest { group_id: ns })
            .await
            .expect("the manager answers")
            .expect("the founding relay may read the namespace");
    }

    /// A member founds a namespace through the relay: the relay takes an
    /// identity in it, publishes the genesis as the member, is seated to serve
    /// it, then admits itself as the namespace's first TEE with a (mock) quote.
    #[actix::test]
    async fn a_namespace_is_founded_through_the_relay_with_tees_on() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("account root");
        let harness = actor::over(store.clone()).await;

        let author_sk = PrivateKey::from([0x44; 32]);
        let author_proof = credential(&author_sk.public_key());
        let author = author_proof.statement.account;
        let salt = [0x5C; 32];
        let ns = ContextGroupId::from(calimero_account::founded_namespace_id(&author, &salt));

        let (_ns, relay_pk, _sk) = NamespaceRepository::new(&store)
            .participate_in(&ns)
            .expect("the relay takes an identity in the new namespace");
        let executor_proof =
            crate::join_credential::build(&store, &ns, &relay_pk).expect("relay credential");
        let relay = executor_proof.statement.account;

        let genesis = RootOp::NamespaceCreatedV2 {
            founder: author,
            account: author_proof.clone(),
            salt,
        };
        let form = borsh::to_vec(&genesis).expect("encode");
        let delegation = GovernanceDelegation {
            warrant: Box::new(
                GovernanceWarrant::sign(
                    &author_sk,
                    GovernanceTerms {
                        scope: ns.to_bytes(),
                        kind: GovernanceOpKind::Root,
                        author_account: author,
                        executor: relay,
                        op_hash: GovernanceWarrant::op_hash(GovernanceOpKind::Root, &form),
                        account_heads: vec![],
                        governance_floor: vec![],
                        nonce: 0,
                        not_after: u64::MAX,
                    },
                )
                .expect("sign"),
            ),
            author_proof,
            executor_proof: executor_proof.clone(),
            executor_key: relay_pk,
        };
        let response = harness
            .context_client
            .govern_on_behalf(GovernOnBehalfRequest {
                delegation,
                op: DelegatedGovernanceOp::Root { op: genesis },
            })
            .await
            .expect("founded");
        assert_eq!(response.group_id, ns);

        let meta = MetaRepository::new(&store)
            .load(&ns)
            .expect("read")
            .expect("founded");
        assert_eq!(meta.owner_identity, author, "the member owns it");
        assert_eq!(meta.admin_identity, author, "and administers it");
        assert!(
            GroupKeyring::new(&store, ns)
                .load_current_key()
                .expect("read")
                .is_some(),
            "the relay minted the namespace key"
        );
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&ns, &relay)
                .expect("read"),
            Some(GroupMemberRole::Member)
        );

        use sha2::{Digest, Sha256};
        let key_hash: [u8; 32] = Sha256::digest(*relay_pk).into();
        let quote = calimero_tee_attestation::generate_mock_attestation(
            calimero_tee_attestation::build_report_data(&[0x07; 32], Some(&key_hash)),
        )
        .quote_bytes;
        harness
            .context_client
            .attest_founding_relay(calimero_context_client::group::AttestFoundingRelayRequest {
                namespace_id: ns,
                account: executor_proof,
                evidence: calimero_context_client::group::TeeAuthorityEvidencePayload {
                    quote,
                    collateral: None,
                    attested_at: 1_700_000_000,
                },
                release_version: "mock".to_owned(),
            })
            .await
            .expect("the founding relay attests");
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&ns, &relay)
                .expect("read"),
            Some(GroupMemberRole::RelayTee),
            "and is the namespace's first TEE"
        );
        let calimero_governance_store::TeeAdmissionPolicyRead::Set(policy) =
            calimero_governance_store::read_tee_admission_policy(&store, &ns).expect("read")
        else {
            panic!("a TEE admission policy is set by default");
        };
        assert_eq!(
            policy.mode,
            calimero_context_client::local_governance::TeeAdmissionMode::Relay
        );
    }

    /// In a namespace founded through the relay, with the relay attested as its
    /// first TEE, a subgroup the member creates through the relay has the relay
    /// in it as the `RelayTee` it is at the root — holding the key it minted —
    /// so the member's group ops on the subgroup go through it, and it serves
    /// the subgroup's reads. There is no admin node in such a namespace, and the
    /// subgroup TEE fan-in finds no verdict for a founding relay, so before this
    /// the relay was in no subgroup and refused every such op.
    #[actix::test]
    async fn a_subgroup_of_a_relay_founded_namespace_has_the_relay_as_relay_tee() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("account root");
        let harness = actor::over(store.clone()).await;

        let author_sk = PrivateKey::from([0x44; 32]);
        let author_proof = credential(&author_sk.public_key());
        let author = author_proof.statement.account;
        let salt = [0x5E; 32];
        let ns = ContextGroupId::from(calimero_account::founded_namespace_id(&author, &salt));
        let (_ns, relay_pk, _sk) = NamespaceRepository::new(&store)
            .participate_in(&ns)
            .expect("the relay takes an identity in the new namespace");
        let executor_proof =
            crate::join_credential::build(&store, &ns, &relay_pk).expect("relay credential");
        let relay = executor_proof.statement.account;

        let delegation = |scope: ContextGroupId, kind, form: &[u8], nonce| GovernanceDelegation {
            warrant: Box::new(
                GovernanceWarrant::sign(
                    &author_sk,
                    GovernanceTerms {
                        scope: scope.to_bytes(),
                        kind,
                        author_account: author,
                        executor: relay,
                        op_hash: GovernanceWarrant::op_hash(kind, form),
                        account_heads: vec![],
                        governance_floor: vec![],
                        nonce,
                        not_after: u64::MAX,
                    },
                )
                .expect("sign"),
            ),
            author_proof: author_proof.clone(),
            executor_proof: executor_proof.clone(),
            executor_key: relay_pk,
        };

        let genesis = RootOp::NamespaceCreatedV2 {
            founder: author,
            account: author_proof.clone(),
            salt,
        };
        let form = borsh::to_vec(&genesis).expect("encode");
        let _ = harness
            .context_client
            .govern_on_behalf(GovernOnBehalfRequest {
                delegation: delegation(ns, GovernanceOpKind::Root, &form, 0),
                op: DelegatedGovernanceOp::Root { op: genesis },
            })
            .await
            .expect("founded");

        use sha2::{Digest, Sha256};
        let key_hash: [u8; 32] = Sha256::digest(*relay_pk).into();
        let quote = calimero_tee_attestation::generate_mock_attestation(
            calimero_tee_attestation::build_report_data(&[0x07; 32], Some(&key_hash)),
        )
        .quote_bytes;
        harness
            .context_client
            .attest_founding_relay(calimero_context_client::group::AttestFoundingRelayRequest {
                namespace_id: ns,
                account: executor_proof.clone(),
                evidence: calimero_context_client::group::TeeAuthorityEvidencePayload {
                    quote,
                    collateral: None,
                    attested_at: 1_700_000_000,
                },
                release_version: "mock".to_owned(),
            })
            .await
            .expect("the founding relay attests");

        let create_salt = [0xD7; 32];
        let create = RootOp::GroupCreated {
            group_id: calimero_account::created_subgroup_id(
                &author,
                &ns.to_bytes(),
                true,
                &create_salt,
            )
            .into(),
            parent_id: ns.to_bytes().into(),
            restricted: true,
            admin: author,
            salt: create_salt,
        };
        let form = borsh::to_vec(&create).expect("encode");
        let sub = harness
            .context_client
            .govern_on_behalf(GovernOnBehalfRequest {
                delegation: delegation(ns, GovernanceOpKind::Root, &form, 1),
                op: DelegatedGovernanceOp::Root { op: create },
            })
            .await
            .expect("the member creates a subgroup through the relay")
            .group_id;

        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&sub, &relay)
                .expect("read"),
            Some(GroupMemberRole::RelayTee),
            "the relay is in the subgroup with its attested role"
        );
        assert!(
            GroupKeyring::new(&store, sub)
                .load_current_key()
                .expect("read")
                .is_some(),
            "and holds the subgroup's key, which it minted"
        );

        let rename = GroupOp::GroupMetadataSet {
            name: Some("general".to_owned()),
            data: std::collections::BTreeMap::new(),
        };
        let form = borsh::to_vec(&rename).expect("encode");
        let _ = harness
            .context_client
            .govern_on_behalf(GovernOnBehalfRequest {
                delegation: delegation(sub, GovernanceOpKind::Group, &form, 2),
                op: DelegatedGovernanceOp::Group {
                    group_id: sub,
                    op: rename,
                },
            })
            .await
            .expect("a delegated group op on the subgroup is admitted");

        let members = harness
            .manager
            .send(calimero_context_client::group::ListGroupMembersRequest {
                group_id: sub,
                offset: 0,
                limit: 10,
            })
            .await
            .expect("the manager answers")
            .expect("the relay may list the subgroup's members");
        assert!(
            members
                .members
                .iter()
                .any(|entry| entry.identity == relay && entry.role == GroupMemberRole::RelayTee),
            "the projection seats it as the RelayTee the rows do"
        );
    }
}
