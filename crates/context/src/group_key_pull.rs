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
    if let Err(err) = calimero_governance_store::retry_encrypted_ops_for_group(
        store,
        namespace_id,
        group_id.to_bytes(),
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
mod tests {
    use std::sync::Arc;

    use calimero_context_client::local_governance::{
        GroupOp, NamespaceOp, RootOp, SignedNamespaceOp,
    };
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::VisibilityMode;
    use calimero_governance_store::{
        CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository,
        NamespaceDagService, NamespaceOpLogService, NamespaceRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;
    use rand::rngs::OsRng;

    use super::adopt_pulled_group_key;

    fn meta(admin: calimero_account::AccountId) -> GroupMetaValue {
        GroupMetaValue {
            bytecode_id: [0xBB; 32],
            target_application_id: calimero_primitives::application::ApplicationId::from(
                [0xCC; 32],
            ),
            created_at: 1_700_000_000,
            admin_identity: admin,
            owner_identity: admin,
            migration: None,
            auto_join: true,
        }
    }

    /// Put a signed op on the DAG the way a received op lands. The op-log write
    /// also writes the unified op, decoding with whatever key is present then.
    fn land(store: &Store, namespace: [u8; 32], op: &SignedNamespaceOp, parents: &[[u8; 32]]) {
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
    fn invitation(
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
            admitter_hints: Vec::new(),
        }
    }

    /// A third node cannot join a namespace through an inherited `Open` subgroup
    /// unless a peer-pulled key recovers BOTH planes.
    ///
    /// The apply gate resolves the membership path from the unified op-store at
    /// the op's causal cut, so re-driving only the live plane leaves the gate
    /// reading the frozen `Noop` and still rejecting the join.
    #[test]
    fn a_pulled_key_unwedges_another_members_open_subgroup_join() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut rng = OsRng;

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

        let sub = ContextGroupId::from(*PrivateKey::random(&mut rng).public_key());
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

        let join = SignedNamespaceOp::sign(
            &joiner_sk,
            namespace_id.into(),
            vec![flip_id],
            4,
            NamespaceOp::Root(RootOp::MemberJoinedOpen {
                member: joiner_account,
                group_id: sub.to_bytes().into(),
                account: crate::test_support::credential(&joiner),
            }),
        )
        .unwrap();
        let apply = |store: &Store| {
            calimero_governance_store::apply_signed_namespace_op_at_cut(
                store,
                &join,
                &[flip_id],
                &crate::apply_authorizer::EphemeralProjectionAuthorizer::new(store),
            )
        };

        // The wedge is an ABSTENTION, not a rejection, and the difference matters.
        // The flip sits in this cut unreadable, so the node does not know whether
        // the subgroup is Open; a node holding the key does and would accept this
        // join. Answering "no membership path" from a fold that is missing the
        // flip is a verdict about a history this node cannot see, and it is the
        // verdict the other node contradicts. Parking until the key arrives is
        // what converges — and the rest of this test is precisely that arrival.
        let wedged = apply(&store).expect_err("the subgroup still reads Restricted");
        let wedged = format!("{wedged:#}");
        assert!(
            wedged.contains("has not folded the ancestry"),
            "the wedge must be an abstention that retries, not a decision taken \
             over an unreadable ancestor, got: {wedged}"
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
        apply(&store).expect("the backfilled join must apply once the flip is readable");

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
    }
}
