//! Owner-level ops: the bare forms fold to nothing, the wrapped form folds only
//! with a proof that is internally valid for the op it wraps, and the fold and
//! the at-cut decision agree with the live apply about a replayed proof.

use calimero_account::{
    AccountGenesis, AccountId, KemPublicKey, OwnerOpAuthorization, OwnerOpKind, OwnerOpTerms,
    SignedOwnerOp,
};
use calimero_authz::{authorize, DeviceBinding, Rejected};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::{GroupOp, RootOp};
use calimero_op::{Op, OpPayload, ScopeId};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_projection::ScopeState;

use super::support::{authorship_of, hlc};
use crate::{payload_from_group_op, payload_from_root_op};

const GROUP: [u8; 32] = [0x5A; 32];

fn group() -> ContextGroupId {
    ContextGroupId::from(GROUP)
}

fn account_of(root: &PrivateKey) -> AccountId {
    AccountGenesis::new(root.public_key()).account_id()
}

/// `root`'s proof for `op` on `group`, at `counter`.
fn proof(
    root: &PrivateKey,
    group_id: [u8; 32],
    kind: OwnerOpKind,
    digest: [u8; 32],
    counter: u64,
) -> SignedOwnerOp {
    let genesis = AccountGenesis::new(root.public_key());
    SignedOwnerOp {
        genesis,
        chain: vec![],
        statement: OwnerOpAuthorization::sign(
            root,
            OwnerOpTerms {
                account: genesis.account_id(),
                namespace_id: GROUP,
                group_id,
                kind,
                op_digest: digest,
                counter,
                key_epoch: 0,
            },
        )
        .unwrap(),
    }
}

fn transfer(new_owner: AccountId) -> GroupOp {
    GroupOp::TransferOwnership { new_owner }
}

fn guarded_transfer(root: &PrivateKey, new_owner: AccountId, counter: u64) -> GroupOp {
    let inner = transfer(new_owner);
    let digest = inner.owner_op_digest().unwrap();
    GroupOp::RootGuarded {
        op: Box::new(inner),
        proof: Box::new(proof(
            root,
            GROUP,
            OwnerOpKind::TransferOwnership,
            digest,
            counter,
        )),
    }
}

#[test]
fn bare_owner_level_ops_fold_to_nothing() {
    let someone = AccountId::from([0x44; 32]);
    for op in [
        transfer(someone),
        GroupOp::GroupDelete,
        GroupOp::TeeAuthoringPolicySet {
            allowed_mrtd: vec!["aa".to_owned()],
        },
    ] {
        assert_eq!(payload_from_group_op(group(), &op), None, "{op:?}");
    }
    assert_eq!(
        payload_from_root_op(&RootOp::AdminChanged { new_admin: someone }),
        None
    );
}

#[test]
fn a_guarded_op_folds_as_the_op_it_carries_with_its_proof_terms() {
    let root = PrivateKey::from([0x21; 32]);
    let owner = account_of(&root);
    let new_owner = AccountId::from([0x44; 32]);
    let Some(OpPayload::RootGuarded {
        carried,
        group: folded_group,
        account,
        counter,
        ..
    }) = payload_from_group_op(group(), &guarded_transfer(&root, new_owner, 7))
    else {
        panic!("a valid guarded op folds as RootGuarded");
    };
    assert_eq!(
        *carried,
        OpPayload::AdminChanged {
            new_admin: new_owner
        }
    );
    assert_eq!((folded_group, account, counter), (group(), owner, 7));

    // A guarded op the projection models nothing about still folds, as a
    // `Noop` carried: the node is what the counter counts.
    let inner = GroupOp::GroupDelete;
    let digest = inner.owner_op_digest().unwrap();
    let delete = GroupOp::RootGuarded {
        op: Box::new(inner),
        proof: Box::new(proof(&root, GROUP, OwnerOpKind::GroupDelete, digest, 0)),
    };
    assert!(matches!(
        payload_from_group_op(group(), &delete),
        Some(OpPayload::RootGuarded { carried, .. }) if *carried == OpPayload::Noop
    ));

    // The root-op form, whose group is the namespace root.
    let inner = RootOp::AdminChanged { new_admin: owner };
    let digest = inner.owner_op_digest().unwrap();
    let root_op = RootOp::RootGuarded {
        op: Box::new(inner),
        proof: Box::new(proof(&root, GROUP, OwnerOpKind::AdminChanged, digest, 0)),
    };
    assert!(matches!(
        payload_from_root_op(&root_op),
        Some(OpPayload::RootGuarded { group: g, .. }) if g == group()
    ));
}

#[test]
fn a_proof_that_does_not_authorise_the_op_it_wraps_folds_to_nothing() {
    let root = PrivateKey::from([0x21; 32]);
    let x = AccountId::from([0x44; 32]);
    let y = AccountId::from([0x45; 32]);
    let GroupOp::RootGuarded { proof: for_x, .. } = guarded_transfer(&root, x, 0) else {
        unreachable!()
    };

    // Signed for "transfer to x", wrapped around "transfer to y".
    let swapped = GroupOp::RootGuarded {
        op: Box::new(transfer(y)),
        proof: for_x.clone(),
    };
    assert_eq!(payload_from_group_op(group(), &swapped), None);

    // Presented in another group.
    assert_eq!(
        payload_from_group_op(
            ContextGroupId::from([0x99; 32]),
            &guarded_transfer(&root, x, 0)
        ),
        None
    );

    // A forged signature.
    let mut forged = for_x;
    forged.statement.signature = [0u8; 64];
    let forged = GroupOp::RootGuarded {
        op: Box::new(transfer(x)),
        proof: forged,
    };
    assert_eq!(payload_from_group_op(group(), &forged), None);
}

/// The fold and the at-cut decision agree with the live apply: the projection
/// never folds a bare transfer, counts a guarded one, and `authorize` refuses a
/// proof that names a spent counter at a cut that includes the op it spent.
#[test]
fn the_fold_and_the_decision_agree_with_the_live_guard() {
    let owner_root = PrivateKey::from([0x21; 32]);
    let owner = account_of(&owner_root);
    let heir_root = PrivateKey::from([0x23; 32]);
    let heir = account_of(&heir_root);
    let owner_key = PublicKey::from([0x31; 32]);
    let heir_key = PublicKey::from([0x32; 32]);

    let op = |author, key, payload, parents: Vec<[u8; 32]>, ns| {
        Op::new(
            ScopeId::from(GROUP),
            parents,
            authorship_of(author, key),
            hlc(ns),
            payload,
            [0u8; 32],
            [0u8; 64],
        )
    };
    let seed = op(
        owner,
        owner_key,
        OpPayload::AdminChanged { new_admin: owner },
        vec![],
        1,
    );

    // A bare transfer, signed by the owner's device: nothing to fold.
    let bare = op(
        owner,
        owner_key,
        payload_from_group_op(group(), &transfer(heir)).unwrap_or(OpPayload::Noop),
        vec![seed.id()],
        2,
    );
    let state = ScopeState::from_ops([&seed, &bare]);
    let view = state.acl_view();
    assert_eq!(
        view.root_admin,
        Some(owner),
        "a bare transfer moved nothing"
    );
    assert_eq!(view.owner_op_count(&group()), 0);

    // The guarded transfer, with the owner's root proof at counter 0.
    let guarded = op(
        owner,
        owner_key,
        payload_from_group_op(group(), &guarded_transfer(&owner_root, heir, 0)).unwrap(),
        vec![bare.id()],
        3,
    );
    let log = vec![seed.clone(), bare.clone(), guarded.clone()];
    let mut before = ScopeState::acl_view_at(&log, &[bare.id()]);
    bind(&mut before, owner, owner_key);
    assert_eq!(
        authorize(&guarded, &before),
        Ok(()),
        "authorized at its own cut"
    );

    let mut after = ScopeState::acl_view_at(&log, &[guarded.id()]);
    assert_eq!(after.root_admin, Some(heir));
    assert_eq!(after.owner_op_count(&group()), 1);
    bind(&mut after, heir, heir_key);

    // The same counter again, at a cut that includes the op that spent it: the
    // live apply refuses it as `StaleCounter`, and so does the decision.
    let replay = op(
        heir,
        heir_key,
        payload_from_group_op(group(), &guarded_transfer(&heir_root, heir, 0)).unwrap(),
        vec![guarded.id()],
        4,
    );
    assert_eq!(
        authorize(&replay, &after),
        Err(Rejected::OwnerOpCounterStale {
            expected: 1,
            found: 0
        })
    );
    let fresh = op(
        heir,
        heir_key,
        payload_from_group_op(group(), &guarded_transfer(&heir_root, heir, 1)).unwrap(),
        vec![guarded.id()],
        4,
    );
    assert_eq!(authorize(&fresh, &after), Ok(()));
}

/// Stand in for the `DeviceLinked` op a real log would carry, so the decision
/// reaches the rule under test instead of the device precondition.
fn bind(view: &mut calimero_authz::AclView, account: AccountId, key: PublicKey) {
    let _ = view.devices.insert(
        calimero_account::DeviceId::from(*account.as_bytes()),
        DeviceBinding {
            account,
            sign_pk: key,
            kem_pk: KemPublicKey::from([0u8; 32]),
            device_epoch: 0,
            key_epoch: 0,
        },
    );
}
