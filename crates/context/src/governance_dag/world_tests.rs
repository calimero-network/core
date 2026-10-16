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
}

/// Concurrent ops judged at their own cuts: each node applies them in its own order and
/// still holds the same verdicts and rows.
mod concurrent {
    use calimero_account::AccountId;
    use calimero_governance_store::test_fixtures::{
        account_for, admission_quote_with, dummy_member_removed_op, fork_store, guarded_group_op,
        guarded_root_op, real_join_account, seal_for_test, signed_invitation_for,
    };
    use calimero_governance_store::{
        parked_op, CapabilitiesRepository, MetadataRepository, NamespaceOpLogService, Parked,
    };
    use calimero_governance_types::RootOp;
    use sha2::Digest;

    use super::*;

    /// `first` and `second` applied in each order on its own copy of `store`.
    async fn in_both_orders(
        store: &Store,
        first: &SignedNamespaceOp,
        second: &SignedNamespaceOp,
    ) -> [Store; 2] {
        let nodes = [fork_store(store), fork_store(store)];
        for (node, order) in nodes.iter().zip([[first, second], [second, first]]) {
            for op in order {
                let _applied = apply(node, op).await;
            }
        }
        nodes
    }

    /// The verdict `node` holds on each of `ops`: `None` applied, `Some` refused or held.
    fn verdicts(node: &Store, world: &World, ops: &[&SignedNamespaceOp]) -> Vec<Option<Parked>> {
        ops.iter()
            .map(|op| {
                let id = op.content_hash().expect("hash the op");
                parked_op(node, world.namespace.to_bytes().into(), id).expect("read the verdict")
            })
            .collect()
    }

    /// `op` sealed under the namespace key and signed by `signer` at `parents`.
    fn signed_at(
        store: &Store,
        world: &World,
        signer: &PrivateKey,
        parents: &[[u8; 32]],
        nonce: u64,
        op: &GroupOp,
    ) -> SignedNamespaceOp {
        SignedNamespaceOp::sign(
            signer,
            world.namespace.to_bytes().into(),
            parents.to_vec(),
            nonce,
            sealed_group_op(store, &world.namespace, op),
        )
        .expect("sign a namespace op")
    }

    /// A member the owner added through the log.
    async fn logged_member(store: &Store, world: &World, seed: u8) -> AccountId {
        let member = account_for(&PrivateKey::from([seed; 32]).public_key());
        publish(
            store,
            &world.namespace,
            &world.owner_sk,
            &GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        )
        .await
        .expect("the owner adds a member");
        member
    }

    #[actix::test]
    async fn a_role_change_concurrent_with_its_targets_removal_lands_the_same_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let member = logged_member(&store, world, 0xE4).await;
        let head = NamespaceGovernance::new(&store, world.namespace.to_bytes().into())
            .read_head_record()
            .expect("read the DAG head");
        let demote = signed_at(
            &store,
            world,
            &admin.sign_sk,
            &head.parent_hashes,
            unused_nonce(),
            &GroupOp::MemberRoleSet {
                member,
                role: GroupMemberRole::ReadOnly,
            },
        );
        let remove = signed_at(
            &store,
            world,
            &admin.sign_sk,
            &head.parent_hashes,
            unused_nonce(),
            &dummy_member_removed_op(member),
        );

        let [a, b] = in_both_orders(&store, &demote, &remove).await;

        let ops = [&demote, &remove];
        for node in [&a, &b] {
            assert_eq!(
                verdicts(node, world, &ops),
                vec![None, None],
                "the role change was judged after the removal its cut does not hold"
            );
        }
        for node in [&a, &b] {
            assert_eq!(
                MembershipRepository::new(node)
                    .role_of(&world.namespace, &member)
                    .expect("read the role"),
                None,
                "the removed member kept a row"
            );
        }
    }

    /// `member_sk`'s join of the namespace with the owner's invitation `nonce`, signed at `parents`.
    fn join_at(
        world: &World,
        member_sk: &PrivateKey,
        nonce: [u8; 32],
        parents: &[[u8; 32]],
        op_nonce: u64,
    ) -> SignedNamespaceOp {
        let member = account_for(&member_sk.public_key());
        let invitation = signed_invitation_for(&world.owner_sk, world.namespace, nonce);
        let endorsement = calimero_governance_types::AdmitterEndorsement::sign(
            &world.owner_sk,
            &world.namespace.to_bytes(),
            &member,
            &nonce,
        )
        .expect("the owner endorses the join");
        let mut join = SignedNamespaceOp::sign(
            member_sk,
            world.namespace.to_bytes().into(),
            parents.to_vec(),
            op_nonce,
            NamespaceOp::Root(RootOp::MemberJoinedAt {
                member,
                signed_invitation: invitation,
                joined_at: 1,
                account: real_join_account(&member_sk.public_key()),
            }),
        )
        .expect("sign the join");
        join.admitter_endorsement = Some(Box::new(endorsement));
        join
    }

    /// A member that joined with the owner's invitation `nonce`, and the heads after it.
    async fn joined_member(store: &Store, world: &World, seed: u8, nonce: [u8; 32]) -> PrivateKey {
        let member_sk = PrivateKey::from([seed; 32]);
        let join = join_at(world, &member_sk, nonce, &heads(store, world), 1);
        apply(store, &join).await.expect("the member joins");
        member_sk
    }

    fn heads(store: &Store, world: &World) -> Vec<[u8; 32]> {
        NamespaceGovernance::new(store, world.namespace.to_bytes().into())
            .read_head_record()
            .expect("read the DAG head")
            .parent_hashes
    }

    /// Both `ops` reached every node's log with the same verdicts, and `member` holds the
    /// same row everywhere.
    fn assert_landed_the_same(
        nodes: &[Store; 2],
        world: &World,
        ops: &[&SignedNamespaceOp],
        member: &AccountId,
    ) {
        let rows: Vec<_> = nodes
            .iter()
            .map(|node| {
                for op in ops {
                    let id = op.content_hash().expect("hash the op");
                    assert!(
                        NamespaceOpLogService::new(node, world.namespace.to_bytes().into())
                            .contains_op(id)
                            .expect("read the op log"),
                        "a node dropped an op, which stalls its namespace DAG"
                    );
                }
                (
                    verdicts(node, world, ops),
                    MembershipRepository::new(node)
                        .role_of(&world.namespace, member)
                        .expect("read the role"),
                )
            })
            .collect();
        assert_eq!(rows[0], rows[1], "the two orders landed differently");
    }

    #[actix::test]
    async fn a_rejoin_concurrent_with_the_joiners_removal_lands_the_same_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let member_sk = joined_member(&store, world, 0xE6, [0xE6; 32]).await;
        let member = account_for(&member_sk.public_key());
        let parents = heads(&store, world);
        let rejoin = join_at(world, &member_sk, [0xE7; 32], &parents, 2);
        let remove = signed_at(
            &store,
            world,
            &admin.sign_sk,
            &parents,
            unused_nonce(),
            &dummy_member_removed_op(member),
        );

        let nodes = in_both_orders(&store, &rejoin, &remove).await;

        assert_landed_the_same(&nodes, world, &[&rejoin, &remove], &member);
    }

    #[actix::test]
    async fn a_rejoin_with_a_spent_invitation_concurrent_with_a_leave_lands_the_same_in_either_order(
    ) {
        let (world, store) = world_with_a_logged_admin().await;
        let member_sk = joined_member(&store, world, 0xE8, [0xE8; 32]).await;
        let member = account_for(&member_sk.public_key());
        let parents = heads(&store, world);
        let rejoin = join_at(world, &member_sk, [0xE8; 32], &parents, 2);
        let leave = signed_at(
            &store,
            world,
            &member_sk,
            &parents,
            3,
            &GroupOp::MemberLeft {
                member,
                expected_group_state_hash: [0; 32],
                expected_context_state_hashes: Vec::new(),
            },
        );

        let nodes = in_both_orders(&store, &rejoin, &leave).await;

        assert_landed_the_same(&nodes, world, &[&rejoin, &leave], &member);
    }

    /// A join refused at its cut is refused on every node: neither logs it.
    fn assert_refused_everywhere(nodes: &[Store; 2], world: &World, join: &SignedNamespaceOp) {
        let id = join.content_hash().expect("hash the join");
        for node in nodes {
            assert!(
                !NamespaceOpLogService::new(node, world.namespace.to_bytes().into())
                    .contains_op(id)
                    .expect("read the op log"),
                "a node applied a join its cut refuses"
            );
        }
    }

    #[actix::test]
    async fn a_removed_members_rejoin_concurrent_with_its_readmission_is_refused_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let member_sk = joined_member(&store, world, 0xE9, [0xE9; 32]).await;
        let member = account_for(&member_sk.public_key());
        publish(
            &store,
            &world.namespace,
            &admin.sign_sk,
            &dummy_member_removed_op(member),
        )
        .await
        .expect("the admin removes the member");
        let parents = heads(&store, world);
        let readd = signed_at(
            &store,
            world,
            &world.owner_sk,
            &parents,
            unused_nonce(),
            &GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        );
        let rejoin = join_at(world, &member_sk, [0xEA; 32], &parents, 2);

        let nodes = in_both_orders(&store, &readd, &rejoin).await;

        assert_refused_everywhere(&nodes, world, &rejoin);
    }

    #[actix::test]
    async fn two_rejoins_with_one_invitation_land_the_same_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let member_sk = joined_member(&store, world, 0xEB, [0xEB; 32]).await;
        let member = account_for(&member_sk.public_key());
        let leave = signed_at(
            &store,
            world,
            &member_sk,
            &heads(&store, world),
            2,
            &GroupOp::MemberLeft {
                member,
                expected_group_state_hash: [0; 32],
                expected_context_state_hashes: Vec::new(),
            },
        );
        apply(&store, &leave).await.expect("the member leaves");
        let parents = heads(&store, world);
        // The same fresh invitation, presented twice from one cut, e.g. from two devices.
        let first = join_at(world, &member_sk, [0xEC; 32], &parents, 3);
        let second = join_at(world, &member_sk, [0xEC; 32], &parents, 4);

        let nodes = in_both_orders(&store, &first, &second).await;

        assert_landed_the_same(&nodes, world, &[&first, &second], &member);
    }

    #[actix::test]
    async fn a_rejoin_with_the_invitation_a_member_left_with_is_refused_at_its_cut() {
        let (world, store) = world_with_a_logged_admin().await;
        let member_sk = joined_member(&store, world, 0xED, [0xED; 32]).await;
        let member = account_for(&member_sk.public_key());
        let leave = signed_at(
            &store,
            world,
            &member_sk,
            &heads(&store, world),
            2,
            &GroupOp::MemberLeft {
                member,
                expected_group_state_hash: [0; 32],
                expected_context_state_hashes: Vec::new(),
            },
        );
        apply(&store, &leave).await.expect("the member leaves");
        let rejoin = join_at(world, &member_sk, [0xED; 32], &heads(&store, world), 3);

        let applied = apply(&store, &rejoin).await;

        assert!(applied.is_err(), "a spent invitation readmitted its member");
    }

    /// `op` of `group`, sealed under the key that covers it.
    fn signed_in(
        store: &Store,
        world: &World,
        group: &ContextGroupId,
        signer: &PrivateKey,
        parents: &[[u8; 32]],
        nonce: u64,
        op: &GroupOp,
    ) -> SignedNamespaceOp {
        // The Restricted subgroup has a key of its own; the rest are under the namespace's.
        let keyring = if *group == world.restricted {
            *group
        } else {
            world.namespace
        };
        let (key_id, key) = GroupKeyring::new(store, keyring)
            .load_current_key()
            .expect("read the keyring")
            .expect("the world keys the group");
        SignedNamespaceOp::sign(
            signer,
            world.namespace.to_bytes().into(),
            parents.to_vec(),
            nonce,
            NamespaceOp::Group {
                group_id: group.to_bytes().into(),
                key_id: key_id.into(),
                encrypted: GroupKeyring::encrypt_op(&key, op).expect("seal a group op"),
                key_rotation: None,
            },
        )
        .expect("sign a namespace op")
    }

    #[actix::test]
    async fn an_open_join_concurrent_with_its_removal_lands_the_same_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let member_sk = joined_member(&store, world, 0xF0, [0xF0; 32]).await;
        let member = account_for(&member_sk.public_key());
        let parents = heads(&store, world);
        let open_join = SignedNamespaceOp::sign(
            &member_sk,
            world.namespace.to_bytes().into(),
            parents.clone(),
            2,
            seal_for_test(
                &store,
                world.namespace,
                RootOp::MemberJoinedOpen {
                    member,
                    group_id: world.open_chain,
                    account: real_join_account(&member_sk.public_key()),
                },
            ),
        )
        .expect("sign the open join");
        let remove = signed_in(
            &store,
            world,
            &world.open_chain,
            &admin.sign_sk,
            &parents,
            unused_nonce(),
            &dummy_member_removed_op(member),
        );

        let nodes = in_both_orders(&store, &open_join, &remove).await;

        let ops = [&open_join, &remove];
        assert_eq!(
            verdicts(&nodes[0], world, &ops),
            verdicts(&nodes[1], world, &ops),
            "the open join was judged after the removal its cut does not hold"
        );
        assert_eq!(verdicts(&nodes[0], world, &ops), vec![None, None]);
    }

    /// The owner's admission policy that accepts mock attestations.
    async fn admit_mock_tees(store: &Store, world: &World) {
        let policy = guarded_group_op(
            store,
            &world.namespace,
            &world.owner_sk.public_key(),
            GroupOp::TeeAdmissionPolicySet {
                allowed_mrtd: vec!["m1".to_owned()],
                allowed_rtmr0: vec![],
                allowed_rtmr1: vec!["r1".to_owned()],
                allowed_rtmr2: vec!["r2".to_owned()],
                allowed_rtmr3: vec!["r3".to_owned()],
                allowed_tcb_statuses: vec!["ok".to_owned()],
                accept_mock: true,
            },
        );
        publish(store, &world.namespace, &world.owner_sk, &policy)
            .await
            .expect("the owner sets the admission policy");
    }

    /// The owner's group-form admission of `member` with the quote hashed `quote_hash`.
    fn group_admission(member: AccountId, quote_hash: [u8; 32]) -> GroupOp {
        GroupOp::MemberJoinedViaTeeAttestation {
            member,
            quote_hash,
            mrtd: "m1".to_owned(),
            rtmr0: String::new(),
            rtmr1: "r1".to_owned(),
            rtmr2: "r2".to_owned(),
            rtmr3: "r3".to_owned(),
            tcb_status: "ok".to_owned(),
            role: GroupMemberRole::ReadOnlyTee,
        }
    }

    /// The owner's root-form admission of `tee_sk`'s node, sealed and signed at `parents`.
    fn root_admission(
        store: &Store,
        world: &World,
        tee_sk: &PrivateKey,
        parents: &[[u8; 32]],
        nonce: u64,
    ) -> SignedNamespaceOp {
        let account = real_join_account(&tee_sk.public_key());
        let quote = admission_quote_with(&world.namespace, &tee_sk.public_key(), &account);
        let quote_hash: [u8; 32] = sha2::Sha256::digest(&quote).into();
        SignedNamespaceOp::sign(
            &world.owner_sk,
            world.namespace.to_bytes().into(),
            parents.to_vec(),
            nonce,
            seal_for_test(
                store,
                world.namespace,
                RootOp::MemberJoinedViaTeeAttestation {
                    group_id: world.namespace,
                    member: tee_sk.public_key(),
                    quote_hash,
                    mrtd: "m1".to_owned(),
                    rtmr0: String::new(),
                    rtmr1: "r1".to_owned(),
                    rtmr2: "r2".to_owned(),
                    rtmr3: "r3".to_owned(),
                    tcb_status: "ok".to_owned(),
                    role: GroupMemberRole::ReadOnlyTee,
                    account,
                    quote,
                },
            ),
        )
        .expect("sign the admission")
    }

    #[actix::test]
    async fn a_tee_readmission_concurrent_with_its_removal_lands_the_same_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        admit_mock_tees(&store, world).await;
        let tee = account_for(&PrivateKey::from([0xF1; 32]).public_key());
        publish(
            &store,
            &world.namespace,
            &world.owner_sk,
            &group_admission(tee, [0xF1; 32]),
        )
        .await
        .expect("the owner admits the TEE");
        let parents = heads(&store, world);
        let nonce = unused_nonce();
        let readmit = signed_in(
            &store,
            world,
            &world.namespace,
            &world.owner_sk,
            &parents,
            nonce,
            &group_admission(tee, [0xF2; 32]),
        );
        let remove = signed_in(
            &store,
            world,
            &world.namespace,
            &admin.sign_sk,
            &parents,
            nonce,
            &dummy_member_removed_op(tee),
        );

        let nodes = in_both_orders(&store, &readmit, &remove).await;

        assert_landed_the_same(&nodes, world, &[&readmit, &remove], &tee);
        assert_eq!(
            verdicts(&nodes[0], world, &[&readmit, &remove]),
            vec![None, None]
        );
    }

    #[actix::test]
    async fn a_root_tee_readmission_concurrent_with_its_removal_lands_the_same_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        admit_mock_tees(&store, world).await;
        let tee_sk = PrivateKey::from([0xF3; 32]);
        let tee = account_for(&tee_sk.public_key());
        let first = root_admission(
            &store,
            world,
            &tee_sk,
            &heads(&store, world),
            unused_nonce(),
        );
        apply(&store, &first)
            .await
            .expect("the owner admits the TEE");
        let parents = heads(&store, world);
        let nonce = unused_nonce();
        let readmit = root_admission(&store, world, &tee_sk, &parents, nonce);
        let remove = signed_in(
            &store,
            world,
            &world.namespace,
            &admin.sign_sk,
            &parents,
            nonce,
            &dummy_member_removed_op(tee),
        );

        let nodes = in_both_orders(&store, &readmit, &remove).await;

        assert_landed_the_same(&nodes, world, &[&readmit, &remove], &tee);
        assert_eq!(
            verdicts(&nodes[0], world, &[&readmit, &remove]),
            vec![None, None]
        );
    }

    /// A TEE the owner admitted through the log, and the cut after it.
    async fn admitted_tee(store: &Store, world: &World, seed: u8) -> AccountId {
        admit_mock_tees(store, world).await;
        let tee = account_for(&PrivateKey::from([seed; 32]).public_key());
        let admission = signed_in(
            store,
            world,
            &world.namespace,
            &world.owner_sk,
            &heads(store, world),
            unused_nonce(),
            &group_admission(tee, [seed; 32]),
        );
        apply(store, &admission)
            .await
            .expect("the owner admits the TEE");
        assert_eq!(
            MembershipRepository::new(store)
                .role_of(&world.namespace, &tee)
                .expect("read the role"),
            Some(GroupMemberRole::ReadOnlyTee),
            "precondition: the TEE is admitted"
        );
        tee
    }

    #[actix::test]
    async fn an_add_over_a_tee_concurrent_with_its_removal_lands_the_same_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let tee = admitted_tee(&store, world, 0xF4).await;
        let parents = heads(&store, world);
        let nonce = unused_nonce();
        let add = signed_in(
            &store,
            world,
            &world.namespace,
            &admin.sign_sk,
            &parents,
            nonce,
            &GroupOp::MemberAdded {
                member: tee,
                role: GroupMemberRole::Member,
            },
        );
        let remove = signed_in(
            &store,
            world,
            &world.namespace,
            &world.owner_sk,
            &parents,
            nonce,
            &dummy_member_removed_op(tee),
        );

        let nodes = in_both_orders(&store, &add, &remove).await;

        assert_landed_the_same(&nodes, world, &[&add, &remove], &tee);
        assert_eq!(
            verdicts(&nodes[0], world, &[&add, &remove]),
            vec![Some(Parked::Refused), None],
            "an add moved a TEE row the cut holds out of the TEE roles"
        );
    }

    /// `row` reads the same on both nodes.
    fn assert_same_rows<T: PartialEq + std::fmt::Debug>(
        nodes: &[Store; 2],
        row: impl Fn(&Store) -> T,
    ) {
        assert_eq!(
            row(&nodes[0]),
            row(&nodes[1]),
            "the two orders left different rows"
        );
    }

    /// `op` on a member the owner logged, signed by `signer`, concurrent with its removal by
    /// the owner: both land, the same on both nodes.
    async fn concurrent_with_the_targets_removal(
        seed: u8,
        signer: impl Fn(&World, &PrivateKey) -> PrivateKey,
        op: impl Fn(AccountId) -> GroupOp,
    ) -> ([Store; 2], &'static World, AccountId) {
        let (world, store) = world_with_a_logged_admin().await;
        let member_sk = joined_member(&store, world, seed, [seed; 32]).await;
        let member = account_for(&member_sk.public_key());
        let parents = heads(&store, world);
        let changed = signed_in(
            &store,
            world,
            &world.namespace,
            &signer(world, &member_sk),
            &parents,
            unused_nonce(),
            &op(member),
        );
        let remove = signed_in(
            &store,
            world,
            &world.namespace,
            &world.owner_sk,
            &parents,
            unused_nonce(),
            &dummy_member_removed_op(member),
        );

        let nodes = in_both_orders(&store, &changed, &remove).await;

        assert_landed_the_same(&nodes, world, &[&changed, &remove], &member);
        assert_eq!(
            verdicts(&nodes[0], world, &[&changed, &remove]),
            vec![None, None],
            "the op was judged after the removal its cut does not hold"
        );
        (nodes, world, member)
    }

    fn by_the_admin(world: &World, _: &PrivateKey) -> PrivateKey {
        PrivateKey::from(*world.actor(ActorState::NamespaceAdmin).sign_sk.as_bytes())
    }

    #[actix::test]
    async fn a_capability_set_concurrent_with_its_targets_removal_lands_the_same_in_either_order() {
        let (nodes, world, member) =
            concurrent_with_the_targets_removal(0xF5, by_the_admin, |member| {
                GroupOp::MemberCapabilitySet {
                    member,
                    capabilities: calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT,
                }
            })
            .await;
        assert_same_rows(&nodes, |node| {
            CapabilitiesRepository::new(node)
                .member_capability(&world.namespace, &member)
                .expect("read the capability")
        });
    }

    #[actix::test]
    async fn an_auto_follow_set_concurrent_with_its_targets_removal_lands_the_same_in_either_order()
    {
        let _ = concurrent_with_the_targets_removal(0xF6, by_the_admin, |target| {
            GroupOp::MemberSetAutoFollow {
                target,
                auto_follow_contexts: true,
                auto_follow_subgroups: true,
            }
        })
        .await;
    }

    #[actix::test]
    async fn own_metadata_set_concurrent_with_the_members_rejoin_lands_the_same_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let member_sk = joined_member(&store, world, 0xF7, [0xF7; 32]).await;
        let member = account_for(&member_sk.public_key());
        let leave = signed_in(
            &store,
            world,
            &world.namespace,
            &member_sk,
            &heads(&store, world),
            2,
            &GroupOp::MemberLeft {
                member,
                expected_group_state_hash: [0; 32],
                expected_context_state_hashes: Vec::new(),
            },
        );
        apply(&store, &leave).await.expect("the member leaves");
        let parents = heads(&store, world);
        let rejoin = join_at(world, &member_sk, [0xF8; 32], &parents, 3);
        let metadata = signed_in(
            &store,
            world,
            &world.namespace,
            &member_sk,
            &parents,
            4,
            &GroupOp::MemberMetadataSet {
                member,
                name: Some("me".to_owned()),
                data: std::collections::BTreeMap::new(),
            },
        );

        let nodes = in_both_orders(&store, &rejoin, &metadata).await;

        assert_eq!(
            verdicts(&nodes[0], world, &[&metadata]),
            verdicts(&nodes[1], world, &[&metadata]),
            "a non-member's own metadata was judged after a rejoin its cut does not hold"
        );
        assert_same_rows(&nodes, |node| {
            MetadataRepository::new(node)
                .member_metadata(&world.namespace, &member)
                .expect("read the metadata")
                .map(|record| record.name)
        });
    }

    #[actix::test]
    async fn an_admin_change_concurrent_with_its_targets_removal_lands_the_same_in_either_order() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let member_sk = joined_member(&store, world, 0xF9, [0xF9; 32]).await;
        let member = account_for(&member_sk.public_key());
        let parents = heads(&store, world);
        let change = SignedNamespaceOp::sign(
            &world.owner_sk,
            world.namespace.to_bytes().into(),
            parents.clone(),
            unused_nonce(),
            seal_for_test(
                &store,
                world.namespace,
                guarded_root_op(
                    &store,
                    &world.namespace,
                    &world.owner_sk.public_key(),
                    RootOp::AdminChanged { new_admin: member },
                ),
            ),
        )
        .expect("sign the admin change");
        let remove = signed_in(
            &store,
            world,
            &world.namespace,
            &admin.sign_sk,
            &parents,
            unused_nonce(),
            &dummy_member_removed_op(member),
        );

        let nodes = in_both_orders(&store, &change, &remove).await;

        assert_landed_the_same(&nodes, world, &[&change, &remove], &member);
        assert_same_rows(&nodes, |node| {
            calimero_governance_store::MetaRepository::new(node)
                .load(&world.namespace)
                .expect("read the meta")
                .map(|meta| meta.admin_identity)
        });
    }

    #[actix::test]
    async fn an_ownership_transfer_concurrent_with_its_targets_demotion_lands_the_same_in_either_order(
    ) {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        // Another admin demotes: the owner's own standing moves with the transfer.
        let other_sk = joined_member(&store, world, 0xFA, [0xFA; 32]).await;
        publish(
            &store,
            &world.namespace,
            &world.owner_sk,
            &GroupOp::MemberRoleSet {
                member: account_for(&other_sk.public_key()),
                role: GroupMemberRole::Admin,
            },
        )
        .await
        .expect("the owner promotes another admin");
        let parents = heads(&store, world);
        let transfer = signed_in(
            &store,
            world,
            &world.namespace,
            &world.owner_sk,
            &parents,
            unused_nonce(),
            &guarded_group_op(
                &store,
                &world.namespace,
                &world.owner_sk.public_key(),
                GroupOp::TransferOwnership {
                    new_owner: admin.account,
                },
            ),
        );
        let demote = signed_in(
            &store,
            world,
            &world.namespace,
            &other_sk,
            &parents,
            unused_nonce(),
            &GroupOp::MemberRoleSet {
                member: admin.account,
                role: GroupMemberRole::Member,
            },
        );

        let nodes = in_both_orders(&store, &transfer, &demote).await;

        assert_landed_the_same(&nodes, world, &[&transfer, &demote], &admin.account);
        assert_same_rows(&nodes, |node| {
            calimero_governance_store::MetaRepository::new(node)
                .load(&world.namespace)
                .expect("read the meta")
                .map(|meta| meta.owner_identity)
        });
    }

    /// A root removal of a TEE takes its subgroup rows with it and blocks its re-entry there,
    /// as the live apply does. A subgroup re-admission whose cut holds that removal is
    /// refused, whichever concurrent op a node took first.
    #[actix::test]
    async fn a_root_tee_removal_cascades_into_its_subgroups_at_the_cut() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let tee = admitted_tee(&store, world, 0xFB).await;
        let seat = signed_in(
            &store,
            world,
            &world.restricted,
            &world.owner_sk,
            &heads(&store, world),
            unused_nonce(),
            &group_admission(tee, [0xFC; 32]),
        );
        apply(&store, &seat)
            .await
            .expect("the owner seats the TEE in the subgroup");
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&world.restricted, &tee)
                .expect("read the role"),
            Some(GroupMemberRole::ReadOnlyTee),
            "precondition: the TEE holds a subgroup row"
        );
        let remove = signed_in(
            &store,
            world,
            &world.namespace,
            &admin.sign_sk,
            &heads(&store, world),
            unused_nonce(),
            &dummy_member_removed_op(tee),
        );
        apply(&store, &remove)
            .await
            .expect("the admin removes the TEE at the root");
        let parents = heads(&store, world);
        let readmit = signed_in(
            &store,
            world,
            &world.restricted,
            &world.owner_sk,
            &parents,
            unused_nonce(),
            &group_admission(tee, [0xFD; 32]),
        );
        let other = logged_member(&store, world, 0xFE).await;
        let unrelated = signed_in(
            &store,
            world,
            &world.namespace,
            &admin.sign_sk,
            &parents,
            unused_nonce(),
            &GroupOp::MemberRoleSet {
                member: other,
                role: GroupMemberRole::ReadOnly,
            },
        );

        let capability = signed_in(
            &store,
            world,
            &world.restricted,
            &world.owner_sk,
            &parents,
            unused_nonce(),
            &GroupOp::MemberCapabilitySet {
                member: tee,
                capabilities: calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT,
            },
        );

        // The owner is this node's identity, so its refused op is not logged at all.
        for evicted in [&readmit, &capability] {
            let nodes = in_both_orders(&store, evicted, &unrelated).await;
            let id = evicted.content_hash().expect("hash the op");
            for node in &nodes {
                let logged = NamespaceOpLogService::new(node, world.namespace.to_bytes().into())
                    .contains_op(id)
                    .expect("read the op log");
                assert!(
                    !logged || verdicts(node, world, &[evicted]) == vec![Some(Parked::Refused)],
                    "an op on a TEE the root removal evicted from the subgroup applied at a cut \
                     holding the removal"
                );
            }
        }
    }

    /// A context registered in one group, registered in another concurrently with its detach
    /// from the first, lands the same in either order.
    #[actix::test]
    #[ignore = "a registration reads live whether its context is in another group: the cut's \
                answer needs every group's registrations of it, and a node outside a Restricted \
                group cannot open them; folding registration into OpPayload is a format change"]
    async fn a_registration_concurrent_with_a_detach_from_another_group_lands_the_same_in_either_order(
    ) {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let context = calimero_primitives::context::ContextId::from([0xC7; 32]);
        calimero_governance_store::register_context_in_group(&store, &world.open_chain, &context)
            .expect("register the context outside the log");
        let parents = heads(&store, world);
        let register = signed_in(
            &store,
            world,
            &world.namespace,
            &admin.sign_sk,
            &parents,
            unused_nonce(),
            &GroupOp::ContextRegistered {
                context_id: context,
                application_id: calimero_primitives::application::ZERO_APPLICATION_ID,
                blob_id: calimero_primitives::blobs::BlobId::from([0; 32]),
                source: String::new(),
                service_name: None,
                package: String::new(),
                version: String::new(),
            },
        );
        let detach = signed_in(
            &store,
            world,
            &world.open_chain,
            &admin.sign_sk,
            &parents,
            unused_nonce(),
            &GroupOp::ContextDetached {
                context_id: context,
            },
        );

        let nodes = in_both_orders(&store, &register, &detach).await;

        let ops = [&register, &detach];
        assert_eq!(
            verdicts(&nodes[0], world, &ops),
            verdicts(&nodes[1], world, &ops)
        );
        assert_same_rows(&nodes, |node| {
            calimero_governance_store::get_group_for_context(node, &context).expect("read")
        });
    }

    /// The projection's view drops a TEE's subgroup rows with its root removal, as the
    /// apply does, so every view-backed gate sees the eviction.
    #[actix::test]
    #[ignore = "the fold is stamp-ordered and a MemberRemoved payload does not say the removed \
                row was a TEE, so AclView cannot cascade it; the at-cut membership reads do, \
                and the view needs the cascade in OpPayload, a format change"]
    async fn the_view_drops_a_root_removed_tees_subgroup_rows() {
        let (world, store) = world_with_a_logged_admin().await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let tee = admitted_tee(&store, world, 0xC8).await;
        let seat = signed_in(
            &store,
            world,
            &world.restricted,
            &world.owner_sk,
            &heads(&store, world),
            unused_nonce(),
            &group_admission(tee, [0xC9; 32]),
        );
        apply(&store, &seat)
            .await
            .expect("the owner seats the TEE in the subgroup");
        publish(
            &store,
            &world.namespace,
            &admin.sign_sk,
            &dummy_member_removed_op(tee),
        )
        .await
        .expect("the admin removes the TEE at the root");
        let (projection, _namespace, heads) =
            ScopeProjections::ephemeral_projection(&store, &world.namespace)
                .expect("fold the namespace");

        assert_eq!(
            projection.effective_role_at_cut(&store, world.restricted, &tee, &heads),
            Some(None),
            "the view kept a subgroup row the root removal took"
        );
    }

    /// A nonce no signer of the world has used: a reused one is dropped as a duplicate.
    fn unused_nonce() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 32);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}
