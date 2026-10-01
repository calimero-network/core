//! The publishing half of the root guard: attach a root proof to an owner-level
//! op before it is signed and published. The apply half, and the reasons, are in
//! `calimero_governance_store::owner_guard`.
//!
//! A proof arrives one of two ways:
//!
//! - **Supplied by the caller.** Signed offline by whoever holds the account
//!   root: a nodeless account, or a root kept cold. The node only publishes it.
//! - **Minted by this node**, when the caller supplies none and this node holds
//!   the root of the account its device speaks for (`merod init` provisions one).
//!   The node reads the group's guarded-op counter and signs at epoch 0.
//!
//! With neither, the op is refused with a `403` that says a root proof is needed.
//! Either way the proof is checked here before anything is published, so a bad one
//! is reported to the caller instead of being refused silently by every replica.
//!
//! A root held on the node guards against a leaked or stolen *other* device key.
//! It does not guard against compromise of this node, which holds the root.

use calimero_account::{AccountId, OwnerOpAuthorization, OwnerOpTerms, SignedOwnerOp};
use calimero_context_client::local_governance::{GroupOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    check_root_proof, member_account_in_namespace, owner_op_counter, GuardedOp,
    NamespaceRepository, NodeDeviceRepository, OwnerGuardRefusal,
};
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use crate::error::ContextError;

/// `op` wrapped in [`GroupOp::RootGuarded`] with a proof for `signer` on
/// `group_id`: `supplied` if given, else one this node mints.
///
/// # Errors
/// [`ContextError::RootProofRequired`] when there is no proof to be had, an
/// [`OwnerGuardRefusal`] when the proof does not authorise the op, or a store
/// failure.
pub fn guard_group_op(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    op: GroupOp,
    supplied: Option<SignedOwnerOp>,
) -> EyreResult<GroupOp> {
    let Some(kind) = op.owner_op_kind() else {
        bail!(OwnerGuardRefusal::NotAGuardedKind {
            inner: op.op_kind_label(),
        });
    };
    let guarded = GuardedOp {
        namespace: NamespaceRepository::new(store).resolve(group_id)?,
        group: *group_id,
        kind,
        digest: op.owner_op_digest()?,
    };
    let proof = proof_for(store, signer, guarded, supplied)?;
    Ok(GroupOp::RootGuarded {
        op: Box::new(op),
        proof: Box::new(proof),
    })
}

/// [`guard_group_op`] for a root op, whose group is the namespace root.
///
/// # Errors
/// As [`guard_group_op`].
pub fn guard_root_op(
    store: &Store,
    namespace_id: &ContextGroupId,
    signer: &PublicKey,
    op: RootOp,
    supplied: Option<SignedOwnerOp>,
) -> EyreResult<RootOp> {
    let Some(kind) = op.owner_op_kind() else {
        bail!(OwnerGuardRefusal::NotAGuardedKind {
            inner: "a root op that needs no proof",
        });
    };
    let guarded = GuardedOp {
        namespace: *namespace_id,
        group: *namespace_id,
        kind,
        digest: op.owner_op_digest()?,
    };
    let proof = proof_for(store, signer, guarded, supplied)?;
    Ok(RootOp::RootGuarded {
        op: Box::new(op),
        proof: Box::new(proof),
    })
}

fn proof_for(
    store: &Store,
    signer: &PublicKey,
    op: GuardedOp,
    supplied: Option<SignedOwnerOp>,
) -> EyreResult<SignedOwnerOp> {
    let Some(account) = member_account_in_namespace(store, &op.group, signer)? else {
        bail!(OwnerGuardRefusal::SignerUnbound);
    };
    let proof = match supplied {
        Some(proof) => proof,
        None => mint(store, account, op)?,
    };
    check_root_proof(store, account, op, &proof)?;
    Ok(proof)
}

/// A proof signed by this node's own root, when it holds the root of
/// `account`.
fn mint(store: &Store, account: AccountId, op: GuardedOp) -> EyreResult<SignedOwnerOp> {
    let root = NodeDeviceRepository::new(store).holder_root()?;
    let Some(root) = root.filter(|root| root.account() == account) else {
        bail!(ContextError::RootProofRequired {
            reason: format!(
                "{} is an owner-level op and needs a proof signed by the root key of account \
                 {account}, which this node does not hold. Sign one offline with that root \
                 (the group's current guarded-op counter is {}) and pass it as `rootProof`, \
                 or run this on a node that holds the root",
                op.kind.label(),
                owner_op_counter(store, &op.group)?,
            ),
        });
    };
    let terms = OwnerOpTerms {
        account,
        namespace_id: op.namespace.to_bytes(),
        group_id: op.group.to_bytes(),
        kind: op.kind,
        op_digest: op.digest,
        counter: owner_op_counter(store, &op.group)?,
        // The node's root is the genesis key: a node holds no handoff chain, so
        // an account that rotated elsewhere cannot mint here, and the floor
        // check in `check_root_proof` says so.
        key_epoch: 0,
    };
    Ok(SignedOwnerOp {
        genesis: root.genesis(),
        chain: vec![],
        statement: OwnerOpAuthorization::sign(root.signing_key(), terms)
            .map_err(|err| eyre::eyre!("failed to sign the root proof: {err}"))?,
    })
}

/// Decode a hex, borsh-encoded [`SignedOwnerOp`], naming which stage failed.
///
/// # Errors
/// A sentence for a `400`: not hex, or hex that is not a proof.
pub fn decode_root_proof(raw: &str) -> Result<SignedOwnerOp, String> {
    let bytes =
        hex::decode(raw.trim()).map_err(|err| format!("rootProof is not valid hex: {err}"))?;
    borsh::from_slice(&bytes).map_err(|err| {
        format!(
            "rootProof is valid hex but not a borsh SignedOwnerOp (AccountProof of an \
             OwnerOpAuthorization): {err}"
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::{
        AccountGenesis, DeviceCert, DeviceId, KemPublicKey, OwnerOpKind, OwnerOpTerms,
    };
    use calimero_context_client::local_governance::SignedGroupOp;
    use calimero_governance_store::{
        apply_local_signed_group_op, AccountBindingRepository, MembershipRepository, MetaRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupTarget};

    use super::*;

    const GROUP: [u8; 32] = [0x5B; 32];

    /// Bind `device_sk`'s key into `group` for the account rooted at `root`.
    fn bind(store: &Store, root: &PrivateKey, device_sk: &PrivateKey, seed: u8) -> AccountId {
        let genesis = AccountGenesis::new(root.public_key());
        let account = genesis.account_id();
        let cert = DeviceCert::sign(
            root,
            account,
            DeviceId::from([seed; 32]),
            &device_sk.public_key(),
            &KemPublicKey::from([seed; 32]),
            0,
            0,
        )
        .unwrap();
        let bindings = AccountBindingRepository::new(store);
        let group = ContextGroupId::from(GROUP);
        bindings
            .apply_link(&group, &genesis, &[], &cert, 0)
            .unwrap()
            .expect("the cert is admissible");
        bindings.record_endorser(&group, account, &account).unwrap();
        MembershipRepository::new(store)
            .add_member(&group, &account, GroupMemberRole::Admin)
            .unwrap();
        account
    }

    fn group_owned_by(store: &Store, owner: AccountId) {
        MetaRepository::new(store)
            .save(
                &ContextGroupId::from(GROUP),
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: [0xCC; 32].into(),
                        bytecode_id: [0xBB; 32],
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: owner,
                    owner_identity: owner,
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
    }

    /// A node that holds the account root signs the proof itself, and the op
    /// it produces applies.
    #[test]
    fn a_node_holding_the_root_signs_the_proof_itself() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let root = NodeDeviceRepository::new(&store)
            .provision_account_root()
            .unwrap();
        let device_sk = PrivateKey::from([0x41; 32]);
        let owner = bind(&store, root.signing_key(), &device_sk, 0x41);
        assert_eq!(owner, root.account());
        let heir = bind(
            &store,
            &PrivateKey::from([0x52; 32]),
            &PrivateKey::from([0x42; 32]),
            0x42,
        );
        group_owned_by(&store, owner);

        let group = ContextGroupId::from(GROUP);
        let op = guard_group_op(
            &store,
            &group,
            &device_sk.public_key(),
            GroupOp::TransferOwnership { new_owner: heir },
            None,
        )
        .expect("the node mints the proof from its own root");
        let GroupOp::RootGuarded { proof, .. } = &op else {
            panic!("wrapped");
        };
        assert_eq!(proof.statement.account, owner);
        assert_eq!(proof.statement.counter, 0);

        apply_local_signed_group_op(
            &store,
            &SignedGroupOp::sign(&device_sk, group.to_bytes().into(), vec![], 1, op).unwrap(),
        )
        .expect("the guarded transfer applies");
        assert_eq!(
            MetaRepository::new(&store)
                .load(&group)
                .unwrap()
                .unwrap()
                .owner_identity,
            heir
        );
        assert_eq!(owner_op_counter(&store, &group).unwrap(), 1);
    }

    /// A node that does not hold the signing account's root refuses without a
    /// proof, naming what is needed, and publishes a proof the caller supplies.
    #[test]
    fn without_the_root_a_proof_is_required_and_a_supplied_one_is_used() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        // The node's own root is some other account's.
        let _node_root = NodeDeviceRepository::new(&store)
            .provision_account_root()
            .unwrap();
        let nodeless_root = PrivateKey::from([0x53; 32]);
        let device_sk = PrivateKey::from([0x43; 32]);
        let owner = bind(&store, &nodeless_root, &device_sk, 0x43);
        group_owned_by(&store, owner);
        let group = ContextGroupId::from(GROUP);
        let op = GroupOp::TeeAuthoringPolicySet {
            allowed_mrtd: vec!["aa".to_owned()],
        };

        let err = guard_group_op(&store, &group, &device_sk.public_key(), op.clone(), None)
            .expect_err("no proof, and no root to sign one");
        assert!(
            matches!(
                err.downcast_ref::<ContextError>(),
                Some(ContextError::RootProofRequired { .. })
            ),
            "{err:#}"
        );

        // Signed offline by the nodeless root.
        let terms = OwnerOpTerms {
            account: owner,
            namespace_id: GROUP,
            group_id: GROUP,
            kind: OwnerOpKind::TeeAuthoringPolicy,
            op_digest: op.owner_op_digest().unwrap(),
            counter: 0,
            key_epoch: 0,
        };
        let supplied = SignedOwnerOp {
            genesis: AccountGenesis::new(nodeless_root.public_key()),
            chain: vec![],
            statement: OwnerOpAuthorization::sign(&nodeless_root, terms).unwrap(),
        };
        let wrapped = guard_group_op(
            &store,
            &group,
            &device_sk.public_key(),
            op.clone(),
            Some(supplied.clone()),
        )
        .expect("a supplied proof is used as given");
        apply_local_signed_group_op(
            &store,
            &SignedGroupOp::sign(&device_sk, group.to_bytes().into(), vec![], 1, wrapped).unwrap(),
        )
        .expect("and applies");

        // Supplied again, it is spent: refused here, before anything is published.
        let err = guard_group_op(&store, &group, &device_sk.public_key(), op, Some(supplied))
            .expect_err("spent");
        assert!(
            matches!(
                err.downcast_ref::<OwnerGuardRefusal>(),
                Some(OwnerGuardRefusal::StaleCounter { .. })
            ),
            "{err:#}"
        );
    }

    #[test]
    fn a_root_proof_that_is_not_one_is_named_as_such() {
        assert!(decode_root_proof("zz")
            .unwrap_err()
            .contains("not valid hex"));
        assert!(decode_root_proof("00")
            .unwrap_err()
            .contains("not a borsh SignedOwnerOp"));
    }
}
