//! Adopting a group key pulled directly from a peer.
//!
//! `apply_received_group_key` re-drives what a `KeyDelivery` op unlocks; the
//! direct stream pulls (`request_namespace_join`, `request_open_subgroup_join`)
//! carry a key too and need the same recovery.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
use calimero_store::Store;
use tracing::warn;

use crate::scope_projection::ScopeProjections;

/// Store a peer-pulled group key and recover everything that was waiting on it.
///
/// A governance op applied before its key arrived is effect-skipped in the live
/// store and frozen as a `Noop` in the unified op-store, so both planes need
/// recovering: the re-drive re-applies the op, the re-persist re-decodes it.
///
/// Both recoveries are best-effort. The key is stored either way, and the
/// key-delivery and startup sweeps retry what fails here.
pub(crate) fn adopt_pulled_group_key(
    store: &Store,
    namespace_id: NamespaceId,
    group_id: ContextGroupId,
    group_key: &[u8; 32],
) -> eyre::Result<[u8; 32]> {
    let key_id =
        calimero_governance_store::GroupKeyring::new(store, group_id).store_key(group_key)?;
    let authorizer = crate::apply_authorizer::EphemeralProjectionAuthorizer::new(store);
    if let Err(err) = calimero_governance_store::retry_encrypted_ops_for_group_with(
        store,
        namespace_id,
        group_id.to_bytes(),
        &authorizer,
    ) {
        warn!(
            group_id = %hex::encode(group_id.to_bytes()),
            %err,
            "failed to re-drive buffered encrypted ops after a direct group-key pull"
        );
    }
    ScopeProjections::repersist_namespace_ops(store, namespace_id.to_bytes());
    Ok(key_id)
}

#[cfg(test)]
pub(crate) mod tests {
    use calimero_store::key::GroupTarget;
    use std::sync::Arc;

    use calimero_context_client::local_governance::{
        GroupOp, NamespaceOp, RootOp, SignedNamespaceOp,
    };
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::VisibilityMode;
    use calimero_governance_store::{
        CapabilitiesRepository, DenyListRepository, GroupKeyring, MembershipRepository,
        MetaRepository, NamespaceDagService, NamespaceOpLogService, NamespaceRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;
    use rand::rand_core::UnwrapErr;
    use rand::rngs::SysRng;

    use super::adopt_pulled_group_key;
    use crate::scope_projection::ScopeProjections;

    fn meta(admin: calimero_account::AccountId) -> GroupMetaValue {
        GroupMetaValue {
            target: GroupTarget {
                application_id: calimero_primitives::application::ApplicationId::from([0xCC; 32]),
                bytecode_id: [0xBB; 32],
                package: Box::default(),
                version: Box::default(),
            },
            created_at: 1_700_000_000,
            admin_identity: admin,
            owner_identity: admin,
            migration: None,
            auto_join: true,
        }
    }

    /// Put a signed op on the DAG the way a received op lands. The op-log write
    /// also writes the unified op, decoding with whatever key is present then.
    pub(crate) fn land(
        store: &Store,
        namespace: [u8; 32],
        op: &SignedNamespaceOp,
        parents: &[[u8; 32]],
    ) {
        let delta_id = op.content_hash().unwrap();
        NamespaceOpLogService::new(store, namespace.into())
            .store_signed_operation(op)
            .unwrap();
        NamespaceDagService::new(store, namespace.into())
            .advance_dag_head(delta_id, parents, 0)
            .unwrap();
    }

    /// An admin-signed invitation, so the joiner's `MemberJoined` folds into a
    /// namespace membership row the inheritance walk can anchor on.
    pub(crate) fn invitation(
        admin_sk: &PrivateKey,
        group: ContextGroupId,
    ) -> calimero_context_config::types::SignedGroupOpenInvitation {
        use sha2::{Digest, Sha256};
        let invitation = calimero_context_config::types::GroupInvitationFromAdmin {
            inviter_identity: calimero_context_config::types::SignerId::from(
                *admin_sk.public_key().digest(),
            ),
            group_id: group,
            expiration_timestamp: 0,
            invitation_nonce: [0x42; 32],
            invited_role: 1,
            admitters: Vec::new(),
        };
        let bytes = borsh::to_vec(&invitation).unwrap();
        let signature = admin_sk.sign(&Sha256::digest(&bytes)).unwrap();
        calimero_context_config::types::SignedGroupOpenInvitation {
            inviter_account: None,
            invitation,
            inviter_signature: hex::encode(signature.to_bytes()),
            application_id: None,
            bytecode_id: None,
            admitter_addrs: Vec::new(),
        }
    }

    /// A third node cannot join a namespace through an inherited `Open` subgroup
    /// unless a peer-pulled key recovers BOTH planes.
    ///
    /// The apply gate resolves the membership path from the unified op-store at
    /// the op's causal cut, so re-driving only the live plane leaves the gate
    /// reading the frozen `Noop` and still rejecting the join.
    ///
    /// Both the join and the visibility flip it depends on are sealed under the
    /// namespace key, so this node -- which does not hold it -- reads neither.
    /// That makes the pre-key state a PARK rather than a verdict: answering "no
    /// membership path" from a fold that is missing the flip would be a decision
    /// about a history this node cannot see, and a keyed peer would contradict
    /// it. The recovery has to fold the flip and THEN the join, in that order,
    /// which is the ordering `retry_encrypted_ops_for_group` closes by running
    /// its sealed-root pass on both sides of the group phase.
    #[test]
    fn a_pulled_key_unwedges_another_members_open_subgroup_join() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut rng = UnwrapErr(SysRng);

        let owner_sk = PrivateKey::random(&mut rng);
        let owner = owner_sk.public_key();
        let joiner_sk = PrivateKey::random(&mut rng);
        let joiner = joiner_sk.public_key();

        let ns = ContextGroupId::from([0x4Bu8; 32]);
        let namespace_id = ns.to_bytes();
        let owner_account = crate::test_support::enrol(&store, &ns, &owner);
        let joiner_account = crate::test_support::enrol(&store, &ns, &joiner);

        MetaRepository::new(&store)
            .save(&ns, &meta(owner_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&ns, &owner_account, GroupMemberRole::Admin)
            .unwrap();
        // Set before the member row: `add_member` seeds a non-admin's capability
        // row from the group default, which is what the walk reads.
        CapabilitiesRepository::new(&store)
            .set_default_capabilities(
                &ns,
                calimero_context_config::MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
            )
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&ns, &joiner_account, GroupMemberRole::Member)
            .unwrap();

        let sub_salt: [u8; 32] = rand::RngExt::random(&mut rng);
        let sub = ContextGroupId::from(calimero_account::created_subgroup_id(
            &owner_account,
            &namespace_id,
            true,
            &sub_salt,
        ));
        NamespaceRepository::new(&store).nest(&ns, &sub).unwrap();
        MetaRepository::new(&store)
            .save(&sub, &meta(owner_account))
            .unwrap();

        let created = SignedNamespaceOp::sign(
            &owner_sk,
            namespace_id.into(),
            vec![],
            1,
            NamespaceOp::Root(RootOp::GroupCreated {
                admin: owner_account,
                group_id: sub.to_bytes().into(),
                parent_id: namespace_id.into(),
                restricted: true,
                salt: sub_salt,
            }),
        )
        .unwrap();
        land(&store, namespace_id, &created, &[]);
        let created_id = created.content_hash().unwrap();

        let joined = SignedNamespaceOp::sign(
            &joiner_sk,
            namespace_id.into(),
            vec![created_id],
            2,
            NamespaceOp::Root(RootOp::MemberJoined {
                member: joiner_account,
                signed_invitation: invitation(&owner_sk, ns),
                account: crate::test_support::credential(&joiner),
            }),
        )
        .unwrap();
        land(&store, namespace_id, &joined, &[created_id]);
        let joined_id = joined.content_hash().unwrap();

        // The key is deliberately not stored yet, so this lands DAG-applied but
        // effect-skipped, and freezes into the op-store as a `Noop`.
        let ns_key = [0x5Au8; 32];
        let flip = SignedNamespaceOp::sign(
            &owner_sk,
            namespace_id.into(),
            vec![joined_id],
            3,
            NamespaceOp::Group {
                group_id: sub.to_bytes().into(),
                key_id: GroupKeyring::key_id_for(&ns_key).into(),
                encrypted: GroupKeyring::encrypt_op(
                    &ns_key,
                    &GroupOp::SubgroupVisibilitySet {
                        mode: VisibilityMode::Open,
                    },
                )
                .unwrap(),
                key_rotation: None,
            },
        )
        .unwrap();
        land(&store, namespace_id, &flip, &[joined_id]);
        let flip_id = flip.content_hash().unwrap();
        assert_eq!(
            CapabilitiesRepository::new(&store)
                .subgroup_visibility(&sub)
                .unwrap(),
            VisibilityMode::Restricted,
            "precondition: the flip is invisible while the key is absent"
        );

        // The state a kick or a leave leaves behind, and the one thing this
        // op's apply -- and nothing else here -- undoes. Membership itself is
        // no use as a signal: it is DERIVED from the anchor row and the
        // subgroup's visibility, so folding the flip alone would make
        // `is_member` true and the assertion would hold with the join never
        // applied at all.
        DenyListRepository::new(&store)
            .mark(&sub, &joiner_account)
            .expect("stamp the joiner as previously denied in the subgroup");

        // Sealed, because `root_op_is_sealable` says this variant is published
        // that way: its publisher is an inherited member that already holds the
        // namespace key. Built by hand rather than through
        // `seal_root_op_for_publish`, which reads the key from the store this
        // fixture deliberately keeps empty.
        let join = SignedNamespaceOp::sign(
            &joiner_sk,
            namespace_id.into(),
            vec![flip_id],
            4,
            NamespaceOp::RootSealed {
                key_id: GroupKeyring::key_id_for(&ns_key).into(),
                encrypted: GroupKeyring::encrypt_root_op(
                    &ns_key,
                    &RootOp::MemberJoinedOpen {
                        member: joiner_account,
                        group_id: sub.to_bytes().into(),
                        account: crate::test_support::credential(&joiner),
                    },
                )
                .unwrap(),
            },
        )
        .unwrap();

        // Parked, not rejected: with no key the node cannot open the join at
        // all, so it reports the miss and leaves the op in the log for the
        // retry pass. An error here would drop an op this node applies a moment
        // later.
        let parked = calimero_governance_store::apply_signed_namespace_op_at_cut(
            &store,
            &join,
            &[flip_id],
            &crate::apply_authorizer::EphemeralProjectionAuthorizer::new(&store),
        )
        .expect("a sealed root op whose key is absent must park, not fail");
        assert!(
            !parked.key_unwrap_failures.is_empty(),
            "the parked join must be reported as a key-unwrap miss so the retry \
             pass has something to pick up"
        );
        assert!(
            DenyListRepository::new(&store)
                .is_denied(&sub, &joiner_account)
                .unwrap(),
            "precondition: the join has not been folded while its key is absent"
        );

        // `load_scope_ops` returns the rows in any order, so compare as sets.
        let op_ids = |store: &Store| {
            let mut ids: Vec<[u8; 32]> = crate::unified_op_store::load_scope_ops(
                store,
                &calimero_op::ScopeId::from(namespace_id),
            )
            .expect("the op-store must read back")
            .iter()
            .map(calimero_op::Op::id)
            .collect();
            ids.sort_unstable();
            ids
        };

        let ops_before_adopt = op_ids(&store);
        let key_id = adopt_pulled_group_key(&store, namespace_id.into(), ns, &ns_key)
            .expect("a peer-pulled namespace key must store");

        // The re-persist re-decodes the flip that froze as a `Noop` and writes it
        // back under the same content-addressed id, so the row is overwritten in
        // place. An id that moved with the decoded payload would leave the stale
        // `Noop` behind and the projection would fold both.
        assert!(
            !ops_before_adopt.is_empty(),
            "the namespace must hold ops for the re-persist to rewrite"
        );
        assert_eq!(
            ops_before_adopt,
            op_ids(&store),
            "re-persisting a decoded op must overwrite its row, not add one"
        );
        assert_eq!(
            CapabilitiesRepository::new(&store)
                .subgroup_visibility(&sub)
                .unwrap(),
            VisibilityMode::Open,
            "the re-drive must apply the buffered flip to the live store"
        );
        // The whole point. The sealed join is only admissible once the flip it
        // parents onto has been folded, so a single sealed-root pass that ran
        // before the group phase would have refused it and logged the refusal —
        // leaving this node the only one that does not see the joiner.
        //
        // Read through the deny list rather than `is_member`, which would pass
        // whether or not the join applied: an inherited membership is DERIVED
        // from the anchor row plus the now-`Open` visibility, so the flip alone
        // is enough to make it true. Clearing the deny stamp is something only
        // this op's apply does.
        assert!(
            !DenyListRepository::new(&store)
                .is_denied(&sub, &joiner_account)
                .unwrap(),
            "the buffered join must fold once the flip it depends on is readable"
        );
        assert_eq!(
            calimero_governance_store::parked_op(
                &store,
                namespace_id.into(),
                join.content_hash().unwrap()
            )
            .unwrap(),
            None,
            "a join its first pass could not apply is no longer parked once a later one did"
        );

        // Both recoveries are best-effort, so the key-delivery and startup sweeps
        // re-run them on a key this path already adopted.
        let ops_before_replay = op_ids(&store);
        let replayed = adopt_pulled_group_key(&store, namespace_id.into(), ns, &ns_key)
            .expect("re-adopting an already-stored key must not fail");

        assert_eq!(
            replayed, key_id,
            "the key id is derived from the key itself"
        );
        assert_eq!(
            ops_before_replay,
            op_ids(&store),
            "a replayed adopt must not add or drop an op"
        );
        assert_eq!(
            CapabilitiesRepository::new(&store)
                .subgroup_visibility(&sub)
                .unwrap(),
            VisibilityMode::Open,
            "a replayed adopt must not disturb the recovered live state"
        );
        assert!(
            !DenyListRepository::new(&store)
                .is_denied(&sub, &joiner_account)
                .unwrap(),
            "a replayed adopt must not undo the folded join"
        );
    }

    /// An admin, a plain member (Mallory), and this node, which holds the
    /// namespace key of neither yet.
    pub(crate) fn keyless_namespace(ns: ContextGroupId) -> (Store, PrivateKey, PrivateKey) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let owner_sk = PrivateKey::from([0x71; 32]);
        let mallory_sk = PrivateKey::from([0x72; 32]);
        let owner = crate::test_support::enrol(&store, &ns, &owner_sk.public_key());
        let mallory = crate::test_support::enrol(&store, &ns, &mallory_sk.public_key());
        MetaRepository::new(&store).save(&ns, &meta(owner)).unwrap();
        let members = MembershipRepository::new(&store);
        members
            .add_member(&ns, &owner, GroupMemberRole::Admin)
            .unwrap();
        members
            .add_member(&ns, &mallory, GroupMemberRole::Member)
            .unwrap();
        (store, owner_sk, mallory_sk)
    }

    /// Apply `op` as it arrives from a peer: parked, since this node lacks its key.
    pub(crate) fn arrive(store: &Store, op: &SignedNamespaceOp, parents: &[[u8; 32]]) -> [u8; 32] {
        let parked = calimero_governance_store::apply_signed_namespace_op_at_cut(
            store,
            op,
            parents,
            &crate::apply_authorizer::EphemeralProjectionAuthorizer::new(store),
        )
        .expect("an op whose key is absent parks");
        let id = op.content_hash().unwrap();
        assert_eq!(
            calimero_governance_store::parked_op(store, op.namespace_id, id).unwrap(),
            Some(calimero_governance_store::Parked::Undecided),
            "precondition: the op is parked unread"
        );
        id
    }

    /// Whether `account` is an admin of `group` at this node's governance heads,
    /// as the apply gates read it.
    pub(crate) fn admin_at_heads(
        store: &Store,
        ns: &ContextGroupId,
        group: ContextGroupId,
        account: &calimero_account::AccountId,
    ) -> Option<bool> {
        let (projection, _, heads) =
            ScopeProjections::ephemeral_projection(store, ns).expect("fold the namespace");
        projection.is_admin_account_at_cut(store, group, account, &heads)
    }

    /// A member's self-promotion, encrypted under a key this node lacks, is refused
    /// by the apply once the key arrives, and must then grant nothing at the cut.
    #[test]
    fn a_refused_encrypted_group_op_grants_nothing_once_its_key_arrives() {
        let ns = ContextGroupId::from([0x4C; 32]);
        let namespace_id = ns.to_bytes();
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());

        let joined = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![],
            1,
            NamespaceOp::Root(RootOp::MemberJoined {
                member: mallory,
                signed_invitation: invitation(&owner_sk, ns),
                account: crate::test_support::credential(&mallory_sk.public_key()),
            }),
        )
        .unwrap();
        land(&store, namespace_id, &joined, &[]);
        let joined_id = joined.content_hash().unwrap();
        assert_eq!(
            admin_at_heads(&store, &ns, ns, &mallory),
            Some(false),
            "control: the cut is decidable and Mallory is no admin before her op"
        );

        let ns_key = [0x5Cu8; 32];
        let promote = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![joined_id],
            2,
            NamespaceOp::Group {
                group_id: ns.to_bytes().into(),
                key_id: GroupKeyring::key_id_for(&ns_key).into(),
                encrypted: GroupKeyring::encrypt_op(
                    &ns_key,
                    &GroupOp::MemberRoleSet {
                        member: mallory,
                        role: GroupMemberRole::Admin,
                    },
                )
                .unwrap(),
                key_rotation: None,
            },
        )
        .unwrap();
        let _ = arrive(&store, &promote, &[joined_id]);
        let _ = adopt_pulled_group_key(&store, namespace_id.into(), ns, &ns_key)
            .expect("the pulled key stores");

        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&ns, &mallory)
                .unwrap(),
            Some(GroupMemberRole::Member),
            "control: the apply refused the self-promotion"
        );
        assert_eq!(
            admin_at_heads(&store, &ns, ns, &owner),
            Some(true),
            "control: the cut is decidable and names the real admin"
        );
        assert_eq!(
            admin_at_heads(&store, &ns, ns, &mallory),
            Some(false),
            "an op the apply refused must not make its signer an admin at the cut"
        );
    }

    /// The sealed-root shape: Mallory creates a subgroup she may not create and
    /// moves the owner's Open subgroup under it. Both are refused once the key
    /// arrives, and neither may give her authority over the owner's subgroup.
    #[test]
    fn a_refused_sealed_root_op_grants_nothing_once_its_key_arrives() {
        let ns = ContextGroupId::from([0x4D; 32]);
        let namespace_id = ns.to_bytes();
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());

        let s_salt = [0x11u8; 32];
        let s = ContextGroupId::from(calimero_account::created_subgroup_id(
            &owner,
            &namespace_id,
            false,
            &s_salt,
        ));
        NamespaceRepository::new(&store).nest(&ns, &s).unwrap();
        MetaRepository::new(&store).save(&s, &meta(owner)).unwrap();
        let created = SignedNamespaceOp::sign(
            &owner_sk,
            namespace_id.into(),
            vec![],
            1,
            NamespaceOp::Root(RootOp::GroupCreated {
                admin: owner,
                group_id: s.to_bytes().into(),
                parent_id: namespace_id.into(),
                restricted: false,
                salt: s_salt,
            }),
        )
        .unwrap();
        land(&store, namespace_id, &created, &[]);
        let created_id = created.content_hash().unwrap();
        assert_eq!(
            admin_at_heads(&store, &ns, s, &mallory),
            Some(false),
            "control: the cut is decidable and Mallory is no admin of S before her ops"
        );

        let ns_key = [0x5Du8; 32];
        let seal = |root: &RootOp| NamespaceOp::RootSealed {
            key_id: GroupKeyring::key_id_for(&ns_key).into(),
            encrypted: GroupKeyring::encrypt_root_op(&ns_key, root).unwrap(),
        };
        let a_salt = [0x22u8; 32];
        let a = ContextGroupId::from(calimero_account::created_subgroup_id(
            &mallory,
            &namespace_id,
            true,
            &a_salt,
        ));
        let create_a = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![created_id],
            1,
            seal(&RootOp::GroupCreated {
                admin: mallory,
                group_id: a.to_bytes().into(),
                parent_id: namespace_id.into(),
                restricted: true,
                salt: a_salt,
            }),
        )
        .unwrap();
        let create_a_id = arrive(&store, &create_a, &[created_id]);
        let move_s = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![create_a_id],
            2,
            seal(&RootOp::GroupReparented {
                child_group_id: s,
                new_parent_id: a,
            }),
        )
        .unwrap();
        let _ = arrive(&store, &move_s, &[create_a_id]);
        let _ = adopt_pulled_group_key(&store, namespace_id.into(), ns, &ns_key)
            .expect("the pulled key stores");

        assert!(
            MetaRepository::new(&store).load(&a).unwrap().is_none(),
            "control: the apply refused Mallory's subgroup"
        );
        assert_eq!(
            NamespaceRepository::new(&store).parent(&s).unwrap(),
            Some(ns),
            "control: the apply refused the move"
        );
        assert_eq!(
            admin_at_heads(&store, &ns, s, &owner),
            Some(true),
            "control: the cut is decidable and names S's creator"
        );
        assert_eq!(
            admin_at_heads(&store, &ns, s, &mallory),
            Some(false),
            "ops the apply refused must not make their signer an admin of S at the cut"
        );
    }

    /// Land Mallory's join, so the cut names her as a namespace member.
    pub(crate) fn land_mallory_join(
        store: &Store,
        ns: ContextGroupId,
        owner_sk: &PrivateKey,
        mallory_sk: &PrivateKey,
    ) -> [u8; 32] {
        let joined = SignedNamespaceOp::sign(
            mallory_sk,
            ns.to_bytes().into(),
            vec![],
            1,
            NamespaceOp::Root(RootOp::MemberJoined {
                member: crate::test_support::account_for(&mallory_sk.public_key()),
                signed_invitation: invitation(owner_sk, ns),
                account: crate::test_support::credential(&mallory_sk.public_key()),
            }),
        )
        .unwrap();
        land(store, ns.to_bytes(), &joined, &[]);
        joined.content_hash().unwrap()
    }

    /// `signer`'s `MemberRoleSet` in the namespace root, encrypted under `key`.
    fn role_set(
        signer: &PrivateKey,
        ns: ContextGroupId,
        parents: Vec<[u8; 32]>,
        nonce: u64,
        key: &[u8; 32],
        member: calimero_account::AccountId,
    ) -> SignedNamespaceOp {
        SignedNamespaceOp::sign(
            signer,
            ns.to_bytes().into(),
            parents,
            nonce,
            NamespaceOp::Group {
                group_id: ns.to_bytes().into(),
                key_id: GroupKeyring::key_id_for(key).into(),
                encrypted: GroupKeyring::encrypt_op(
                    key,
                    &GroupOp::MemberRoleSet {
                        member,
                        role: GroupMemberRole::Admin,
                    },
                )
                .unwrap(),
                key_rotation: None,
            },
        )
        .unwrap()
    }

    /// The owner's subgroup S, folded here, and Mallory's sealed `GroupCreated`
    /// naming S with herself as its admin, parked for lack of `ns_key`.
    pub(crate) fn park_a_create_naming_a_folded_group(
        store: &Store,
        ns: ContextGroupId,
        owner_sk: &PrivateKey,
        mallory_sk: &PrivateKey,
        ns_key: &[u8; 32],
    ) -> ContextGroupId {
        let namespace_id = ns.to_bytes();
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());
        let s_salt = [0x13u8; 32];
        let s = ContextGroupId::from(calimero_account::created_subgroup_id(
            &owner,
            &namespace_id,
            false,
            &s_salt,
        ));
        NamespaceRepository::new(store).nest(&ns, &s).unwrap();
        MetaRepository::new(store).save(&s, &meta(owner)).unwrap();
        CapabilitiesRepository::new(store)
            .set_subgroup_visibility(&s, VisibilityMode::Open)
            .unwrap();
        let created = SignedNamespaceOp::sign(
            owner_sk,
            namespace_id.into(),
            vec![],
            1,
            NamespaceOp::Root(RootOp::GroupCreated {
                admin: owner,
                group_id: s.to_bytes().into(),
                parent_id: namespace_id.into(),
                restricted: false,
                salt: s_salt,
            }),
        )
        .unwrap();
        land(store, namespace_id, &created, &[]);
        let created_id = created.content_hash().unwrap();

        let foreign = SignedNamespaceOp::sign(
            mallory_sk,
            namespace_id.into(),
            vec![created_id],
            1,
            NamespaceOp::RootSealed {
                key_id: GroupKeyring::key_id_for(ns_key).into(),
                encrypted: GroupKeyring::encrypt_root_op(
                    ns_key,
                    &RootOp::GroupCreated {
                        admin: mallory,
                        group_id: s.to_bytes().into(),
                        parent_id: namespace_id.into(),
                        restricted: false,
                        salt: s_salt,
                    },
                )
                .unwrap(),
            },
        )
        .unwrap();
        let _ = arrive(store, &foreign, &[created_id]);
        s
    }

    /// A create naming a group this node already folded is skipped by the replay as
    /// a re-fed create, which must not leave a parked one folding its payload.
    #[test]
    fn a_parked_create_naming_a_folded_group_grants_nothing_once_its_key_arrives() {
        let ns = ContextGroupId::from([0x4E; 32]);
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());
        let ns_key = [0x5Eu8; 32];
        let s = park_a_create_naming_a_folded_group(&store, ns, &owner_sk, &mallory_sk, &ns_key);

        let _ = adopt_pulled_group_key(&store, ns.to_bytes().into(), ns, &ns_key)
            .expect("the pulled key stores");

        assert_eq!(
            admin_at_heads(&store, &ns, s, &owner),
            Some(true),
            "control: the cut is decidable and names S's creator"
        );
        assert_eq!(
            admin_at_heads(&store, &ns, s, &mallory),
            Some(false),
            "a create the apply would refuse must not seat its signer as S's admin"
        );
    }

    /// A parked op is judged at its own cut, as on a node that held the key on
    /// arrival, not against the rows this node holds when the key arrives.
    #[test]
    fn a_parked_op_is_judged_at_its_own_cut_once_its_key_arrives() {
        let ns = ContextGroupId::from([0x4F; 32]);
        let namespace_id = ns.to_bytes();
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());
        let yara_sk = PrivateKey::from([0x73; 32]);
        let yara = crate::test_support::enrol(&store, &ns, &yara_sk.public_key());
        let joined_id = land_mallory_join(&store, ns, &owner_sk, &mallory_sk);

        // Mallory's Restricted subgroup S. Its live rows already show her gone,
        // as a later removal's cascade leaves them; at the cut she is its admin.
        let s_salt = [0x14u8; 32];
        let s = ContextGroupId::from(calimero_account::created_subgroup_id(
            &mallory,
            &namespace_id,
            true,
            &s_salt,
        ));
        NamespaceRepository::new(&store).nest(&ns, &s).unwrap();
        MetaRepository::new(&store).save(&s, &meta(owner)).unwrap();
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&s, VisibilityMode::Restricted)
            .unwrap();
        let created = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![joined_id],
            2,
            NamespaceOp::Root(RootOp::GroupCreated {
                admin: mallory,
                group_id: s.to_bytes().into(),
                parent_id: namespace_id.into(),
                restricted: true,
                salt: s_salt,
            }),
        )
        .unwrap();
        land(&store, namespace_id, &created, &[joined_id]);
        let created_id = created.content_hash().unwrap();
        assert_eq!(
            admin_at_heads(&store, &ns, s, &mallory),
            Some(true),
            "control: at the cut Mallory is S's admin"
        );

        let s_key = [0x5Fu8; 32];
        let add = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![created_id],
            3,
            NamespaceOp::Group {
                group_id: s.to_bytes().into(),
                key_id: GroupKeyring::key_id_for(&s_key).into(),
                encrypted: GroupKeyring::encrypt_op(
                    &s_key,
                    &GroupOp::MemberAdded {
                        member: yara,
                        role: GroupMemberRole::Member,
                    },
                )
                .unwrap(),
                key_rotation: None,
            },
        )
        .unwrap();
        let _ = arrive(&store, &add, &[created_id]);
        let _ = adopt_pulled_group_key(&store, namespace_id.into(), s, &s_key)
            .expect("the pulled key stores");

        let (projection, _, heads) =
            ScopeProjections::ephemeral_projection(&store, &ns).expect("fold the namespace");
        assert_eq!(
            projection.account_member_at_cut(&store, s, &yara, &heads),
            Some(true),
            "an add its signer was entitled to at its cut must fold"
        );
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&s, &yara)
                .unwrap(),
            Some(GroupMemberRole::Member),
            "and apply, as on every node that held the key on arrival"
        );
    }

    /// Between a key being stored and the replay deciding a parked op, the op is a
    /// hole: the key alone must not let it grant what its apply may refuse.
    #[test]
    fn a_parked_op_grants_nothing_before_its_replay_decides_it() {
        let ns = ContextGroupId::from([0x50; 32]);
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());
        let joined_id = land_mallory_join(&store, ns, &owner_sk, &mallory_sk);
        let ns_key = [0x60u8; 32];
        let promote = role_set(&mallory_sk, ns, vec![joined_id], 2, &ns_key, mallory);
        let _ = arrive(&store, &promote, &[joined_id]);

        let _ = GroupKeyring::new(&store, ns).store_key(&ns_key).unwrap();
        assert_ne!(
            admin_at_heads(&store, &ns, ns, &mallory),
            Some(true),
            "a stored key with no replay yet must not make the op's signer an admin"
        );

        let _ = adopt_pulled_group_key(&store, ns.to_bytes().into(), ns, &ns_key)
            .expect("the pulled key stores");
        assert_eq!(admin_at_heads(&store, &ns, ns, &owner), Some(true));
        assert_eq!(admin_at_heads(&store, &ns, ns, &mallory), Some(false));
    }

    /// An op its replay applies folds its payload: a parked op never stays a hole
    /// once its key has arrived and its apply took.
    #[test]
    fn an_honest_parked_op_folds_once_its_key_arrives() {
        let ns = ContextGroupId::from([0x51; 32]);
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());
        let joined_id = land_mallory_join(&store, ns, &owner_sk, &mallory_sk);
        let ns_key = [0x61u8; 32];
        let promote = role_set(&owner_sk, ns, vec![joined_id], 2, &ns_key, mallory);
        let _ = arrive(&store, &promote, &[joined_id]);

        let _ = adopt_pulled_group_key(&store, ns.to_bytes().into(), ns, &ns_key)
            .expect("the pulled key stores");
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&ns, &mallory)
                .unwrap(),
            Some(GroupMemberRole::Admin),
            "control: the apply took"
        );
        assert_eq!(
            admin_at_heads(&store, &ns, ns, &mallory),
            Some(true),
            "the owner's promotion folds once applied"
        );
    }

    /// A subgroup-sealed op carrying anything but a join is refused when its key
    /// arrives, and folds as nothing rather than as a hole over the subgroup.
    #[test]
    fn a_group_sealed_op_carrying_no_join_folds_as_nothing_once_its_key_arrives() {
        let ns = ContextGroupId::from([0x52; 32]);
        let namespace_id = ns.to_bytes();
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());
        let s_salt = [0x15u8; 32];
        let s = ContextGroupId::from(calimero_account::created_subgroup_id(
            &owner,
            &namespace_id,
            true,
            &s_salt,
        ));
        NamespaceRepository::new(&store).nest(&ns, &s).unwrap();
        MetaRepository::new(&store).save(&s, &meta(owner)).unwrap();
        let created = SignedNamespaceOp::sign(
            &owner_sk,
            namespace_id.into(),
            vec![],
            1,
            NamespaceOp::Root(RootOp::GroupCreated {
                admin: owner,
                group_id: s.to_bytes().into(),
                parent_id: namespace_id.into(),
                restricted: true,
                salt: s_salt,
            }),
        )
        .unwrap();
        land(&store, namespace_id, &created, &[]);
        let created_id = created.content_hash().unwrap();

        let s_key = [0x62u8; 32];
        let not_a_join = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![created_id],
            1,
            NamespaceOp::RootSealedForGroup {
                group_id: s.to_bytes().into(),
                key_id: GroupKeyring::key_id_for(&s_key).into(),
                encrypted: GroupKeyring::encrypt_root_op(
                    &s_key,
                    &RootOp::AdminChanged { new_admin: mallory },
                )
                .unwrap(),
            },
        )
        .unwrap();
        let _ = arrive(&store, &not_a_join, &[created_id]);
        let _ = adopt_pulled_group_key(&store, namespace_id.into(), s, &s_key)
            .expect("the pulled key stores");

        assert_eq!(
            admin_at_heads(&store, &ns, s, &owner),
            Some(true),
            "the refused op leaves S's cut decidable"
        );
        assert_eq!(admin_at_heads(&store, &ns, s, &mallory), Some(false));
    }

    /// A relayed join its replay refuses (an inviter with no standing) is recorded
    /// as refused, and its joiner is no member at the cut.
    #[test]
    fn a_refused_relayed_join_is_recorded_as_refused_once_its_key_arrives() {
        let ns = ContextGroupId::from([0x54; 32]);
        let namespace_id = ns.to_bytes();
        let (store, _owner_sk, mallory_sk) = keyless_namespace(ns);
        let joiner_sk = PrivateKey::from([0x75; 32]);
        let joiner = crate::test_support::account_for(&joiner_sk.public_key());
        let ns_key = [0x64u8; 32];

        let inner = SignedNamespaceOp::sign(
            &joiner_sk,
            namespace_id.into(),
            vec![],
            1,
            NamespaceOp::Root(RootOp::MemberJoined {
                member: joiner,
                signed_invitation: invitation(&mallory_sk, ns),
                account: crate::test_support::credential(&joiner_sk.public_key()),
            }),
        )
        .unwrap();
        let relayed = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![],
            1,
            NamespaceOp::RootRelaySealed {
                key_id: GroupKeyring::key_id_for(&ns_key).into(),
                encrypted: GroupKeyring::encrypt_relayed_op(&ns_key, &inner).unwrap(),
            },
        )
        .unwrap();
        let id = arrive(&store, &relayed, &[]);
        let _ = adopt_pulled_group_key(&store, namespace_id.into(), ns, &ns_key)
            .expect("the pulled key stores");

        assert_eq!(
            calimero_governance_store::parked_op(&store, namespace_id.into(), id).unwrap(),
            Some(calimero_governance_store::Parked::Refused)
        );
        let (projection, _, heads) =
            ScopeProjections::ephemeral_projection(&store, &ns).expect("fold the namespace");
        assert_eq!(
            projection.account_member_at_cut(&store, ns, &joiner, &heads),
            Some(false)
        );
    }
}
