//! Namespace ops applied through the node's applier over the shared
//! authorization world, so the projection at the op's cut decides.

use calimero_account::{
    AccountGenesis, AccountMemberEndorsement, DeviceCert, DeviceId, KemPublicKey,
};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::{BytecodeId, ContextGroupId};
use calimero_governance_store::authz_matrix::{ActorState, World};
use calimero_governance_store::{
    AccountBindingRepository, GroupKeyring, MembershipRepository, NamespaceGovernance,
    NodeDeviceRepository,
};
use calimero_governance_types::NamespaceOp;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;

use super::*;
use crate::scope_projection::ScopeProjections;
use crate::test_support::device_scope;

const DEVICE_SEED: u8 = 0xD2; // key and id material of the one linked device

fn sign_next(
    store: &Store,
    namespace: &ContextGroupId,
    signer: &PrivateKey,
    op: NamespaceOp,
) -> SignedNamespaceOp {
    let head = NamespaceGovernance::new(store, namespace.to_bytes().into())
        .read_head_record()
        .expect("read the DAG head");
    SignedNamespaceOp::sign(
        signer,
        namespace.to_bytes().into(),
        head.parent_hashes,
        head.next_nonce,
        op,
    )
    .expect("sign a namespace op")
}

fn sealed_group_op(store: &Store, namespace: &ContextGroupId, op: &GroupOp) -> NamespaceOp {
    let (key_id, key) = GroupKeyring::new(store, *namespace)
        .load_current_key()
        .expect("read the namespace keyring")
        .expect("the world keys the namespace");
    NamespaceOp::Group {
        group_id: namespace.to_bytes().into(),
        key_id: key_id.into(),
        encrypted: GroupKeyring::encrypt_op(&key, op).expect("seal a group op"),
        key_rotation: None,
    }
}

async fn apply(store: &Store, op: &SignedNamespaceOp) -> Result<(), ApplyError> {
    let delta = signed_namespace_op_to_delta(op).expect("build the delta");
    NamespaceGovernanceApplier::new(store.clone())
        .apply(&delta)
        .await
}

/// Sign `op` at the current heads, seal it under the namespace key and apply it.
async fn publish(
    store: &Store,
    namespace: &ContextGroupId,
    signer: &PrivateKey,
    op: &GroupOp,
) -> Result<(), ApplyError> {
    let signed = sign_next(
        store,
        namespace,
        signer,
        sealed_group_op(store, namespace, op),
    );
    apply(store, &signed).await
}

/// A private copy of the world with the namespace admin's role in the op log:
/// the world seats its admins outside it, where the projection cannot see them.
async fn world_with_a_logged_admin() -> (&'static World, Store) {
    let world = World::shared();
    let store = world.fork();
    let admin = world.actor(ActorState::NamespaceAdmin);
    publish(
        &store,
        &world.namespace,
        &world.owner_sk,
        &GroupOp::MemberRoleSet {
            member: admin.account,
            role: GroupMemberRole::Admin,
        },
    )
    .await
    .expect("the owner logs the admin's role");
    (world, store)
}

/// `signer` promotes the world's direct member to admin; the member's role after.
async fn role_after_promotion_by(
    store: &Store,
    world: &World,
    signer: &PrivateKey,
) -> Option<GroupMemberRole> {
    let member = world.actor(ActorState::DirectMember);
    let _applied = publish(
        store,
        &world.namespace,
        signer,
        &GroupOp::MemberRoleSet {
            member: member.account,
            role: GroupMemberRole::Admin,
        },
    )
    .await;
    MembershipRepository::new(store)
        .role_of(&world.namespace, &member.account)
        .expect("read the role")
}

/// Who vouches for the account of a linked device.
#[derive(Clone, Copy)]
enum Endorsement {
    Member,
    Forged,
    NonMember,
}

/// A member publishes a link of a new device of the admin's account, scoped to
/// `applications`. Returns the device's key.
async fn link_a_device_of_the_admin(
    store: &Store,
    world: &World,
    applications: &[ApplicationId],
    vouched: Endorsement,
) -> PrivateKey {
    let admin = world.actor(ActorState::NamespaceAdmin);
    let member = world.actor(ActorState::DirectMember);
    let endorser = match vouched {
        Endorsement::NonMember => world.actor(ActorState::NonMember),
        Endorsement::Member | Endorsement::Forged => member,
    };
    let device_sk = PrivateKey::from([DEVICE_SEED; 32]);
    let cert = DeviceCert::sign(
        &admin.root,
        admin.account,
        DeviceId::mint(admin.account, [DEVICE_SEED; 16]),
        &device_sk.public_key(),
        &KemPublicKey::from([DEVICE_SEED ^ 0x01; 32]),
        0,
        0,
    )
    .expect("the admin's root certifies the device");
    let mut endorsement = AccountMemberEndorsement::sign(&endorser.sign_sk, admin.account)
        .expect("the endorser endorses the account");
    if matches!(vouched, Endorsement::Forged) {
        endorsement.signature[0] ^= 0xFF;
    }
    let link = GroupOp::AccountDeviceLinked {
        genesis: AccountGenesis::new(admin.root.public_key()),
        chain: vec![],
        cert,
        endorsement,
        scope: Box::new(device_scope(&admin.root, &cert, applications, 0)),
    };
    let _applied = publish(store, &world.namespace, &member.sign_sk, &link).await;
    device_sk
}

/// [`link_a_device_of_the_admin`] under an endorsement whose signature does not verify.
async fn link_under_a_forged_endorsement(store: &Store, world: &World) -> PrivateKey {
    let device_sk = link_a_device_of_the_admin(store, world, &[], Endorsement::Forged).await;
    assert!(
        AccountBindingRepository::new(store)
            .binding_for_sign_pk(&world.namespace, &device_sk.public_key())
            .expect("read the binding")
            .is_none(),
        "control: the refused link leaves the device unbound"
    );
    device_sk
}

#[actix::test]
async fn a_device_linked_with_a_forged_endorsement_does_not_speak_for_its_account() {
    let (world, store) = world_with_a_logged_admin().await;
    let device_sk = link_under_a_forged_endorsement(&store, world).await;

    let role = role_after_promotion_by(&store, world, &device_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Member),
        "a device linked under a forged endorsement promoted a member"
    );
}

#[actix::test]
async fn a_device_linked_with_a_forged_endorsement_writes_for_no_account_at_the_cut() {
    let (world, store) = world_with_a_logged_admin().await;
    let device_sk = link_under_a_forged_endorsement(&store, world).await;

    let (projection, _namespace, heads) =
        ScopeProjections::ephemeral_projection(&store, &world.namespace)
            .expect("fold the namespace");

    assert_eq!(
        projection.writer_account_at_cut(
            &store,
            world.namespace,
            &device_sk.public_key(),
            &heads,
            0
        ),
        None,
        "a device linked under a forged endorsement writes data as its account"
    );
}

#[actix::test]
async fn a_member_without_a_proof_does_not_withdraw_another_accounts_device_at_the_cut() {
    let (world, store) = world_with_a_logged_admin().await;
    let admin = world.actor(ActorState::NamespaceAdmin);
    let member = world.actor(ActorState::DirectMember);
    let unlink = GroupOp::AccountDeviceUnlinked {
        account: admin.account,
        device: admin.device,
        proof: None,
    };
    let _refused = publish(&store, &world.namespace, &member.sign_sk, &unlink).await;
    assert!(
        !AccountBindingRepository::new(&store)
            .is_revoked(&world.namespace, admin.device)
            .expect("read a tombstone"),
        "control: the apply records no revocation"
    );

    let role = role_after_promotion_by(&store, world, &admin.sign_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Admin),
        "a member without a proof withdrew the admin's device at the cut"
    );
}

/// A member publishes the admin's own root-signed scope, naming `applications`,
/// as a narrowing of the admin's device out of `application`; then that device
/// promotes the member. Returns the member's role after.
async fn role_after_a_members_descope(
    applications: &[ApplicationId],
    application: ApplicationId,
) -> Option<GroupMemberRole> {
    let (world, store) = world_with_a_logged_admin().await;
    let admin = world.actor(ActorState::NamespaceAdmin);
    let member = world.actor(ActorState::DirectMember);
    let descope = GroupOp::AccountDeviceDescoped {
        account: admin.account,
        device: admin.device,
        application: Some(application),
        scope: Box::new(device_scope(
            &admin.root,
            &admin.proof().statement,
            applications,
            0,
        )),
    };
    let _refused = publish(&store, &world.namespace, &member.sign_sk, &descope).await;
    assert_eq!(
        AccountBindingRepository::new(&store)
            .scope_floor(&world.namespace, admin.account, admin.device)
            .expect("read the scope floor"),
        None,
        "control: the apply raises no floor"
    );

    role_after_promotion_by(&store, world, &admin.sign_sk).await
}

#[actix::test]
async fn a_descope_whose_scope_still_reaches_the_group_does_not_narrow_the_device_at_the_cut() {
    let role = role_after_a_members_descope(&[], ApplicationId::from([0xEE; 32])).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Admin),
        "a scope that still reaches the group narrowed the admin's device at the cut"
    );
}

#[actix::test]
async fn a_device_linked_under_a_non_members_endorsement_does_not_speak_for_its_account() {
    let (world, store) = world_with_a_logged_admin().await;
    let device_sk = link_a_device_of_the_admin(&store, world, &[], Endorsement::NonMember).await;

    let role = role_after_promotion_by(&store, world, &device_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Member),
        "a device linked under a non-member's endorsement promoted a member"
    );
}

#[actix::test]
async fn a_descope_by_a_signer_of_another_account_does_not_narrow_the_device_at_the_cut() {
    let world = World::shared();
    let role =
        role_after_a_members_descope(&[ApplicationId::from([0xEE; 32])], world.application).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Admin),
        "a member of another account narrowed the admin's device at the cut"
    );
}

fn target(application: ApplicationId) -> GroupOp {
    GroupOp::TargetApplicationSet {
        bytecode_id: BytecodeId::from([0xB1; 32]),
        target_application_id: application,
        package: "matrix.app".to_owned(),
        version: "1.0.0".to_owned(),
    }
}

/// A private copy of the world with a logged admin and the namespace pointed at
/// the world's application in the op log.
async fn world_targeting() -> (&'static World, Store) {
    let (world, store) = world_with_a_logged_admin().await;
    publish(
        &store,
        &world.namespace,
        &world.owner_sk,
        &target(world.application),
    )
    .await
    .expect("the owner sets the namespace's target");
    (world, store)
}

#[actix::test]
async fn a_device_scoped_to_the_namespace_application_speaks_for_its_account() {
    let (world, store) = world_targeting().await;
    let device_sk =
        link_a_device_of_the_admin(&store, world, &[world.application], Endorsement::Member).await;

    let role = role_after_promotion_by(&store, world, &device_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Admin),
        "a device whose scope covers the namespace's application speaks for its account"
    );
}

#[actix::test]
async fn a_device_scoped_away_from_the_namespace_does_not_speak_for_its_account() {
    let (world, store) = world_targeting().await;
    let elsewhere = ApplicationId::from([0xEE; 32]);
    let device_sk =
        link_a_device_of_the_admin(&store, world, &[elsewhere], Endorsement::Member).await;

    let role = role_after_promotion_by(&store, world, &device_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Member),
        "a device whose scope excludes the namespace's application promoted a member"
    );
}

#[actix::test]
async fn a_device_scoped_away_from_the_namespace_writes_for_no_account_at_the_cut() {
    let (world, store) = world_targeting().await;
    let elsewhere = ApplicationId::from([0xEE; 32]);
    let device_sk =
        link_a_device_of_the_admin(&store, world, &[elsewhere], Endorsement::Member).await;

    let (projection, _namespace, heads) =
        ScopeProjections::ephemeral_projection(&store, &world.namespace)
            .expect("fold the namespace");

    assert_eq!(
        projection.writer_account_at_cut(
            &store,
            world.namespace,
            &device_sk.public_key(),
            &heads,
            0
        ),
        None,
        "a device whose scope excludes the namespace's application writes data as its account"
    );
}

#[actix::test]
async fn a_device_stops_speaking_for_its_account_once_the_target_leaves_its_scope() {
    let (world, store) = world_targeting().await;
    let device_sk =
        link_a_device_of_the_admin(&store, world, &[world.application], Endorsement::Member).await;
    publish(
        &store,
        &world.namespace,
        &world.owner_sk,
        &target(ApplicationId::from([0xEE; 32])),
    )
    .await
    .expect("the owner moves the namespace to another application");

    let role = role_after_promotion_by(&store, world, &device_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Member),
        "a device spoke for its account after the namespace's target left its scope"
    );
}

#[actix::test]
async fn this_nodes_own_account_namespace_row_exempts_no_namespace_from_scope() {
    let (world, store) = world_with_a_logged_admin().await;
    NodeDeviceRepository::new(&store)
        .store_account_namespace(&world.namespace)
        .expect("record a node-local account namespace");
    let elsewhere = ApplicationId::from([0xEE; 32]);
    let device_sk =
        link_a_device_of_the_admin(&store, world, &[elsewhere], Endorsement::Member).await;

    let role = role_after_promotion_by(&store, world, &device_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Member),
        "a node-local row exempted a namespace its genesis does not mark as an account namespace"
    );
}

/// Two admins point the namespace at different applications from the same heads.
/// Every replica must settle on one target, whichever op it applies first.
#[actix::test]
async fn concurrent_targets_settle_the_same_way_in_either_order() {
    let mut roles = Vec::new();
    for rival_first in [false, true] {
        let (world, store) = world_targeting().await;
        let device_sk =
            link_a_device_of_the_admin(&store, world, &[world.application], Endorsement::Member)
                .await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let concurrent = |signer: &PrivateKey, application: ApplicationId| {
            let sealed = sealed_group_op(&store, &world.namespace, &target(application));
            sign_next(&store, &world.namespace, signer, sealed)
        };
        let id = |op: &SignedNamespaceOp| op.content_hash().expect("hash the op");
        // Sealing draws a fresh nonce, so re-sign until the rival ranks above the
        // restated target and the device is out of scope wherever the two meet.
        let (restated, rival) = std::iter::repeat_with(|| {
            (
                concurrent(&world.owner_sk, world.application),
                concurrent(&admin.sign_sk, ApplicationId::from([0xEE; 32])),
            )
        })
        .take(64)
        .find(|(restated, rival)| id(rival) > id(restated))
        .expect("a rival that ranks above the restated target");
        let order = if rival_first {
            [&rival, &restated]
        } else {
            [&restated, &rival]
        };
        for op in order {
            apply(&store, op).await.expect("both targets apply");
        }
        roles.push(role_after_promotion_by(&store, world, &device_sk).await);
    }

    assert_eq!(
        roles[0], roles[1],
        "the two orders settled on different targets"
    );
    assert_eq!(
        roles[0],
        Some(GroupMemberRole::Member),
        "the concurrent target that ranks first left the device in scope"
    );
}

#[actix::test]
async fn another_accounts_proof_does_not_withdraw_a_device_at_the_cut() {
    let (world, store) = world_with_a_logged_admin().await;
    let admin = world.actor(ActorState::NamespaceAdmin);
    let member = world.actor(ActorState::DirectMember);
    // The member's own root signs "my account withdraws device D" for the admin's D.
    let proof = calimero_account::SignedDeviceRevocation {
        genesis: AccountGenesis::new(member.root.public_key()),
        chain: vec![],
        statement: calimero_account::DeviceRevocation::sign(
            &member.root,
            member.account,
            admin.device,
            0,
        )
        .expect("the member's root signs the statement"),
    };
    let unlink = GroupOp::AccountDeviceUnlinked {
        account: member.account,
        device: admin.device,
        proof: Some(proof),
    };
    let _applied = publish(&store, &world.namespace, &member.sign_sk, &unlink).await;
    assert!(
        !AccountBindingRepository::new(&store)
            .is_revoked(&world.namespace, admin.device)
            .expect("read a tombstone"),
        "control: the apply records no revocation of the admin's device"
    );

    let role = role_after_promotion_by(&store, world, &admin.sign_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Admin),
        "another account's proof withdrew the admin's device at the cut"
    );
}

/// A cascade from code the namespace does not run moves nothing in the apply, so it
/// must not move the target a device scope is judged against either.
#[actix::test]
async fn a_cascade_that_moves_nothing_leaves_the_target() {
    let (world, store) = world_targeting().await;
    let device_sk =
        link_a_device_of_the_admin(&store, world, &[world.application], Endorsement::Member).await;
    let cascade = GroupOp::CascadeUpgrade {
        from_bytecode_id: BytecodeId::from([0xB9; 32]),
        bytecode_id: BytecodeId::from([0xBA; 32]),
        target_application_id: ApplicationId::from([0xEE; 32]),
        to_state_version: 0,
        migration: None,
        cascade_hlc: calimero_storage::logical_clock::HybridTimestamp::default(),
        package: "matrix.app".to_owned(),
        version: "2.0.0".to_owned(),
    };
    publish(&store, &world.namespace, &world.owner_sk, &cascade)
        .await
        .expect("the owner publishes a cascade from code nobody runs");

    let role = role_after_promotion_by(&store, world, &device_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Admin),
        "a cascade the apply did not apply moved the target out of the device's scope"
    );
}

mod open_join {
    use calimero_governance_store::test_fixtures::{
        derived_group_id, group_created, seal_for_test,
    };
    use calimero_governance_types::RootOp;

    use super::*;

    /// The owner creates an Open subgroup and a device of the admin's account that no
    /// link carried into the namespace self-joins it as that account.
    async fn open_join_by_an_unlinked_device_of_the_admin(
        store: &Store,
        world: &World,
    ) -> (PrivateKey, [u8; 32]) {
        let owner = world.actor(ActorState::Owner);
        let admin = world.actor(ActorState::NamespaceAdmin);
        let namespace = world.namespace;
        let create = sign_next(
            store,
            &namespace,
            &world.owner_sk,
            seal_for_test(
                store,
                namespace,
                group_created(owner.account, namespace.to_bytes(), false, 0x91),
            ),
        );
        apply(store, &create)
            .await
            .expect("the owner makes an Open subgroup");
        let open = ContextGroupId::from(derived_group_id(
            &owner.account,
            namespace.to_bytes(),
            false,
            0x91,
        ));

        let device_sk = PrivateKey::from([DEVICE_SEED; 32]);
        let cert = DeviceCert::sign(
            &admin.root,
            admin.account,
            DeviceId::mint(admin.account, [DEVICE_SEED; 16]),
            &device_sk.public_key(),
            &KemPublicKey::from([DEVICE_SEED ^ 0x01; 32]),
            0,
            0,
        )
        .expect("the admin's root certifies the device");
        let join = sign_next(
            store,
            &namespace,
            &device_sk,
            seal_for_test(
                store,
                namespace,
                RootOp::MemberJoinedOpen {
                    member: admin.account,
                    group_id: open,
                    account: Box::new(calimero_account::AccountProof {
                        genesis: AccountGenesis::new(admin.root.public_key()),
                        chain: vec![],
                        statement: cert,
                    }),
                },
            ),
        );
        let _applied = apply(store, &join).await;
        (device_sk, join.content_hash().expect("hash the join"))
    }

    #[actix::test]
    async fn an_open_self_join_by_a_device_no_link_brought_in_is_refused() {
        let (world, store) = world_targeting().await;
        let (device_sk, join) = open_join_by_an_unlinked_device_of_the_admin(&store, world).await;

        let bound = AccountBindingRepository::new(&store)
            .binding_for_sign_pk(&world.namespace, &device_sk.public_key())
            .expect("read the binding");
        let role = role_after_promotion_by(&store, world, &device_sk).await;

        assert_eq!(
            calimero_governance_store::parked_op(&store, world.namespace.to_bytes().into(), join)
                .expect("read the verdict"),
            Some(calimero_governance_store::Parked::Refused),
            "an unlinked device's open join applied"
        );
        assert!(
            bound.is_none(),
            "an open self-join bound its device: {bound:?}"
        );
        assert_eq!(
            role,
            Some(GroupMemberRole::Member),
            "a device whose only link is an open self-join promoted a member"
        );
    }
}

mod held_ops {
    use std::collections::BTreeMap;

    use calimero_governance_store::held_ops::HeldOps;
    use calimero_governance_store::test_fixtures::{
        derived_group_id, group_created, seal_for_test,
    };
    use calimero_governance_store::unified_op_decode::OpenedNamespaceOp;

    use super::*;

    const SUBGROUP_KEY: [u8; 32] = [0x78; 32]; // the key only the inside node holds at first

    fn under_namespace_key(
        store: &Store,
        namespace: &ContextGroupId,
        group: &ContextGroupId,
        op: &GroupOp,
    ) -> NamespaceOp {
        let (key_id, key) = GroupKeyring::new(store, *namespace)
            .load_current_key()
            .expect("read the namespace keyring")
            .expect("the world keys the namespace");
        NamespaceOp::Group {
            group_id: group.to_bytes().into(),
            key_id: key_id.into(),
            encrypted: GroupKeyring::encrypt_op(&key, op).expect("seal a group op"),
            key_rotation: None,
        }
    }

    /// A Restricted subgroup whose history is sealed under a key only `inside`
    /// holds, and a member's unauthorized op of it sealed under the namespace key.
    struct SealedSubgroup {
        world: &'static World,
        inside: Store,
        outside: Store,
        subgroup: ContextGroupId,
        attack: SignedNamespaceOp,
    }

    impl SealedSubgroup {
        fn heads(store: &Store, world: &World) -> Vec<[u8; 32]> {
            NamespaceGovernance::new(store, world.namespace.to_bytes().into())
                .read_head_record()
                .expect("read the DAG head")
                .parent_hashes
        }

        fn role_in_subgroup(&self, store: &Store) -> Option<GroupMemberRole> {
            let member = self.world.actor(ActorState::DirectMember);
            MembershipRepository::new(store)
                .role_of(&self.subgroup, &member.account)
                .expect("read the role")
        }
    }

    async fn unauthorized_op_under_the_namespace_key() -> SealedSubgroup {
        let world = World::shared();
        let (inside, outside) = (world.fork(), world.fork());
        let namespace = world.namespace;
        let owner = world.actor(ActorState::Owner);
        let member = world.actor(ActorState::DirectMember);
        let subgroup = ContextGroupId::from(derived_group_id(
            &owner.account,
            namespace.to_bytes(),
            true,
            0x72,
        ));
        let create = sign_next(
            &inside,
            &namespace,
            &world.owner_sk,
            seal_for_test(
                &inside,
                namespace,
                group_created(owner.account, namespace.to_bytes(), true, 0x72),
            ),
        );
        for store in [&inside, &outside] {
            apply(store, &create)
                .await
                .expect("the owner makes a Restricted subgroup");
        }

        // The sealed history is the admin's, not the local identity's own: a node
        // never re-drives its own ops, which it held the key to write.
        let admin = world.actor(ActorState::NamespaceAdmin);
        let seat_admin = sign_next(
            &inside,
            &namespace,
            &world.owner_sk,
            under_namespace_key(
                &inside,
                &namespace,
                &namespace,
                &GroupOp::MemberRoleSet {
                    member: admin.account,
                    role: GroupMemberRole::Admin,
                },
            ),
        );
        for store in [&inside, &outside] {
            apply(store, &seat_admin)
                .await
                .expect("the owner logs the admin's role");
        }

        let subgroup_key_id = GroupKeyring::new(&inside, subgroup)
            .store_key(&SUBGROUP_KEY)
            .expect("only the inside node holds the subgroup's key");
        let sealed = sign_next(
            &inside,
            &namespace,
            &admin.sign_sk,
            NamespaceOp::Group {
                group_id: subgroup.to_bytes().into(),
                key_id: subgroup_key_id.into(),
                encrypted: GroupKeyring::encrypt_op(
                    &SUBGROUP_KEY,
                    &GroupOp::GroupMetadataSet {
                        name: Some("sealed".to_owned()),
                        data: BTreeMap::new(),
                    },
                )
                .expect("seal under the subgroup's key"),
                key_rotation: None,
            },
        );
        for store in [&inside, &outside] {
            apply(store, &sealed)
                .await
                .expect("the sealed op applies or is logged unreadable");
        }

        let attack = sign_next(
            &inside,
            &namespace,
            &member.sign_sk,
            under_namespace_key(
                &inside,
                &namespace,
                &subgroup,
                &GroupOp::MemberRoleSet {
                    member: member.account,
                    role: GroupMemberRole::Admin,
                },
            ),
        );
        SealedSubgroup {
            world,
            inside,
            outside,
            subgroup,
            attack,
        }
    }

    #[actix::test]
    async fn an_unauthorized_subgroup_op_gets_the_same_verdict_inside_and_outside_the_subgroup() {
        let f = unauthorized_op_under_the_namespace_key().await;

        let outside_verdict = apply(&f.outside, &f.attack).await;
        let inside_verdict = apply(&f.inside, &f.attack).await;

        assert!(
            inside_verdict.is_ok() && outside_verdict.is_ok(),
            "inside: {inside_verdict:?}, outside: {outside_verdict:?}"
        );
        assert_eq!(
            SealedSubgroup::heads(&f.inside, f.world),
            SealedSubgroup::heads(&f.outside, f.world),
            "the two nodes moved past the op differently"
        );
        assert_eq!(f.role_in_subgroup(&f.inside), None, "inside applied it");
        assert_eq!(f.role_in_subgroup(&f.outside), None, "outside applied it");
    }

    #[actix::test]
    async fn an_op_published_after_a_refused_subgroup_op_applies_on_every_node() {
        let f = unauthorized_op_under_the_namespace_key().await;
        apply(&f.outside, &f.attack)
            .await
            .expect("outside logs the op it cannot judge");

        let after = sign_next(
            &f.outside,
            &f.world.namespace,
            &f.world.owner_sk,
            under_namespace_key(
                &f.outside,
                &f.world.namespace,
                &f.world.namespace,
                &GroupOp::GroupMetadataSet {
                    name: Some("after".to_owned()),
                    data: BTreeMap::new(),
                },
            ),
        );
        apply(&f.outside, &after)
            .await
            .expect("outside applies its own follow-up");

        apply(&f.inside, &f.attack)
            .await
            .expect("inside logs the op it refuses");
        apply(&f.inside, &after)
            .await
            .expect("an op citing the refused one applies inside");
        assert_eq!(
            SealedSubgroup::heads(&f.inside, f.world),
            SealedSubgroup::heads(&f.outside, f.world)
        );
    }

    #[actix::test]
    async fn a_held_subgroup_op_is_not_folded_into_the_projection() {
        let f = unauthorized_op_under_the_namespace_key().await;

        let applied = apply(&f.outside, &f.attack).await;
        let held = HeldOps::new(&f.outside, f.world.namespace.to_bytes().into())
            .read()
            .expect("read the held ops")
            .ops
            .iter()
            .any(|held| Some(held.delta_id) == f.attack.content_hash().ok());
        assert!(held, "setup: the outside node holds the op ({applied:?})");

        assert!(
            OpenedNamespaceOp::open(&f.outside, &f.attack)
                .group_op
                .is_none(),
            "the projection folds the payload of an op the node holds unapplied"
        );
    }

    #[actix::test]
    async fn a_node_that_gets_the_key_later_reaches_the_inside_verdict() {
        let f = unauthorized_op_under_the_namespace_key().await;
        apply(&f.inside, &f.attack).await.expect("inside logs it");
        apply(&f.outside, &f.attack)
            .await
            .expect("outside holds it");

        let _key_id = crate::group_key_pull::adopt_pulled_group_key(
            &f.outside,
            f.world.namespace.to_bytes().into(),
            f.subgroup,
            &SUBGROUP_KEY,
        )
        .expect("the key reaches the outside node");

        assert_eq!(
            f.role_in_subgroup(&f.outside),
            None,
            "the replay applied it"
        );
        assert!(
            OpenedNamespaceOp::open(&f.inside, &f.attack).refused,
            "inside does not fold the op as refused"
        );
        assert!(
            OpenedNamespaceOp::open(&f.outside, &f.attack).refused,
            "the replay did not reach the inside node's verdict"
        );
    }

    #[actix::test]
    async fn a_refused_op_stays_refused_through_a_replay_without_a_judge() {
        let f = unauthorized_op_under_the_namespace_key().await;
        apply(&f.inside, &f.attack).await.expect("inside logs it");
        let member = f.world.actor(ActorState::DirectMember);
        MembershipRepository::new(&f.inside)
            .add_member(&f.subgroup, &member.account, GroupMemberRole::Admin)
            .expect("live rows that would now admit the op");

        let _replayed = calimero_governance_store::retry_encrypted_ops_for_group(
            &f.inside,
            f.world.namespace.to_bytes().into(),
            f.subgroup.to_bytes(),
        );

        assert!(
            OpenedNamespaceOp::open(&f.inside, &f.attack).refused,
            "a replay on live gates applied an op refused at its cut"
        );
    }

    #[actix::test]
    async fn a_refused_op_of_the_namespace_root_is_logged_for_members_without_its_key() {
        let f = unauthorized_op_under_the_namespace_key().await;
        let member = f.world.actor(ActorState::DirectMember);
        let promote = sign_next(
            &f.inside,
            &f.world.namespace,
            &member.sign_sk,
            under_namespace_key(
                &f.inside,
                &f.world.namespace,
                &f.world.namespace,
                &GroupOp::MemberRoleSet {
                    member: member.account,
                    role: GroupMemberRole::Admin,
                },
            ),
        );

        let applied = apply(&f.inside, &promote).await;

        assert!(applied.is_ok(), "the refusal dropped the op: {applied:?}");
        assert!(
            SealedSubgroup::heads(&f.inside, f.world).contains(&promote.content_hash().unwrap()),
            "the head did not move past the refused op"
        );
        assert!(OpenedNamespaceOp::open(&f.inside, &promote).refused);
        assert_eq!(
            MembershipRepository::new(&f.inside)
                .role_of(&f.world.namespace, &member.account)
                .expect("read the role"),
            Some(GroupMemberRole::Member)
        );
    }

    #[actix::test]
    async fn this_nodes_own_refused_op_still_fails() {
        let f = unauthorized_op_under_the_namespace_key().await;
        // The owner is this node's identity; the target is no member of the subgroup.
        let own = sign_next(
            &f.inside,
            &f.world.namespace,
            &f.world.owner_sk,
            under_namespace_key(
                &f.inside,
                &f.world.namespace,
                &f.subgroup,
                &GroupOp::MemberRoleSet {
                    member: f.world.actor(ActorState::DirectMember).account,
                    role: GroupMemberRole::Admin,
                },
            ),
        );
        let applied = apply(&f.inside, &own).await;
        assert!(
            applied.is_err(),
            "this node's own refused op was logged: {applied:?}"
        );
    }

    #[actix::test]
    async fn an_op_reusing_a_spent_nonce_is_refused_and_folds_as_nothing() {
        let (world, store) = world_targeting().await;
        let member = world.actor(ActorState::DirectMember);
        let ns = world.namespace;
        let named = sign_next(
            &store,
            &ns,
            &member.sign_sk,
            under_namespace_key(
                &store,
                &ns,
                &ns,
                &GroupOp::MemberMetadataSet {
                    member: member.account,
                    name: Some("me".to_owned()),
                    data: BTreeMap::new(),
                },
            ),
        );
        apply(&store, &named)
            .await
            .expect("the member names itself");
        let head = NamespaceGovernance::new(&store, ns.to_bytes().into())
            .read_head_record()
            .expect("read the DAG head");
        let reuse = SignedNamespaceOp::sign(
            &member.sign_sk,
            ns.to_bytes().into(),
            head.parent_hashes,
            named.nonce,
            under_namespace_key(
                &store,
                &ns,
                &ns,
                &GroupOp::MemberRoleSet {
                    member: member.account,
                    role: GroupMemberRole::Admin,
                },
            ),
        )
        .expect("sign the reuse");
        apply(&store, &reuse).await.expect("the reuse is logged");
        assert!(OpenedNamespaceOp::open(&store, &reuse).refused);

        let admin = world.actor(ActorState::NamespaceAdmin);
        let demote = sign_next(
            &store,
            &ns,
            &member.sign_sk,
            under_namespace_key(
                &store,
                &ns,
                &ns,
                &GroupOp::MemberRoleSet {
                    member: admin.account,
                    role: GroupMemberRole::ReadOnly,
                },
            ),
        );
        let _refused = apply(&store, &demote).await;
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&ns, &admin.account)
                .expect("read the role"),
            Some(GroupMemberRole::Admin),
            "a member acted as admin after reusing a spent nonce"
        );
    }

    /// Two ops one signer minted at the same head share its nonce, as two sessions of
    /// one signer do; neither is in the other's past, so both apply in either order.
    #[actix::test]
    async fn concurrent_ops_sharing_a_nonce_both_apply_in_either_order() {
        let (world, first) = world_targeting().await;
        let second = World::fork_store(&first);
        let ns = world.namespace;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let member = world.actor(ActorState::DirectMember);
        let head = NamespaceGovernance::new(&first, ns.to_bytes().into())
            .read_head_record()
            .expect("read the DAG head");
        let at_head = |op: &GroupOp| {
            SignedNamespaceOp::sign(
                &admin.sign_sk,
                ns.to_bytes().into(),
                head.parent_hashes.clone(),
                head.next_nonce,
                under_namespace_key(&first, &ns, &ns, op),
            )
            .expect("sign at the head")
        };
        let demote = at_head(&GroupOp::MemberRoleSet {
            member: member.account,
            role: GroupMemberRole::ReadOnly,
        });
        let named = at_head(&GroupOp::MemberMetadataSet {
            member: admin.account,
            name: Some("admin".to_owned()),
            data: BTreeMap::new(),
        });

        for (store, order) in [(&first, [&demote, &named]), (&second, [&named, &demote])] {
            for op in order {
                apply(store, op).await.expect("the op is logged");
                assert!(
                    !OpenedNamespaceOp::open(store, op).refused,
                    "a concurrent op was refused"
                );
            }
            assert_eq!(
                MembershipRepository::new(store)
                    .role_of(&ns, &member.account)
                    .expect("read the role"),
                Some(GroupMemberRole::ReadOnly),
                "the demotion did not apply"
            );
            assert!(
                calimero_governance_store::MetadataRepository::new(store)
                    .member_metadata(&ns, &admin.account)
                    .expect("read the metadata")
                    .is_some(),
                "the naming did not apply"
            );
        }
    }
}
