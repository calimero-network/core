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
    let authorizer = crate::apply_authorizer::VoidJudge::new(store);
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
    let _ = refresh_after_group_key(store, namespace_id, group_id);
    Ok(key_id)
}

/// End a key recovery for `group_id`: re-decode the namespace's op-store with the
/// keys now held, then replay the group's ops once more.
///
/// The replay that ran when the key was stored re-drives only the ops it
/// collected then. An op of the group that arrives while that replay is still
/// judging the group's parked ops is read against a fold that shows them as
/// holes, so the gate holds it for a key this node already has (core#4511's
/// hold). No later key arrives to replay it, so without this pass it stays held
/// for good: the member row an `add_group_members` wrote never lands here and
/// the group's state hash differs from its peers'. The pass is idempotent: an op
/// already applied is skipped by its nonce.
///
/// Returns the divergence the replay reports, for the caller to reconcile.
pub fn refresh_after_group_key(
    store: &Store,
    namespace_id: NamespaceId,
    group_id: ContextGroupId,
) -> Option<calimero_governance_store::DivergenceReport> {
    ScopeProjections::repersist_namespace_ops(store, namespace_id.to_bytes());
    let authorizer = crate::apply_authorizer::VoidJudge::new(store);
    match calimero_governance_store::retry_encrypted_ops_for_group_with(
        store,
        namespace_id,
        group_id.to_bytes(),
        &authorizer,
    ) {
        Ok(divergence) => divergence,
        Err(err) => {
            warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                %err,
                "failed to replay the group's ops after its key recovery; the next key \
                 arrival or the startup sweep replays them"
            );
            None
        }
    }
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
        CapabilitiesRepository, GroupKeyring, KeyRecipient, MembershipRepository, MetaRepository,
        NamespaceDagService, NamespaceOpLogService, NamespaceRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;

    use super::{adopt_pulled_group_key, refresh_after_group_key};
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
        invitation_with(admin_sk, group, [0x42; 32], Vec::new())
    }

    fn invitation_with(
        admin_sk: &PrivateKey,
        group: ContextGroupId,
        nonce: [u8; 32],
        admitters: Vec<calimero_account::AccountId>,
    ) -> calimero_context_config::types::SignedGroupOpenInvitation {
        use sha2::{Digest, Sha256};
        let invitation = calimero_context_config::types::GroupInvitationFromAdmin {
            inviter_identity: calimero_context_config::types::SignerId::from(
                *admin_sk.public_key().digest(),
            ),
            group_id: group,
            expiration_timestamp: 0,
            invitation_nonce: nonce,
            invited_role: 1,
            admitters,
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
        let _ = calimero_governance_store::apply_signed_namespace_op_at_cut(
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
                registers: None,
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
                registers: None,
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

    /// Mallory's Restricted subgroup S. Its live rows already show her gone, as a later
    /// removal's cascade leaves them; at the cut she is its admin.
    fn mallory_subgroup(
        store: &Store,
        ns: ContextGroupId,
        owner_sk: &PrivateKey,
        mallory_sk: &PrivateKey,
        joined_id: [u8; 32],
    ) -> (ContextGroupId, [u8; 32]) {
        let namespace_id = ns.to_bytes();
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());
        let s_salt = [0x14u8; 32];
        let s = ContextGroupId::from(calimero_account::created_subgroup_id(
            &mallory,
            &namespace_id,
            true,
            &s_salt,
        ));
        NamespaceRepository::new(store).nest(&ns, &s).unwrap();
        MetaRepository::new(store).save(&s, &meta(owner)).unwrap();
        CapabilitiesRepository::new(store)
            .set_subgroup_visibility(&s, VisibilityMode::Restricted)
            .unwrap();
        let created = SignedNamespaceOp::sign(
            mallory_sk,
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
        land(store, namespace_id, &created, &[joined_id]);
        let created_id = created.content_hash().unwrap();
        assert_eq!(
            admin_at_heads(store, &ns, s, &mallory),
            Some(true),
            "control: at the cut Mallory is S's admin"
        );
        (s, created_id)
    }

    /// A parked op is judged at its own cut, as on a node that held the key on
    /// arrival, not against the rows this node holds when the key arrives.
    #[test]
    fn a_parked_op_is_judged_at_its_own_cut_once_its_key_arrives() {
        let ns = ContextGroupId::from([0x4F; 32]);
        let namespace_id = ns.to_bytes();
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let yara_sk = PrivateKey::from([0x73; 32]);
        let yara = crate::test_support::enrol(&store, &ns, &yara_sk.public_key());
        let joined_id = land_mallory_join(&store, ns, &owner_sk, &mallory_sk);

        let (s, created_id) = mallory_subgroup(&store, ns, &owner_sk, &mallory_sk, joined_id);
        let yara_joined = SignedNamespaceOp::sign(
            &yara_sk,
            namespace_id.into(),
            vec![joined_id],
            1,
            NamespaceOp::Root(RootOp::MemberJoined {
                member: yara,
                signed_invitation: invitation(&owner_sk, ns),
                account: crate::test_support::credential(&yara_sk.public_key()),
            }),
        )
        .unwrap();
        land(&store, namespace_id, &yara_joined, &[joined_id]);
        let yara_joined_id = yara_joined.content_hash().unwrap();

        let s_key = [0x5Fu8; 32];
        let add = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![created_id, yara_joined_id],
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
                registers: None,
            },
        )
        .unwrap();
        let _ = arrive(&store, &add, &[created_id, yara_joined_id]);
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
        // The owner's key speaks for its account at the cut only through a folded credential.
        let owner_joined = land_mallory_join(&store, ns, &owner_sk, &owner_sk);
        let ns_key = [0x61u8; 32];
        let cut = vec![joined_id, owner_joined];
        let promote = role_set(&owner_sk, ns, cut.clone(), 2, &ns_key, mallory);
        let _ = arrive(&store, &promote, &cut);

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

    /// A parked op's carried rotation is judged at the op's own cut, as the op is: its
    /// signer was S's admin there, though the live rows show her gone.
    #[test]
    fn a_parked_ops_rotation_is_judged_at_its_own_cut_once_its_key_arrives() {
        let ns = ContextGroupId::from([0x52; 32]);
        let namespace_id = ns.to_bytes();
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let yara_sk = PrivateKey::from([0x73; 32]);
        let yara = crate::test_support::enrol(&store, &ns, &yara_sk.public_key());
        let node_sk = PrivateKey::from([0x74; 32]);
        NamespaceRepository::new(&store)
            .store_identity(&ns, &node_sk.public_key(), &[0x74; 32])
            .unwrap();
        let joined_id = land_mallory_join(&store, ns, &owner_sk, &mallory_sk);
        let (s, created_id) = mallory_subgroup(&store, ns, &owner_sk, &mallory_sk, joined_id);

        let yara_joined = SignedNamespaceOp::sign(
            &yara_sk,
            namespace_id.into(),
            vec![joined_id],
            1,
            NamespaceOp::Root(RootOp::MemberJoined {
                member: yara,
                signed_invitation: invitation(&owner_sk, ns),
                account: crate::test_support::credential(&yara_sk.public_key()),
            }),
        )
        .unwrap();
        land(&store, namespace_id, &yara_joined, &[joined_id]);
        let yara_joined_id = yara_joined.content_hash().unwrap();

        let s_key = [0x5Fu8; 32];
        let rotated_key = [0x5Eu8; 32];
        let rotation = GroupKeyring::new(&store, s)
            .build_rotation(
                &rotated_key,
                &mallory_sk,
                &[KeyRecipient::Member(node_sk.public_key())],
            )
            .unwrap();
        let add = SignedNamespaceOp::sign(
            &mallory_sk,
            namespace_id.into(),
            vec![created_id, yara_joined_id],
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
                key_rotation: Some(rotation),
                registers: None,
            },
        )
        .unwrap();
        let _ = arrive(&store, &add, &[created_id, yara_joined_id]);
        let _ = adopt_pulled_group_key(&store, namespace_id.into(), s, &s_key)
            .expect("the pulled key stores");

        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&s, &yara)
                .unwrap(),
            Some(GroupMemberRole::Member),
            "control: the op applied at its cut"
        );
        assert_eq!(
            GroupKeyring::new(&store, s)
                .load_key_by_id(&GroupKeyring::key_id_for(&rotated_key))
                .unwrap(),
            Some(rotated_key),
            "the rotation its signer was entitled to at the cut is stored, as on a keyed node"
        );
    }

    /// A parked op whose cut cannot be resolved is not judged: the live rows are another
    /// cut, so neither an apply nor a refusal there is the verdict at its own.
    #[test]
    fn a_parked_op_with_an_unresolvable_cut_stays_unjudged() {
        let ns = ContextGroupId::from([0x53; 32]);
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());
        let yara_sk = PrivateKey::from([0x73; 32]);
        let yara = crate::test_support::enrol(&store, &ns, &yara_sk.public_key());
        MembershipRepository::new(&store)
            .add_member(&ns, &yara, GroupMemberRole::Member)
            .unwrap();
        let joined_id = land_mallory_join(&store, ns, &owner_sk, &mallory_sk);
        let owner_joined = land_mallory_join(&store, ns, &owner_sk, &owner_sk);
        let ns_key = [0x62u8; 32];
        let cut = vec![joined_id, owner_joined, [0xEEu8; 32]];
        let by_owner = role_set(&owner_sk, ns, cut.clone(), 2, &ns_key, yara);
        let by_mallory = role_set(&mallory_sk, ns, cut.clone(), 2, &ns_key, mallory);
        let owner_op = arrive(&store, &by_owner, &cut);
        let mallory_op = arrive(&store, &by_mallory, &cut);

        let _ = adopt_pulled_group_key(&store, ns.to_bytes().into(), ns, &ns_key)
            .expect("the pulled key stores");

        let members = MembershipRepository::new(&store);
        assert_eq!(
            members.role_of(&ns, &yara).unwrap(),
            Some(GroupMemberRole::Member),
            "the owner's op is not applied on the live rows"
        );
        assert_eq!(
            members.role_of(&ns, &mallory).unwrap(),
            Some(GroupMemberRole::Member),
            "nor is Mallory's"
        );
        for (op, id) in [(&by_owner, owner_op), (&by_mallory, mallory_op)] {
            assert_eq!(
                calimero_governance_store::parked_op(&store, op.namespace_id, id).unwrap(),
                Some(calimero_governance_store::Parked::Undecided),
                "an op whose cut is unresolvable stays parked"
            );
        }
    }

    /// An op a replay refused at its cut stays refused when a later replay cannot
    /// resolve that cut, even where the live rows would admit it.
    #[test]
    fn a_refused_parked_op_stays_refused_when_its_cut_turns_unresolvable() {
        let ns = ContextGroupId::from([0x54; 32]);
        let namespace_id = ns.to_bytes();
        let (store, owner_sk, mallory_sk) = keyless_namespace(ns);
        let mallory = crate::test_support::account_for(&mallory_sk.public_key());
        let yara_sk = PrivateKey::from([0x73; 32]);
        let yara = crate::test_support::enrol(&store, &ns, &yara_sk.public_key());
        MembershipRepository::new(&store)
            .add_member(&ns, &yara, GroupMemberRole::Member)
            .unwrap();
        let joined_id = land_mallory_join(&store, ns, &owner_sk, &mallory_sk);
        let owner_joined = land_mallory_join(&store, ns, &owner_sk, &owner_sk);
        let cut = vec![joined_id, owner_joined];
        let ns_key = [0x63u8; 32];
        let add = role_set(&mallory_sk, ns, cut.clone(), 2, &ns_key, yara);
        let id = arrive(&store, &add, &cut);
        // The live rows show Mallory an admin; at the op's cut she is none.
        MembershipRepository::new(&store)
            .set_role(&ns, &mallory, GroupMemberRole::Admin)
            .unwrap();

        let _ = adopt_pulled_group_key(&store, namespace_id.into(), ns, &ns_key)
            .expect("the pulled key stores");
        let verdict = || calimero_governance_store::parked_op(&store, add.namespace_id, id);
        assert_eq!(
            verdict().unwrap(),
            Some(calimero_governance_store::Parked::Refused),
            "control: the replay refused the op at its cut"
        );

        let mut handle = store.handle();
        handle
            .delete(&calimero_store::key::ScopeUnifiedOp::new(
                namespace_id,
                owner_joined,
            ))
            .unwrap();
        drop(handle);
        let _ = adopt_pulled_group_key(&store, namespace_id.into(), ns, &ns_key)
            .expect("the pulled key stores");

        assert_eq!(
            verdict().unwrap(),
            Some(calimero_governance_store::Parked::Refused),
            "the verdict stands"
        );
        assert_ne!(
            MembershipRepository::new(&store)
                .role_of(&ns, &yara)
                .unwrap(),
            Some(GroupMemberRole::Admin),
            "an op refused at its cut is not applied by the live rows"
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

    /// An op of a subgroup that arrives while the subgroup's key-arrival replay
    /// is still running is held, and the refresh that ends the key recovery must
    /// apply it.
    ///
    /// Until the replay judges the subgroup's parked op, the fold reads that op
    /// as a hole, so the gate holds a readable op for a key this node already
    /// stored. The replay only re-drives what it collected before the op
    /// arrived, and no later key arrives to replay again: the member row never
    /// lands and the group's state hash differs from its peers' for good.
    #[test]
    fn an_op_held_during_its_keys_replay_applies_once_the_key_recovery_ends() {
        let ns = ContextGroupId::from([0x55; 32]);
        let namespace_id = ns.to_bytes();
        let (store, owner_sk, _mallory_sk) = keyless_namespace(ns);
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let bob_sk = PrivateKey::from([0x76; 32]);
        let bob = crate::test_support::enrol(&store, &ns, &bob_sk.public_key());
        MembershipRepository::new(&store)
            .add_member(&ns, &bob, GroupMemberRole::Member)
            .unwrap();

        let sub_salt = [0x77u8; 32];
        let sub = ContextGroupId::from(calimero_account::created_subgroup_id(
            &owner,
            &namespace_id,
            true,
            &sub_salt,
        ));
        NamespaceRepository::new(&store).nest(&ns, &sub).unwrap();
        MetaRepository::new(&store)
            .save(&sub, &meta(owner))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&sub, &owner, GroupMemberRole::Admin)
            .unwrap();
        // The owner's key speaks for its account at the cut only through a folded credential.
        let owner_joined = land_mallory_join(&store, ns, &owner_sk, &owner_sk);
        let created = SignedNamespaceOp::sign(
            &owner_sk,
            namespace_id.into(),
            vec![owner_joined],
            2,
            NamespaceOp::Root(RootOp::GroupCreated {
                admin: owner,
                group_id: sub.to_bytes().into(),
                parent_id: namespace_id.into(),
                restricted: true,
                salt: sub_salt,
            }),
        )
        .unwrap();
        land(&store, namespace_id, &created, &[owner_joined]);
        let created_id = created.content_hash().unwrap();
        let bob_joined = land_mallory_join(&store, ns, &owner_sk, &bob_sk);

        // Sealed under the subgroup's own key, which this node does not hold
        // yet: it lands unreadable in the op-store.
        let sub_key = [0x78u8; 32];
        let sealed = |nonce: u64, parents: Vec<[u8; 32]>, op: GroupOp| {
            SignedNamespaceOp::sign(
                &owner_sk,
                namespace_id.into(),
                parents,
                nonce,
                NamespaceOp::Group {
                    group_id: sub.to_bytes().into(),
                    key_id: GroupKeyring::key_id_for(&sub_key).into(),
                    encrypted: GroupKeyring::encrypt_op(&sub_key, &op).unwrap(),
                    key_rotation: None,
                    registers: op.registered_context(),
                },
            )
            .unwrap()
        };
        let named = sealed(
            3,
            vec![created_id, bob_joined],
            GroupOp::GroupMetadataSet {
                name: Some("K".to_owned()),
                data: std::collections::BTreeMap::new(),
            },
        );
        let named_id = arrive(&store, &named, &[created_id, bob_joined]);

        // The key is stored, and its replay has collected what it will re-drive
        // but not yet judged the parked op: until it does, the fold reads that op
        // as a hole. The add arrives in this window, readable, and is held.
        let _ = GroupKeyring::new(&store, sub).store_key(&sub_key).unwrap();
        let add = sealed(
            4,
            vec![named_id],
            GroupOp::MemberAdded {
                member: bob,
                role: GroupMemberRole::ReadOnly,
            },
        );
        let _ = calimero_governance_store::apply_signed_namespace_op_at_cut(
            &store,
            &add,
            &[named_id],
            &crate::apply_authorizer::EphemeralProjectionAuthorizer::new(&store),
        )
        .expect("an op whose own group's history reads as sealed is held, not refused");
        let add_id = add.content_hash().unwrap();
        let held = || {
            calimero_governance_store::held_ops::HeldOps::new(&store, namespace_id.into())
                .read_live()
                .unwrap()
                .ops
                .iter()
                .any(|op| op.delta_id == add_id)
        };
        assert!(
            held(),
            "precondition: the add is held while its key's replay is in flight"
        );
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&sub, &bob)
                .unwrap(),
            None,
            "precondition: the held add wrote no member row"
        );

        let _ = refresh_after_group_key(&store, namespace_id.into(), sub);

        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&sub, &bob)
                .unwrap(),
            Some(GroupMemberRole::ReadOnly),
            "the add must apply once the key recovery ends"
        );
        assert!(!held(), "an applied op is no longer listed as held");
    }
    /// `joiner_sk`'s join of `ns`, admitted by the owner and sealed under `key`.
    fn sealed_admitted_join(
        owner_sk: &PrivateKey,
        ns: ContextGroupId,
        joiner_sk: &PrivateKey,
        key: &[u8; 32],
    ) -> SignedNamespaceOp {
        let owner = crate::test_support::account_for(&owner_sk.public_key());
        let member = crate::test_support::account_for(&joiner_sk.public_key());
        let nonce = [0x43; 32];
        let join = RootOp::MemberJoined {
            member,
            signed_invitation: invitation_with(owner_sk, ns, nonce, vec![owner]),
            account: crate::test_support::credential(&joiner_sk.public_key()),
        };
        let mut signed = SignedNamespaceOp::sign(
            joiner_sk,
            ns.to_bytes().into(),
            vec![],
            1,
            NamespaceOp::RootSealed {
                key_id: GroupKeyring::key_id_for(key).into(),
                encrypted: GroupKeyring::encrypt_root_op(key, &join).unwrap(),
            },
        )
        .unwrap();
        signed.admitter_endorsement = Some(Box::new(
            calimero_governance_types::AdmitterEndorsement::sign(
                owner_sk,
                &ns.to_bytes(),
                &member,
                &nonce,
            )
            .unwrap(),
        ));
        signed
    }

    /// A device link sealed under one namespace key, endorsed by an account whose
    /// join is sealed under another, binds the device whichever key arrives first.
    #[test]
    fn a_device_link_behind_a_sealed_join_binds_in_either_key_order() {
        let link_key = [0x65u8; 32];
        let join_key = [0x66u8; 32];
        for (seed, keys) in [(0x55, [link_key, join_key]), (0x56, [join_key, link_key])] {
            let ns = ContextGroupId::from([seed; 32]);
            let namespace_id = ns.to_bytes();
            let (store, owner_sk, _mallory_sk) = keyless_namespace(ns);
            let member_sk = PrivateKey::from([0x76; 32]);
            let member_pk = member_sk.public_key();
            let account = crate::test_support::account_for(&member_pk);

            let join = sealed_admitted_join(&owner_sk, ns, &member_sk, &join_key);
            let join_id = arrive(&store, &join, &[]);

            // `credential` derives the account root from the member's key.
            let root_sk = PrivateKey::from(*member_pk);
            let device_pk = PrivateKey::from([0x77; 32]).public_key();
            let cert = calimero_account::DeviceCert::sign(
                &root_sk,
                account,
                calimero_account::DeviceId::mint(account, [0x78; 16]),
                &device_pk,
                &calimero_account::KemPublicKey::from([0x79; 32]),
                0,
                0,
            )
            .unwrap();
            let link = SignedNamespaceOp::sign(
                &member_sk,
                namespace_id.into(),
                vec![join_id],
                2,
                NamespaceOp::Group {
                    group_id: namespace_id.into(),
                    key_id: GroupKeyring::key_id_for(&link_key).into(),
                    encrypted: GroupKeyring::encrypt_op(
                        &link_key,
                        &GroupOp::AccountDeviceLinked {
                            genesis: calimero_account::AccountGenesis::new(root_sk.public_key()),
                            chain: vec![],
                            cert,
                            endorsement: calimero_account::AccountMemberEndorsement::sign(
                                &member_sk, account,
                            )
                            .unwrap(),
                            scope: Box::new(crate::test_support::device_scope(
                                &root_sk,
                                &cert,
                                &[],
                                0,
                            )),
                        },
                    )
                    .unwrap(),
                    key_rotation: None,
                    registers: None,
                },
            )
            .unwrap();
            let link_id = arrive(&store, &link, &[join_id]);
            let bound = || {
                calimero_governance_store::member_account_in_namespace(&store, &ns, &device_pk)
                    .unwrap()
            };
            let parked =
                || calimero_governance_store::parked_op(&store, namespace_id.into(), link_id);

            let _ = adopt_pulled_group_key(&store, namespace_id.into(), ns, &keys[0])
                .expect("the pulled key stores");
            assert_eq!(
                parked().unwrap(),
                Some(calimero_governance_store::Parked::Undecided),
                "a link whose endorser's join is still sealed stays parked"
            );
            assert_eq!(bound(), None, "and binds nothing yet");

            let _ = adopt_pulled_group_key(&store, namespace_id.into(), ns, &keys[1])
                .expect("the pulled key stores");
            assert_eq!(
                bound(),
                Some(account),
                "the link binds once both keys are held"
            );
            assert_eq!(parked().unwrap(), None, "and is no longer parked");
        }
    }
}
