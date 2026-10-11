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

/// A member publishes a link of a new device of the admin's account, its key from
/// `seed`, scoped to `applications`. Returns the device's key.
async fn link_a_device_of_the_admin(
    store: &Store,
    world: &World,
    seed: u8,
    applications: &[ApplicationId],
    vouched: Endorsement,
) -> PrivateKey {
    let admin = world.actor(ActorState::NamespaceAdmin);
    let member = world.actor(ActorState::DirectMember);
    let endorser = match vouched {
        Endorsement::NonMember => world.actor(ActorState::NonMember),
        Endorsement::Member | Endorsement::Forged => member,
    };
    let device_sk = PrivateKey::from([seed; 32]);
    let cert = DeviceCert::sign(
        &admin.root,
        admin.account,
        DeviceId::mint(admin.account, [seed; 16]),
        &device_sk.public_key(),
        &KemPublicKey::from([seed ^ 0x01; 32]),
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
    let device_sk = link_a_device_of_the_admin(store, world, 0xD2, &[], Endorsement::Forged).await;
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
    let device_sk =
        link_a_device_of_the_admin(&store, world, 0xDA, &[], Endorsement::NonMember).await;

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
/// `application` in the op log.
async fn world_targeting(application: ApplicationId) -> (&'static World, Store) {
    let (world, store) = world_with_a_logged_admin().await;
    publish(
        &store,
        &world.namespace,
        &world.owner_sk,
        &target(application),
    )
    .await
    .expect("the owner sets the namespace's target");
    (world, store)
}

#[actix::test]
async fn a_device_scoped_to_the_namespace_application_speaks_for_its_account() {
    let (world, store) = world_targeting(World::shared().application).await;
    let device_sk = link_a_device_of_the_admin(
        &store,
        world,
        0xD4,
        &[world.application],
        Endorsement::Member,
    )
    .await;

    let role = role_after_promotion_by(&store, world, &device_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Admin),
        "a device whose scope covers the namespace's application speaks for its account"
    );
}

#[actix::test]
async fn a_device_scoped_away_from_the_namespace_does_not_speak_for_its_account() {
    let (world, store) = world_targeting(World::shared().application).await;
    let elsewhere = ApplicationId::from([0xEE; 32]);
    let device_sk =
        link_a_device_of_the_admin(&store, world, 0xD5, &[elsewhere], Endorsement::Member).await;

    let role = role_after_promotion_by(&store, world, &device_sk).await;

    assert_eq!(
        role,
        Some(GroupMemberRole::Member),
        "a device whose scope excludes the namespace's application promoted a member"
    );
}

#[actix::test]
async fn a_device_scoped_away_from_the_namespace_writes_for_no_account_at_the_cut() {
    let (world, store) = world_targeting(World::shared().application).await;
    let elsewhere = ApplicationId::from([0xEE; 32]);
    let device_sk =
        link_a_device_of_the_admin(&store, world, 0xD6, &[elsewhere], Endorsement::Member).await;

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
    let (world, store) = world_targeting(World::shared().application).await;
    let device_sk = link_a_device_of_the_admin(
        &store,
        world,
        0xD7,
        &[world.application],
        Endorsement::Member,
    )
    .await;
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
        link_a_device_of_the_admin(&store, world, 0xD8, &[elsewhere], Endorsement::Member).await;

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
        // Signing is deterministic, so each pass builds the same history.
        let (world, store) = world_targeting(World::shared().application).await;
        let device_sk = link_a_device_of_the_admin(
            &store,
            world,
            0xD9,
            &[world.application],
            Endorsement::Member,
        )
        .await;
        let admin = world.actor(ActorState::NamespaceAdmin);
        let concurrent = |signer: &PrivateKey, application: ApplicationId| {
            let sealed = sealed_group_op(&store, &world.namespace, &target(application));
            sign_next(&store, &world.namespace, signer, sealed)
        };
        let id = |op: &SignedNamespaceOp| op.content_hash().expect("hash the op");
        let restated = concurrent(&world.owner_sk, world.application);
        // A rival the deterministic order ranks above the restated target, so the
        // device is out of scope wherever the two meet.
        let rival = (0xE0..=0xFF)
            .map(|byte| concurrent(&admin.sign_sk, ApplicationId::from([byte; 32])))
            .find(|rival| id(rival) > id(&restated))
            .expect("some rival target ranks above the restated one");
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
