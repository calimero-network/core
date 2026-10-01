//! The root guard on owner-level ops (`crate::owner_guard`), driven through the
//! full signed-op apply path.
//!
//! The fixtures derive each member's account root from its signing key
//! (`test_fixtures::root_for`), so a test can sign a root proof for any member.
//! What a test calls "the stolen device" is the signing key alone: it signs the
//! op, and it never signs a root proof, because a device does not hold the root.

use calimero_account::{
    AccountGenesis, AccountId, OwnerOpAuthorization, OwnerOpKind, OwnerOpTerms, RootKeyHandoff,
    SignedOwnerOp,
};
use calimero_context_client::local_governance::{
    GroupOp, RootOp, SignedGroupOp, SignedNamespaceOp, TeeAdmissionMode,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;

use crate::test_fixtures::{
    bootstrap_namespace_with_admin_account, enrol_member, guarded_group_op, guarded_root_op,
    owner_proof_for, root_for, sample_meta_with_admin, seal_for_test, test_group_id, test_store,
};
use crate::{
    apply_local_signed_group_op, owner_op_counter, read_tee_authoring_policy,
    AccountBindingRepository, MembershipError, MembershipRepository, MetaRepository,
    NamespaceGovernance, OwnerGuardRefusal,
};

/// A group whose owner and a second admin are both enrolled, with the owner
/// holding the owner and admin pins. Returns the signing keys and accounts.
struct Fixture {
    store: Store,
    gid: ContextGroupId,
    owner_sk: PrivateKey,
    owner: AccountId,
    admin_sk: PrivateKey,
    admin: AccountId,
    accomplice_sk: PrivateKey,
    accomplice: AccountId,
}

fn fixture() -> Fixture {
    let store = test_store();
    let gid = test_group_id();
    let owner_sk = PrivateKey::from([0x61; 32]);
    let admin_sk = PrivateKey::from([0x62; 32]);
    let accomplice_sk = PrivateKey::from([0x63; 32]);
    let owner = enrol_member(&store, &gid, &owner_sk.public_key());
    let admin = enrol_member(&store, &gid, &admin_sk.public_key());
    let accomplice = enrol_member(&store, &gid, &accomplice_sk.public_key());
    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(owner))
        .unwrap();
    let membership = MembershipRepository::new(&store);
    membership
        .add_member(&gid, &owner, GroupMemberRole::Admin)
        .unwrap();
    membership
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    membership
        .add_member(&gid, &accomplice, GroupMemberRole::Member)
        .unwrap();
    Fixture {
        store,
        gid,
        owner_sk,
        owner,
        admin_sk,
        admin,
        accomplice_sk,
        accomplice,
    }
}

fn sign(sk: &PrivateKey, gid: &ContextGroupId, nonce: u64, op: GroupOp) -> SignedGroupOp {
    SignedGroupOp::sign(sk, gid.to_bytes().into(), vec![], nonce, op).unwrap()
}

fn refusal(err: &eyre::Report) -> &OwnerGuardRefusal {
    err.downcast_ref::<OwnerGuardRefusal>()
        .unwrap_or_else(|| panic!("expected a root-guard refusal, got: {err:#}"))
}

fn owner_of(store: &Store, gid: &ContextGroupId) -> AccountId {
    MetaRepository::new(store)
        .load(gid)
        .unwrap()
        .unwrap()
        .owner_identity
}

fn admission_policy() -> GroupOp {
    GroupOp::TeeAdmissionPolicySetV2 {
        allowed_mrtd: vec!["aa".to_owned()],
        allowed_rtmr0: vec![],
        allowed_rtmr1: vec![],
        allowed_rtmr2: vec![],
        allowed_rtmr3: vec!["bb".to_owned()],
        allowed_tcb_statuses: vec![],
        accept_mock: true,
        mode: TeeAdmissionMode::Replica,
    }
}

fn release_policy() -> GroupOp {
    GroupOp::TeeReleaseAdmissionPolicySetV2 {
        allowed_profiles: vec!["locked-read-only".to_owned()],
        min_release_version: None,
        allowed_tcb_statuses: vec![],
        accept_mock: true,
        mode: TeeAdmissionMode::Replica,
    }
}

/// A bare owner-level op signed by the owner's own device is refused, and
/// changes nothing. Before the guard every one of these applied.
#[test]
fn every_bare_owner_level_op_signed_by_a_device_is_refused() {
    let f = fixture();
    let bare = [
        GroupOp::TransferOwnership { new_owner: f.admin },
        GroupOp::GroupDelete,
        GroupOp::TeeAuthoringPolicySet {
            allowed_mrtd: vec!["aa".to_owned()],
        },
        admission_policy(),
        release_policy(),
    ];
    for (nonce, op) in (1..).zip(bare) {
        let label = op.op_kind_label();
        let err = apply_local_signed_group_op(&f.store, &sign(&f.owner_sk, &f.gid, nonce, op))
            .expect_err(label);
        assert!(
            matches!(refusal(&err), OwnerGuardRefusal::ProofRequired { .. }),
            "{label}: {err:#}"
        );
    }
    assert_eq!(owner_of(&f.store, &f.gid), f.owner, "ownership unchanged");
    assert!(
        MetaRepository::new(&f.store)
            .load(&f.gid)
            .unwrap()
            .is_some(),
        "the group was not deleted"
    );
    assert!(read_tee_authoring_policy(&f.store, &f.gid)
        .unwrap()
        .is_empty());
    assert_eq!(owner_op_counter(&f.store, &f.gid).unwrap(), 0);
}

/// The stolen-device takeover, end to end. The attacker holds the owner's
/// device key and nothing else. It promotes an accomplice, tries to hand them
/// the group, then has the accomplice demote and remove the owner.
///
/// Before the guard every step applied and the owner lost the group for good.
/// Now the transfer is refused, and the rest fails because the owner still owns
/// the group.
#[test]
fn a_stolen_owner_device_can_no_longer_take_the_group() {
    let f = fixture();
    let stolen = &f.owner_sk;

    // 1. Promote the accomplice. Member-level governance: a device may do it.
    apply_local_signed_group_op(
        &f.store,
        &sign(
            stolen,
            &f.gid,
            1,
            GroupOp::MemberRoleSet {
                member: f.accomplice,
                role: GroupMemberRole::Admin,
            },
        ),
    )
    .expect("an owner's device may promote a member");

    // 2. Transfer, bare: refused.
    let err = apply_local_signed_group_op(
        &f.store,
        &sign(
            stolen,
            &f.gid,
            2,
            GroupOp::TransferOwnership {
                new_owner: f.accomplice,
            },
        ),
    )
    .expect_err("a device alone cannot transfer");
    assert!(matches!(
        refusal(&err),
        OwnerGuardRefusal::ProofRequired { .. }
    ));

    // 2b. Transfer, wrapped in a proof the attacker can make: its accomplice's
    // root. Refused, because the proof is not the signing account's.
    let foreign = owner_proof_for(
        &f.store,
        &f.gid,
        &f.gid,
        &f.accomplice_sk.public_key(),
        OwnerOpKind::TransferOwnership,
        GroupOp::TransferOwnership {
            new_owner: f.accomplice,
        }
        .owner_op_digest()
        .unwrap(),
    );
    let err = apply_local_signed_group_op(
        &f.store,
        &sign(
            stolen,
            &f.gid,
            3,
            GroupOp::RootGuarded {
                op: Box::new(GroupOp::TransferOwnership {
                    new_owner: f.accomplice,
                }),
                proof: Box::new(foreign),
            },
        ),
    )
    .expect_err("a proof from another account is not the owner's");
    assert!(matches!(
        refusal(&err),
        OwnerGuardRefusal::ProofAccountMismatch { .. }
    ));
    assert_eq!(owner_of(&f.store, &f.gid), f.owner);

    // 3. The accomplice, now an admin, tries to remove the owner. The owner is
    // still the owner, so that is refused too.
    assert!(apply_local_signed_group_op(
        &f.store,
        &sign(
            &f.accomplice_sk,
            &f.gid,
            1,
            crate::test_fixtures::dummy_member_removed_op(f.owner),
        ),
    )
    .is_err());
    assert_eq!(
        owner_of(&f.store, &f.gid),
        f.owner,
        "the owner keeps the group"
    );
    assert_eq!(
        MembershipRepository::new(&f.store)
            .role_of(&f.gid, &f.owner)
            .unwrap(),
        Some(GroupMemberRole::Admin),
        "and stays a member"
    );
}

/// With the owner root's proof the transfer applies, and the counter moves.
#[test]
fn a_guarded_transfer_with_the_owners_root_proof_applies_and_spends_the_counter() {
    let f = fixture();
    assert_eq!(owner_op_counter(&f.store, &f.gid).unwrap(), 0);
    let op = guarded_group_op(
        &f.store,
        &f.gid,
        &f.owner_sk.public_key(),
        GroupOp::TransferOwnership { new_owner: f.admin },
    );
    apply_local_signed_group_op(&f.store, &sign(&f.owner_sk, &f.gid, 1, op.clone()))
        .expect("the owner's root authorises the transfer");
    assert_eq!(owner_of(&f.store, &f.gid), f.admin);
    assert_eq!(owner_op_counter(&f.store, &f.gid).unwrap(), 1);

    // The same proof again, under a fresh nonce so the nonce window does not
    // hide the check: the counter has moved on, so it is spent. The new owner
    // presents it to hand the group straight back.
    let GroupOp::RootGuarded { proof, .. } = op else {
        unreachable!()
    };
    let replay = GroupOp::RootGuarded {
        op: Box::new(GroupOp::TransferOwnership { new_owner: f.admin }),
        proof,
    };
    let err = apply_local_signed_group_op(&f.store, &sign(&f.owner_sk, &f.gid, 2, replay))
        .expect_err("a spent proof is refused");
    assert!(
        matches!(
            refusal(&err),
            OwnerGuardRefusal::StaleCounter {
                expected: 1,
                found: 0
            }
        ),
        "{err:#}"
    );
}

/// A genuine proof presented with something it was not signed for.
#[test]
fn a_proof_for_another_op_group_kind_or_account_is_refused() {
    let f = fixture();
    let owner_pk = f.owner_sk.public_key();
    let to_admin = GroupOp::TransferOwnership { new_owner: f.admin };
    let to_accomplice = GroupOp::TransferOwnership {
        new_owner: f.accomplice,
    };
    let proof = |group: &ContextGroupId, kind, op: &GroupOp| {
        owner_proof_for(
            &f.store,
            &f.gid,
            group,
            &owner_pk,
            kind,
            op.owner_op_digest().unwrap(),
        )
    };
    let wrap = |op: GroupOp, proof: SignedOwnerOp| GroupOp::RootGuarded {
        op: Box::new(op),
        proof: Box::new(proof),
    };

    let cases = [
        // Signed for "transfer to admin", presented with "transfer to accomplice".
        (
            "another op",
            wrap(
                to_accomplice.clone(),
                proof(&f.gid, OwnerOpKind::TransferOwnership, &to_admin),
            ),
        ),
        // Signed for another group.
        (
            "another group",
            wrap(
                to_admin.clone(),
                proof(
                    &ContextGroupId::from([0x99; 32]),
                    OwnerOpKind::TransferOwnership,
                    &to_admin,
                ),
            ),
        ),
        // Signed for another kind of op.
        (
            "another kind",
            wrap(
                to_admin.clone(),
                proof(&f.gid, OwnerOpKind::GroupDelete, &to_admin),
            ),
        ),
    ];
    for (nonce, (what, op)) in (1..).zip(cases) {
        let err = apply_local_signed_group_op(&f.store, &sign(&f.owner_sk, &f.gid, nonce, op))
            .expect_err(what);
        assert!(
            matches!(refusal(&err), OwnerGuardRefusal::ProofMismatch { .. }),
            "{what}: {err:#}"
        );
    }

    // Another account's genuine proof.
    let theirs = owner_proof_for(
        &f.store,
        &f.gid,
        &f.gid,
        &f.admin_sk.public_key(),
        OwnerOpKind::TransferOwnership,
        to_admin.owner_op_digest().unwrap(),
    );
    let err = apply_local_signed_group_op(
        &f.store,
        &sign(&f.owner_sk, &f.gid, 10, wrap(to_admin, theirs)),
    )
    .expect_err("another account's proof");
    assert!(matches!(
        refusal(&err),
        OwnerGuardRefusal::ProofAccountMismatch { .. }
    ));

    // A wrapper inside a wrapper.
    let inner = guarded_group_op(&f.store, &f.gid, &owner_pk, GroupOp::GroupDelete);
    let nested = wrap(
        inner,
        proof(&f.gid, OwnerOpKind::GroupDelete, &GroupOp::GroupDelete),
    );
    let err = apply_local_signed_group_op(&f.store, &sign(&f.owner_sk, &f.gid, 11, nested))
        .expect_err("nested");
    assert!(matches!(
        refusal(&err),
        OwnerGuardRefusal::NotAGuardedKind { .. }
    ));

    assert_eq!(owner_of(&f.store, &f.gid), f.owner);
    assert_eq!(owner_op_counter(&f.store, &f.gid).unwrap(), 0);
}

/// Any epoch the chain reaches may sign, but the chain must reach the epoch
/// this group recorded for the account, with the key it recorded.
#[test]
fn a_proof_whose_chain_stops_below_the_recorded_epoch_is_refused() {
    let f = fixture();
    let owner_pk = f.owner_sk.public_key();
    let root0 = root_for(&owner_pk);
    let root1 = PrivateKey::from([0x71; 32]);
    let genesis = AccountGenesis::new(root0.public_key());
    assert_eq!(genesis.account_id(), f.owner);

    // The group folds a rotation of the owner's root to epoch 1.
    let handoff = RootKeyHandoff::sign(&root0, f.owner, 0, &root1.public_key()).unwrap();
    AccountBindingRepository::new(&f.store)
        .apply_rotation(&f.gid, &handoff)
        .unwrap()
        .expect("the rotation continues the chain");
    // And re-certifies the device under the new root, so it still speaks for
    // the account: a binding signed by a superseded epoch is filtered out.
    let device = calimero_account::DeviceId::from(*AsRef::<[u8; 32]>::as_ref(&owner_pk));
    let recert = calimero_account::DeviceCert::sign(
        &root1,
        f.owner,
        device,
        &owner_pk,
        &calimero_account::KemPublicKey::from([0x09; 32]),
        1,
        1,
    )
    .unwrap();
    AccountBindingRepository::new(&f.store)
        .apply_link(&f.gid, &genesis, &[handoff], &recert, 0)
        .unwrap()
        .expect("the device is re-certified at epoch 1");

    let op = GroupOp::TransferOwnership { new_owner: f.admin };
    let signed = |signer: &PrivateKey, key_epoch, chain: Vec<RootKeyHandoff>| SignedOwnerOp {
        genesis,
        chain,
        statement: OwnerOpAuthorization::sign(
            signer,
            OwnerOpTerms {
                account: f.owner,
                namespace_id: f.gid.to_bytes(),
                group_id: f.gid.to_bytes(),
                kind: OwnerOpKind::TransferOwnership,
                op_digest: op.owner_op_digest().unwrap(),
                counter: 0,
                key_epoch,
            },
        )
        .unwrap(),
    };
    let wrap = |proof| GroupOp::RootGuarded {
        op: Box::new(op.clone()),
        proof: Box::new(proof),
    };

    // Old root, no chain: it cannot show it knows about the rotation.
    let err = apply_local_signed_group_op(
        &f.store,
        &sign(&f.owner_sk, &f.gid, 1, wrap(signed(&root0, 0, vec![]))),
    )
    .expect_err("stops below the recorded epoch");
    assert!(
        matches!(
            refusal(&err),
            OwnerGuardRefusal::BelowRecordedEpoch { recorded: 1, .. }
        ),
        "{err:#}"
    );

    // A chain that forks at epoch 0 onto a key the group never recorded.
    let forked = RootKeyHandoff::sign(
        &root0,
        f.owner,
        0,
        &PrivateKey::from([0x72; 32]).public_key(),
    )
    .unwrap();
    let err = apply_local_signed_group_op(
        &f.store,
        &sign(
            &f.owner_sk,
            &f.gid,
            2,
            wrap(signed(&root0, 0, vec![forked])),
        ),
    )
    .expect_err("a forked chain");
    assert!(
        matches!(
            refusal(&err),
            OwnerGuardRefusal::ForkedChain { epoch: 1, .. }
        ),
        "{err:#}"
    );

    // The old root with the full chain: accepted, as for a revocation.
    apply_local_signed_group_op(
        &f.store,
        &sign(
            &f.owner_sk,
            &f.gid,
            3,
            wrap(signed(&root0, 0, vec![handoff])),
        ),
    )
    .expect("any epoch the chain reaches may sign, once it reaches the recorded one");
    assert_eq!(owner_of(&f.store, &f.gid), f.admin);
}

/// The TEE policy ops stay admin-level: a non-owner admin may set one, with a
/// proof from its OWN account's root.
#[test]
fn a_tee_policy_with_the_signing_admins_own_proof_applies() {
    let f = fixture();
    let admin_pk = f.admin_sk.public_key();
    let policy = GroupOp::TeeAuthoringPolicySet {
        allowed_mrtd: vec!["aa".to_owned()],
    };

    // The owner's root cannot vouch for another admin's device.
    let owners = guarded_group_op(&f.store, &f.gid, &f.owner_sk.public_key(), policy.clone());
    let err = apply_local_signed_group_op(&f.store, &sign(&f.admin_sk, &f.gid, 1, owners))
        .expect_err("not the signer's own root");
    assert!(matches!(
        refusal(&err),
        OwnerGuardRefusal::ProofAccountMismatch { .. }
    ));

    for (nonce, op) in (2..).zip([policy, admission_policy(), release_policy()]) {
        let label = op.op_kind_label();
        let guarded = guarded_group_op(&f.store, &f.gid, &admin_pk, op);
        apply_local_signed_group_op(&f.store, &sign(&f.admin_sk, &f.gid, nonce, guarded))
            .unwrap_or_else(|err| panic!("{label}: {err:#}"));
    }
    assert_eq!(
        read_tee_authoring_policy(&f.store, &f.gid).unwrap(),
        vec!["aa".to_owned()],
        "the readers see through the wrapper"
    );
    assert!(matches!(
        crate::read_tee_admission_policy(&f.store, &f.gid).unwrap(),
        crate::TeeAdmissionPolicyRead::Set(policy) if policy.release_trust.is_some()
    ));
    assert_eq!(owner_op_counter(&f.store, &f.gid).unwrap(), 3);

    // A plain member's own proof does not make it an admin.
    let member = guarded_group_op(
        &f.store,
        &f.gid,
        &f.accomplice_sk.public_key(),
        GroupOp::TeeAuthoringPolicySet {
            allowed_mrtd: vec![],
        },
    );
    assert!(
        apply_local_signed_group_op(&f.store, &sign(&f.accomplice_sk, &f.gid, 1, member)).is_err()
    );
    assert_eq!(owner_op_counter(&f.store, &f.gid).unwrap(), 3);
}

/// The owner-only deletion, guarded: it applies with the owner's proof, and a
/// non-owner admin's own proof is still not the owner's.
#[test]
fn a_guarded_group_delete_is_owner_only() {
    let f = fixture();
    let admins = guarded_group_op(
        &f.store,
        &f.gid,
        &f.admin_sk.public_key(),
        GroupOp::GroupDelete,
    );
    let err = apply_local_signed_group_op(&f.store, &sign(&f.admin_sk, &f.gid, 1, admins))
        .expect_err("an admin's own proof does not make it the owner");
    assert!(matches!(
        err.downcast_ref::<MembershipError>(),
        Some(MembershipError::OnlyOwnerCanDelete(_))
    ));

    let owners = guarded_group_op(
        &f.store,
        &f.gid,
        &f.owner_sk.public_key(),
        GroupOp::GroupDelete,
    );
    apply_local_signed_group_op(&f.store, &sign(&f.owner_sk, &f.gid, 1, owners))
        .expect("the owner deletes with its root proof");
    assert!(MetaRepository::new(&f.store)
        .load(&f.gid)
        .unwrap()
        .is_none());
    assert_eq!(
        owner_op_counter(&f.store, &f.gid).unwrap(),
        1,
        "the counter outlives the group, so a spent proof never applies to a recreated one"
    );
}

/// `AdminChanged` is guarded and owner-only: a bare one is refused, a second
/// admin's own proof is refused, and the owner's proof applies.
#[test]
fn admin_changed_is_guarded_and_owner_only() {
    let store = test_store();
    let ns_id = [0xB7u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let ((owner_sk, owner_pk), owner) = bootstrap_namespace_with_admin_account(&store, ns_id);
    let second_sk = PrivateKey::from([0xB8; 32]);
    let second = enrol_member(&store, &ns_gid, &second_sk.public_key());
    let target = enrol_member(&store, &ns_gid, &PublicKey::from([0xB9u8; 32]));
    let membership = MembershipRepository::new(&store);
    membership
        .add_member(&ns_gid, &second, GroupMemberRole::Admin)
        .unwrap();
    membership
        .add_member(&ns_gid, &target, GroupMemberRole::Member)
        .unwrap();
    let gov = NamespaceGovernance::new(&store, ns_id.into());
    let publish = |sk: &PrivateKey, op: RootOp| {
        let head = gov.read_head_record().expect("head");
        gov.apply_signed_op(
            &SignedNamespaceOp::sign(
                sk,
                ns_id.into(),
                head.parent_hashes,
                head.next_nonce,
                seal_for_test(&store, ns_gid, op),
            )
            .unwrap(),
        )
    };
    let admin_pin = || {
        MetaRepository::new(&store)
            .load(&ns_gid)
            .unwrap()
            .unwrap()
            .admin_identity
    };

    // Bare, from the owner's own device: refused.
    let err = publish(&owner_sk, RootOp::AdminChanged { new_admin: target })
        .expect_err("bare AdminChanged");
    assert!(matches!(
        refusal(&err),
        OwnerGuardRefusal::ProofRequired { .. }
    ));

    // A non-owner admin with a valid proof from its own root: refused, because
    // repointing the admin pin is the owner's alone.
    let seconds = guarded_root_op(
        &store,
        &ns_gid,
        &second_sk.public_key(),
        RootOp::AdminChanged { new_admin: second },
    );
    let err = publish(&second_sk, seconds).expect_err("not the owner");
    assert!(
        matches!(
            err.downcast_ref::<MembershipError>(),
            Some(MembershipError::OnlyOwnerCanChangeAdmin(_))
        ),
        "{err:#}"
    );
    assert_eq!(admin_pin(), owner);
    assert_eq!(owner_op_counter(&store, &ns_gid).unwrap(), 0);

    // The owner, with its root proof.
    let owners = guarded_root_op(
        &store,
        &ns_gid,
        &owner_pk,
        RootOp::AdminChanged { new_admin: target },
    );
    publish(&owner_sk, owners).expect("the owner repoints the admin pin");
    assert_eq!(admin_pin(), target);
    assert_eq!(owner_op_counter(&store, &ns_gid).unwrap(), 1);
}

/// Neither form of an owner-level op may travel through a relay.
#[test]
fn guarded_ops_are_never_delegable() {
    let f = fixture();
    let owner_pk = f.owner_sk.public_key();
    for op in [
        GroupOp::TransferOwnership { new_owner: f.admin },
        GroupOp::GroupDelete,
        admission_policy(),
    ] {
        assert!(op.delegable_form().is_none());
        assert!(guarded_group_op(&f.store, &f.gid, &owner_pk, op)
            .delegable_form()
            .is_none());
    }
    let admin_changed = RootOp::AdminChanged { new_admin: f.admin };
    assert!(admin_changed.delegable_form().is_none());
    assert!(guarded_root_op(&f.store, &f.gid, &owner_pk, admin_changed)
        .delegable_form()
        .is_none());
}
