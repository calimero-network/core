//! The shared world: one namespace with a Restricted subgroup and an Open
//! subgroup under it, and one actor per state, all reached through signed ops.

use std::sync::{Arc, OnceLock};

use calimero_account::{
    AccountGenesis, AccountId, AccountMemberEndorsement, AccountProof, DeviceCert, DeviceId,
    DeviceScope, KemPublicKey,
};
use calimero_context_client::local_governance::{GroupOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::KeyEnvelope;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::db::{Column, InMemoryDB};
use calimero_store::Store;
use strum::IntoEnumIterator;

use super::ActorState;
use crate::test_fixtures::{
    apply_member_joined, device_kem_secret, dummy_member_removed_op, group_created,
    namespace_genesis_v2_for, real_join_account, root_for, seal_for_test, signed_invitation_for,
    test_store, GENESIS_SALT,
};
use crate::{
    apply_signed_namespace_op, sign_apply_local_group_op_borsh, AccountBindingRepository,
    DeviceSecret, GroupKeyring, KeyRequester, MembershipPath, MembershipRepository,
    NamespaceRepository,
};

/// The application every group of the world targets.
const APP: [u8; 32] = [0xCC; 32];
/// An application that no scope of the world reaches.
const OTHER_APP: [u8; 32] = [0xEE; 32];

/// One actor: the key it speaks with and the account and devices behind it.
pub struct Actor {
    pub state: ActorState,
    /// The key the actor signs and requests with.
    pub sign_sk: PrivateKey,
    pub account: AccountId,
    /// The device the actor speaks as.
    pub device: DeviceId,
    /// Another device of the same account, live unless the actor is that account's
    /// only live one.
    pub peer: DeviceId,
    /// The account root, held by the test so it can certify and narrow devices.
    pub root: PrivateKey,
}

impl Actor {
    pub fn sign_pk(&self) -> PublicKey {
        self.sign_sk.public_key()
    }

    pub fn requester(&self) -> KeyRequester {
        KeyRequester {
            identity: self.sign_pk(),
            device: Some(self.device),
        }
    }

    /// The group key an envelope served to this actor carries, if the actor can
    /// open it as its member key or its device.
    pub fn open(&self, group: &ContextGroupId, envelope: &[u8]) -> Option<[u8; 32]> {
        let envelope: KeyEnvelope = borsh::from_slice(envelope).ok()?;
        let device = DeviceSecret {
            device: self.device,
            kem_secret: device_kem_secret(*self.device.as_bytes()),
        };
        GroupKeyring::unwrap_any(
            &self.sign_sk,
            Some(&device),
            &group.to_bytes(),
            None,
            &envelope,
        )
        .ok()
    }

    /// The root-signed certificate binding the actor's key to its device.
    pub fn proof(&self) -> AccountProof<DeviceCert> {
        AccountProof {
            genesis: AccountGenesis::new(self.root.public_key()),
            chain: vec![],
            statement: DeviceCert::sign(
                &self.root,
                self.account,
                self.device,
                &self.sign_pk(),
                &kem_public(&self.device),
                0,
                0,
            )
            .expect("the account root certifies its device"),
        }
    }
}

/// Everything a row needs; built once and read by every row.
pub struct World {
    pub store: Store,
    pub namespace: ContextGroupId,
    pub restricted: ContextGroupId,
    pub subject: ContextGroupId,
    pub open_chain: ContextGroupId,
    pub other_namespace: ContextGroupId,
    pub context: ContextId,
    pub owner_sk: PrivateKey,
    pub application: ApplicationId,
    /// A member whose device the revoke row tries to withdraw.
    pub victim: Actor,
    /// A subject member granted authorship on behalf of others.
    pub relay: Actor,
    actors: Vec<Actor>,
}

struct Nonces(u64);

impl Nonces {
    fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

fn sk(seed: u8) -> PrivateKey {
    PrivateKey::from([seed; 32])
}

fn kem_public(device: &DeviceId) -> KemPublicKey {
    KemPublicKey::from(
        *device_kem_secret(*device.as_bytes())
            .public_key()
            .as_bytes(),
    )
}

fn apply_root_op(store: &Store, namespace: &ContextGroupId, signer: &PrivateKey, op: RootOp) {
    let governance = crate::NamespaceGovernance::new(store, namespace.to_bytes().into());
    let head = governance.read_head_record().expect("read the DAG head");
    let sealed = seal_for_test(store, *namespace, op);
    let signed = SignedNamespaceOp::sign(
        signer,
        namespace.to_bytes().into(),
        head.parent_hashes.clone(),
        head.next_nonce,
        sealed,
    )
    .expect("sign a root op");
    governance
        .apply_signed_op(&signed)
        .expect("a root op the world needs applies");
}

fn created_group(op: &RootOp) -> ContextGroupId {
    match op {
        RootOp::GroupCreated { group_id, .. } => ContextGroupId::from(group_id.to_bytes()),
        _ => unreachable!("group_created builds a GroupCreated"),
    }
}

fn group_op(store: &Store, group: &ContextGroupId, signer: &PrivateKey, op: GroupOp) {
    let _signed = sign_apply_local_group_op_borsh(store, group, signer, op)
        .expect("a group op the world needs applies");
}

fn found_namespace(store: &Store, founder: &PrivateKey, salt: [u8; 32]) -> ContextGroupId {
    let (genesis, _account, id) = namespace_genesis_v2_for(founder, salt);
    let namespace = ContextGroupId::from(id);
    NamespaceRepository::new(store)
        .store_identity(&namespace, &founder.public_key(), founder.as_bytes())
        .expect("store the founder's namespace identity");
    let signed =
        SignedNamespaceOp::sign(founder, id.into(), vec![], 1, genesis).expect("sign the genesis");
    let _applied = apply_signed_namespace_op(store, &signed).expect("the genesis applies");
    namespace
}

/// A member that joined `namespace` by an invitation its founder issued.
struct Seat {
    primary: PrivateKey,
    root: PrivateKey,
    account: AccountId,
}

fn join(
    store: &Store,
    namespace: &ContextGroupId,
    inviter: &PrivateKey,
    nonces: &mut Nonces,
    seed: u8,
) -> Seat {
    let primary = sk(seed);
    let nonce = nonces.next();
    let mut invitation_nonce = [seed; 32];
    invitation_nonce[..8].copy_from_slice(&nonce.to_be_bytes());
    apply_member_joined(
        store,
        namespace.to_bytes(),
        &primary,
        signed_invitation_for(inviter, *namespace, invitation_nonce),
        nonce,
        inviter,
    )
    .expect("the join applies");
    Seat {
        root: root_for(&primary.public_key()),
        account: real_join_account(&primary.public_key()).statement.account,
        primary,
    }
}

/// Link a second device for `seat` through a link a member endorses.
fn link_sibling(
    store: &Store,
    namespace: &ContextGroupId,
    endorser: &PrivateKey,
    seat: &Seat,
    seed: u8,
) -> (PrivateKey, DeviceId, DeviceCert) {
    let device_sk = sk(seed ^ 0x80);
    let device = DeviceId::mint(seat.account, [seed; 16]);
    let cert = DeviceCert::sign(
        &seat.root,
        seat.account,
        device,
        &device_sk.public_key(),
        &kem_public(&device),
        0,
        0,
    )
    .expect("the account root certifies its device");
    group_op(
        store,
        namespace,
        endorser,
        GroupOp::AccountDeviceLinked {
            genesis: AccountGenesis::new(seat.root.public_key()),
            chain: vec![],
            cert,
            endorsement: AccountMemberEndorsement::sign(endorser, seat.account)
                .expect("endorse the account"),
            scope: Box::new(crate::test_fixtures::device_scope(
                &seat.root,
                &cert,
                vec![],
                0,
            )),
        },
    );
    (device_sk, device, cert)
}

fn primary_actor(state: ActorState, seat: &Seat, peer: DeviceId) -> Actor {
    let device = DeviceId::from(*seat.primary.public_key());
    Actor {
        state,
        sign_sk: PrivateKey::from(*seat.primary.as_bytes()),
        account: seat.account,
        device,
        peer,
        root: PrivateKey::from(*seat.root.as_bytes()),
    }
}

fn sibling_actor(state: ActorState, seat: &Seat, sibling: (PrivateKey, DeviceId)) -> Actor {
    let (device_sk, device) = sibling;
    Actor {
        state,
        sign_sk: device_sk,
        account: seat.account,
        device,
        peer: DeviceId::from(*seat.primary.public_key()),
        root: PrivateKey::from(*seat.root.as_bytes()),
    }
}

fn add_member(
    store: &Store,
    group: &ContextGroupId,
    owner: &PrivateKey,
    seat: &Seat,
    role: GroupMemberRole,
) {
    group_op(
        store,
        group,
        owner,
        GroupOp::MemberAdded {
            member: seat.account,
            role,
        },
    );
}

fn remove_member(store: &Store, group: &ContextGroupId, owner: &PrivateKey, seat: &Seat) {
    group_op(store, group, owner, dummy_member_removed_op(seat.account));
}

impl World {
    /// The world every row shares, built on first use.
    pub fn shared() -> &'static World {
        static WORLD: OnceLock<World> = OnceLock::new();
        WORLD.get_or_init(World::build)
    }

    pub fn actor(&self, state: ActorState) -> &Actor {
        self.actors
            .iter()
            .find(|a| a.state == state)
            .expect("the world builds an actor for every state")
    }

    /// A private copy of the store, for rows that write.
    pub fn fork(&self) -> Store {
        let copy = Store::new(Arc::new(InMemoryDB::owned()));
        let top = vec![0xFF; 4096];
        for column in Column::iter() {
            let rows = self
                .store
                .raw_scan(column, &[], &top, None)
                .expect("scan a column");
            for (key, value) in rows {
                copy.raw_put(column, &key, &value).expect("copy a row");
            }
        }
        copy
    }

    fn build() -> World {
        let store = test_store();
        let mut nonces = Nonces(0);
        let owner_sk = sk(0x11);
        let namespace = found_namespace(&store, &owner_sk, GENESIS_SALT);
        let owner_account = real_join_account(&owner_sk.public_key()).statement.account;

        // The restricted subgroup, then an open one beneath it: the open one has a
        // key of its own and inherits from the restricted one only.
        let restricted_op = group_created(owner_account, namespace.to_bytes(), true, 0x01);
        let restricted = created_group(&restricted_op);
        apply_root_op(&store, &namespace, &owner_sk, restricted_op);
        let subject_op = group_created(owner_account, restricted.to_bytes(), false, 0x02);
        let subject = created_group(&subject_op);
        apply_root_op(&store, &namespace, &owner_sk, subject_op);
        // An Open subgroup straight under the namespace, which the namespace key covers.
        let open_chain_op = group_created(owner_account, namespace.to_bytes(), false, 0x03);
        let open_chain = created_group(&open_chain_op);
        apply_root_op(&store, &namespace, &owner_sk, open_chain_op);
        // Every group gets a key row at birth, as `create_group` mints one.
        for (group, key) in [
            (restricted, 0x52u8),
            (subject, 0x53u8),
            (open_chain, 0x54u8),
        ] {
            let _id = GroupKeyring::new(&store, group)
                .store_key(&[key; 32])
                .expect("the creating node keys the group it creates");
        }

        // What a created group publishes: members may join its open subgroups.
        group_op(
            &store,
            &restricted,
            &owner_sk,
            GroupOp::DefaultCapabilitiesSet {
                capabilities: calimero_context_config::MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
            },
        );

        let context = ContextId::from([0xC5; 32]);
        group_op(
            &store,
            &subject,
            &owner_sk,
            GroupOp::ContextRegistered {
                context_id: context,
                application_id: ApplicationId::from(APP),
                blob_id: BlobId::from([0xB1; 32]),
                source: String::new(),
                service_name: None,
                package: "matrix.app".to_owned(),
                version: "1.0.0".to_owned(),
            },
        );

        let mut actors = Vec::new();
        let mut seat_for = |seed: u8| {
            let seat = join(&store, &namespace, &owner_sk, &mut nonces, seed);
            let sibling = link_sibling(&store, &namespace, &owner_sk, &seat, seed);
            (seat, sibling)
        };

        // Owner: founded the namespace and both subgroups.
        let owner_seat = Seat {
            primary: PrivateKey::from(*owner_sk.as_bytes()),
            root: root_for(&owner_sk.public_key()),
            account: owner_account,
        };
        let owner_sibling = link_sibling(&store, &namespace, &owner_sk, &owner_seat, 0x11);
        // The founder's device is the one its genesis credential certifies.
        let founder_device = DeviceId::from([0x3E; 32]);
        actors.push(Actor {
            device: founder_device,
            ..primary_actor(ActorState::Owner, &owner_seat, owner_sibling.1)
        });

        // Direct admin and direct member of the subject.
        let (seat, sibling) = seat_for(0x21);
        add_member(&store, &subject, &owner_sk, &seat, GroupMemberRole::Admin);
        actors.push(primary_actor(ActorState::DirectAdmin, &seat, sibling.1));
        let (seat, sibling) = seat_for(0x22);
        add_member(&store, &subject, &owner_sk, &seat, GroupMemberRole::Member);
        actors.push(primary_actor(ActorState::DirectMember, &seat, sibling.1));

        // An admin of the namespace that is neither its owner nor in the subject.
        let (seat, sibling) = seat_for(0x31);
        add_member(&store, &namespace, &owner_sk, &seat, GroupMemberRole::Admin);
        actors.push(primary_actor(ActorState::NamespaceAdmin, &seat, sibling.1));

        // Inherited admin and member: seated in the restricted parent only.
        let (seat, sibling) = seat_for(0x23);
        add_member(
            &store,
            &restricted,
            &owner_sk,
            &seat,
            GroupMemberRole::Admin,
        );
        actors.push(primary_actor(ActorState::InheritedAdmin, &seat, sibling.1));
        let (seat, sibling) = seat_for(0x24);
        add_member(
            &store,
            &restricted,
            &owner_sk,
            &seat,
            GroupMemberRole::Member,
        );
        actors.push(primary_actor(ActorState::InheritedMember, &seat, sibling.1));

        // Kicked: a member of the subject, inheriting it too, removed by an admin.
        let (seat, sibling) = seat_for(0x25);
        add_member(
            &store,
            &restricted,
            &owner_sk,
            &seat,
            GroupMemberRole::Member,
        );
        add_member(&store, &subject, &owner_sk, &seat, GroupMemberRole::Member);
        remove_member(&store, &subject, &owner_sk, &seat);
        actors.push(primary_actor(ActorState::Kicked, &seat, sibling.1));

        // Left: an inherited member that joined the subject and then left it.
        let (seat, sibling) = seat_for(0x26);
        add_member(
            &store,
            &restricted,
            &owner_sk,
            &seat,
            GroupMemberRole::Member,
        );
        add_member(&store, &subject, &owner_sk, &seat, GroupMemberRole::Member);
        group_op(
            &store,
            &subject,
            &seat.primary,
            GroupOp::MemberLeft {
                member: seat.account,
                expected_group_state_hash: [0; 32],
                expected_context_state_hashes: Vec::new(),
            },
        );
        actors.push(primary_actor(ActorState::Left, &seat, sibling.1));

        // Deny-listed: removed from the parent and the namespace, so no path left.
        let (seat, sibling) = seat_for(0x27);
        add_member(
            &store,
            &restricted,
            &owner_sk,
            &seat,
            GroupMemberRole::Member,
        );
        remove_member(&store, &restricted, &owner_sk, &seat);
        remove_member(&store, &namespace, &owner_sk, &seat);
        actors.push(primary_actor(ActorState::DenyListed, &seat, sibling.1));

        // Readmitted: kicked from the subject, then added back by an admin.
        let (seat, sibling) = seat_for(0x28);
        add_member(
            &store,
            &restricted,
            &owner_sk,
            &seat,
            GroupMemberRole::Member,
        );
        remove_member(&store, &subject, &owner_sk, &seat);
        assert!(
            crate::DenyListRepository::new(&store)
                .is_denied(&subject, &seat.account)
                .expect("read the deny list"),
            "the kick recorded"
        );
        add_member(&store, &subject, &owner_sk, &seat, GroupMemberRole::Member);
        actors.push(primary_actor(
            ActorState::ReadmittedAfterKick,
            &seat,
            sibling.1,
        ));

        // Three accounts, each an admin of the namespace and of the subject with a
        // second device in a different condition, so admin and anchor gates see it.
        let (seat, sibling) = seat_for(0x29);
        add_member(&store, &namespace, &owner_sk, &seat, GroupMemberRole::Admin);
        add_member(&store, &subject, &owner_sk, &seat, GroupMemberRole::Admin);
        group_op(
            &store,
            &namespace,
            &owner_sk,
            GroupOp::AccountDeviceUnlinked {
                account: seat.account,
                device: sibling.1,
                proof: None,
            },
        );
        actors.push(sibling_actor(
            ActorState::RevokedDevice,
            &seat,
            (sibling.0, sibling.1),
        ));

        let (seat, sibling) = seat_for(0x2A);
        add_member(&store, &namespace, &owner_sk, &seat, GroupMemberRole::Admin);
        add_member(&store, &subject, &owner_sk, &seat, GroupMemberRole::Admin);
        let narrowed = DeviceScope::sign(
            &seat.root,
            seat.account,
            sibling.1,
            vec![ApplicationId::from(OTHER_APP)],
            1,
            0,
        )
        .expect("the root narrows its device");
        group_op(
            &store,
            &namespace,
            &seat.primary,
            GroupOp::AccountDeviceDescoped {
                account: seat.account,
                device: sibling.1,
                application: Some(ApplicationId::from(APP)),
                scope: Box::new(calimero_account::AccountProof {
                    genesis: AccountGenesis::new(seat.root.public_key()),
                    chain: vec![],
                    statement: narrowed,
                }),
            },
        );
        actors.push(sibling_actor(
            ActorState::DescopedDevice,
            &seat,
            (sibling.0, sibling.1),
        ));

        let (seat, sibling) = seat_for(0x2B);
        add_member(&store, &namespace, &owner_sk, &seat, GroupMemberRole::Admin);
        add_member(&store, &subject, &owner_sk, &seat, GroupMemberRole::Admin);
        actors.push(sibling_actor(
            ActorState::SecondDevice,
            &seat,
            (sibling.0, sibling.1),
        ));

        // A member of a second namespace this node hosts under the same key.
        let other_namespace = found_namespace(&store, &owner_sk, [0x6B; 32]);
        let mut other_nonces = Nonces(0);
        let seat = join(&store, &other_namespace, &owner_sk, &mut other_nonces, 0x2C);
        let sibling = link_sibling(&store, &other_namespace, &owner_sk, &seat, 0x2C);
        actors.push(primary_actor(
            ActorState::OtherNamespaceMember,
            &seat,
            sibling.1,
        ));

        // Nobody: a key the namespace has never heard of.
        let stranger = Seat {
            primary: sk(0x2D),
            root: root_for(&sk(0x2D).public_key()),
            account: real_join_account(&sk(0x2D).public_key()).statement.account,
        };
        actors.push(primary_actor(
            ActorState::NonMember,
            &stranger,
            DeviceId::mint(stranger.account, [0x2D; 16]),
        ));

        // The victim of the revoke row: a member with devices of its own.
        let (victim_seat, victim_sibling) = seat_for(0x2E);
        add_member(
            &store,
            &subject,
            &owner_sk,
            &victim_seat,
            GroupMemberRole::Member,
        );
        let victim = primary_actor(ActorState::DirectMember, &victim_seat, victim_sibling.1);

        // The relay every delegated write in the world goes through.
        let (relay_seat, relay_sibling) = seat_for(0x30);
        add_member(
            &store,
            &subject,
            &owner_sk,
            &relay_seat,
            GroupMemberRole::Member,
        );
        group_op(
            &store,
            &subject,
            &owner_sk,
            GroupOp::MemberCapabilitySet {
                member: relay_seat.account,
                capabilities: calimero_context_config::MemberCapabilities::CAN_AUTHOR_ON_BEHALF,
            },
        );
        let relay = primary_actor(ActorState::DirectMember, &relay_seat, relay_sibling.1);

        let world = World {
            store,
            namespace,
            restricted,
            subject,
            open_chain,
            other_namespace,
            context,
            owner_sk,
            application: ApplicationId::from(APP),
            victim,
            relay,
            actors,
        };
        world.check_states();
        world
    }

    /// Each builder must have reached the state it names; fail the build if not.
    fn check_states(&self) {
        let members = MembershipRepository::new(&self.store);
        let bindings = AccountBindingRepository::new(&self.store);
        let deny = crate::DenyListRepository::new(&self.store);
        let path = |state: ActorState| {
            members
                .check_path(&self.subject, &self.actor(state).account)
                .expect("read a membership path")
        };
        let direct = |state: ActorState| {
            members
                .has_direct_member(&self.subject, &self.actor(state).account)
                .expect("read a direct row")
        };
        let denied = |state: ActorState| {
            deny.is_denied(&self.subject, &self.actor(state).account)
                .expect("read the deny list")
        };
        let admin = |group: &ContextGroupId, state: ActorState| {
            members
                .is_admin(group, &self.actor(state).account)
                .expect("read an admin row")
        };

        assert!(admin(&self.subject, ActorState::Owner));
        assert!(direct(ActorState::DirectAdmin) && admin(&self.subject, ActorState::DirectAdmin));
        assert!(direct(ActorState::DirectMember));
        assert!(!admin(&self.subject, ActorState::DirectMember));
        assert!(matches!(
            path(ActorState::InheritedAdmin),
            MembershipPath::Inherited {
                via_admin: true,
                ..
            }
        ));
        assert!(matches!(
            path(ActorState::InheritedMember),
            MembershipPath::Inherited {
                via_admin: false,
                ..
            }
        ));
        assert!(!direct(ActorState::InheritedMember));
        assert!(denied(ActorState::Kicked) && !direct(ActorState::Kicked));
        assert!(matches!(
            path(ActorState::Kicked),
            MembershipPath::Inherited { .. }
        ));
        assert!(denied(ActorState::Left) && !direct(ActorState::Left));
        assert!(matches!(
            path(ActorState::Left),
            MembershipPath::Inherited { .. }
        ));
        assert!(matches!(path(ActorState::DenyListed), MembershipPath::None));
        assert!(deny
            .is_denied(&self.namespace, &self.actor(ActorState::DenyListed).account)
            .expect("read the deny list"));
        assert!(direct(ActorState::ReadmittedAfterKick));
        assert!(!denied(ActorState::ReadmittedAfterKick));

        let live = |state: ActorState| {
            let actor = self.actor(state);
            bindings
                .live_bindings(&self.namespace)
                .expect("read live bindings")
                .iter()
                .any(|b| b.device == actor.device)
        };
        assert!(live(ActorState::SecondDevice));
        assert!(!live(ActorState::RevokedDevice));
        assert!(bindings
            .is_revoked(
                &self.namespace,
                self.actor(ActorState::RevokedDevice).device
            )
            .expect("read a tombstone"));
        assert!(!live(ActorState::DescopedDevice));
        assert!(
            !bindings
                .is_revoked(
                    &self.namespace,
                    self.actor(ActorState::DescopedDevice).device
                )
                .expect("read a tombstone"),
            "a descoped device is narrowed, not revoked"
        );
        assert!(live(ActorState::DirectMember));

        // The device states are admins, so anchor gates see their device standing.
        for state in [
            ActorState::RevokedDevice,
            ActorState::DescopedDevice,
            ActorState::SecondDevice,
        ] {
            assert!(admin(&self.subject, state), "{state:?} speaks for an admin");
            assert!(
                admin(&self.namespace, state),
                "{state:?} speaks for an admin"
            );
        }
        assert!(admin(&self.namespace, ActorState::NamespaceAdmin));
        assert!(matches!(
            path(ActorState::NamespaceAdmin),
            MembershipPath::None
        ));
        let other = self.actor(ActorState::OtherNamespaceMember);
        assert!(matches!(
            members
                .check_path(&self.other_namespace, &other.account)
                .expect("read a membership path"),
            MembershipPath::Direct
        ));
        assert!(bindings
            .live_bindings(&self.other_namespace)
            .expect("read live bindings")
            .iter()
            .any(|b| b.device == other.device));
        assert!(!live(ActorState::OtherNamespaceMember));
        assert!(!live(ActorState::NonMember));
        assert!(matches!(path(ActorState::NonMember), MembershipPath::None));
        let descoped = self.actor(ActorState::DescopedDevice);
        assert!(bindings
            .scope_floor(&self.namespace, descoped.account, descoped.device)
            .expect("read a scope floor")
            .is_some_and(|floor| floor > 0));
        assert_eq!(
            crate::key_covering_group(&self.store, &self.open_chain).expect("resolve a cover"),
            self.namespace,
            "the namespace key covers the open-chain subgroup"
        );
    }
}
