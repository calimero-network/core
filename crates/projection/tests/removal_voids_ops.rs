//! A device revocation voids the concurrent ops of the device it revokes, not
//! those of the account's other devices.

use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, KemPublicKey};
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_projection::{AuthorityBase, ScopeState};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use core::num::NonZeroU128;

fn scope() -> ScopeId {
    ScopeId::from([7u8; 32])
}

fn group() -> ContextGroupId {
    ContextGroupId::from([0x33; 32])
}

fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

struct Person {
    genesis: AccountGenesis,
    id: AccountId,
    root: PrivateKey,
}

impl Person {
    fn new(seed: u8) -> Self {
        let root = PrivateKey::from([seed; 32]);
        let genesis = AccountGenesis::new(root.public_key());
        Self {
            id: genesis.account_id(),
            genesis,
            root,
        }
    }

    fn device(&self, seed: u8) -> Device {
        let sk = PrivateKey::from([seed; 32]);
        let id = DeviceId::mint(self.id, [seed; 16]);
        let cert = DeviceCert::sign(
            &self.root,
            self.id,
            id,
            &sk.public_key(),
            &KemPublicKey::from([seed; 32]),
            0,
            0,
        )
        .expect("sign cert");
        Device {
            id,
            cert,
            sk,
            account: self.id,
            genesis: self.genesis,
        }
    }
}

struct Device {
    id: DeviceId,
    cert: DeviceCert,
    sk: PrivateKey,
    account: AccountId,
    genesis: AccountGenesis,
}

impl Device {
    fn op(&self, ns: u64, parents: &[&Op], payload: OpPayload) -> Op {
        let parents: Vec<[u8; 32]> = parents.iter().map(|p| p.id()).collect();
        let authorship = Authorship {
            account: self.account,
            device: self.id,
            device_key: self.sk.public_key(),
        };
        let h = hlc(ns);
        let id = Op::compute_id(scope(), &parents, &authorship, &h, &payload);
        Op::new(
            scope(),
            parents,
            authorship,
            h,
            payload,
            [0u8; 32],
            self.sk.sign(&id).expect("sign").to_bytes(),
        )
    }

    fn link(&self, ns: u64, parents: &[&Op]) -> Op {
        self.op(
            ns,
            parents,
            OpPayload::DeviceLinked {
                genesis: self.genesis,
                chain: Vec::new(),
                cert: self.cert,
                scope_epoch: 0,
            },
        )
    }
}

fn add(author: &Device, ns: u64, parents: &[&Op], member: AccountId, role: GroupMemberRole) -> Op {
    author.op(
        ns,
        parents,
        OpPayload::MemberAdded {
            group: group(),
            member,
            role,
        },
    )
}

#[test]
fn a_revoked_device_cannot_act_from_a_cut_before_its_revocation() {
    let alice = Person::new(1);
    let bob = Person::new(2);
    let alice_device = alice.device(11);
    let bob_phone = bob.device(21);
    let bob_laptop = bob.device(22);
    let carol = Person::new(3);

    // Alice is the root admin; Bob is an admin with two devices.
    let link_alice = alice_device.link(10, &[]);
    let root = alice_device.op(
        20,
        &[&link_alice],
        OpPayload::AdminChanged {
            new_admin: alice.id,
        },
    );
    let link_phone = bob_phone.link(30, &[&root]);
    let link_laptop = bob_laptop.link(31, &[&link_phone]);
    let bob_admin = add(
        &alice_device,
        40,
        &[&link_laptop],
        bob.id,
        GroupMemberRole::Admin,
    );

    // Alice revokes Bob's laptop. The laptop, offline, adds Carol as an admin
    // from the cut before that; the phone adds her as a member from the same cut.
    let revocation = alice_device.op(
        50,
        &[&bob_admin],
        OpPayload::DeviceRevoked {
            account: bob.id,
            device: bob_laptop.id,
        },
    );
    let by_laptop = add(
        &bob_laptop,
        51,
        &[&bob_admin],
        carol.id,
        GroupMemberRole::Admin,
    );
    let by_phone = add(
        &bob_phone,
        52,
        &[&bob_admin],
        carol.id,
        GroupMemberRole::Member,
    );
    let log = vec![
        link_alice,
        root,
        link_phone,
        link_laptop,
        bob_admin,
        revocation.clone(),
        by_laptop.clone(),
        by_phone.clone(),
    ];

    let void = ScopeState::void_ops(&log, AuthorityBase::default());
    assert!(
        void.contains(&by_laptop.id()),
        "the revoked device's concurrent op carries no authority"
    );
    assert!(
        !void.contains(&by_phone.id()),
        "the account's other device is untouched"
    );
    assert!(!void.contains(&revocation.id()));

    let heads = [revocation.id(), by_laptop.id(), by_phone.id()];
    let view = ScopeState::acl_view_at(&log, &heads);
    assert_eq!(
        view.groups
            .get(&group())
            .and_then(|members| members.get(&carol.id)),
        Some(&GroupMemberRole::Member),
        "Carol is only the member the phone added"
    );
}

#[test]
fn an_op_of_a_revoked_device_built_without_an_author_is_still_void() {
    let alice = Person::new(1);
    let bob = Person::new(2);
    let alice_device = alice.device(11);
    let bob_phone = bob.device(21);
    let bob_laptop = bob.device(22);
    let carol = Person::new(3);

    let link_alice = alice_device.link(10, &[]);
    let root = alice_device.op(
        20,
        &[&link_alice],
        OpPayload::AdminChanged {
            new_admin: alice.id,
        },
    );
    let link_phone = bob_phone.link(30, &[&root]);
    let link_laptop = bob_laptop.link(31, &[&link_phone]);
    let bob_admin = add(
        &alice_device,
        40,
        &[&link_laptop],
        bob.id,
        GroupMemberRole::Admin,
    );
    let revocation = alice_device.op(
        50,
        &[&bob_admin],
        OpPayload::DeviceRevoked {
            account: bob.id,
            device: bob_laptop.id,
        },
    );

    // A node that applied the revocation first builds the laptop's op with the
    // key it signed with and no account, its binding being gone.
    let parents = vec![bob_admin.id()];
    let payload = OpPayload::MemberAdded {
        group: group(),
        member: carol.id,
        role: GroupMemberRole::Admin,
    };
    let authorship = Authorship::unattributed(bob_laptop.sk.public_key());
    let h = hlc(51);
    let id = Op::compute_id(scope(), &parents, &authorship, &h, &payload);
    let by_laptop = Op::new(
        scope(),
        parents,
        authorship,
        h,
        payload,
        [0u8; 32],
        bob_laptop.sk.sign(&id).expect("sign").to_bytes(),
    );

    let log = vec![
        link_alice,
        root,
        link_phone,
        link_laptop,
        bob_admin,
        revocation,
        by_laptop.clone(),
    ];
    assert!(
        ScopeState::void_ops(&log, AuthorityBase::default()).contains(&by_laptop.id()),
        "the revoked device's op is void however a node built it"
    );
}

struct World {
    log: Vec<Op>,
    bob_admin: Op,
    alice_device: Device,
    bob_phone: Device,
    bob_laptop: Device,
    carol: Person,
    carol_device: Device,
    bob: Person,
}

/// Alice is the root admin, Bob an admin with a phone and a laptop, Carol a plain
/// member with a device of her own.
fn world() -> World {
    let alice = Person::new(1);
    let bob = Person::new(2);
    let carol = Person::new(3);
    let alice_device = alice.device(11);
    let bob_phone = bob.device(21);
    let bob_laptop = bob.device(22);
    let carol_device = carol.device(31);

    let link_alice = alice_device.link(10, &[]);
    let root = alice_device.op(
        20,
        &[&link_alice],
        OpPayload::AdminChanged {
            new_admin: alice.id,
        },
    );
    let link_phone = bob_phone.link(30, &[&root]);
    let link_laptop = bob_laptop.link(31, &[&link_phone]);
    let link_carol = carol_device.link(32, &[&link_laptop]);
    let bob_admin = add(
        &alice_device,
        40,
        &[&link_carol],
        bob.id,
        GroupMemberRole::Admin,
    );
    let carol_member = add(
        &alice_device,
        41,
        &[&bob_admin],
        carol.id,
        GroupMemberRole::Member,
    );
    World {
        log: vec![
            link_alice,
            root,
            link_phone,
            link_laptop,
            link_carol,
            bob_admin.clone(),
            carol_member,
        ],
        bob_admin,
        alice_device,
        bob_phone,
        bob_laptop,
        carol,
        carol_device,
        bob,
    }
}

#[test]
fn a_revocation_nobody_was_entitled_to_make_voids_nothing() {
    let mut w = world();
    let head = w.log.last().unwrap().clone();

    // Carol, a plain member, revokes Bob's phone from a cut before anything else
    // happened, first naming Bob's account and then her own beside his device. The
    // apply refuses both and still logs them.
    let by_phone = add(
        &w.bob_phone,
        60,
        &[&head],
        w.carol.id,
        GroupMemberRole::Admin,
    );
    let names_bob = w.carol_device.op(
        61,
        &[&w.bob_admin],
        OpPayload::DeviceRevoked {
            account: w.bob.id,
            device: w.bob_phone.id,
        },
    );
    let names_herself = w.carol_device.op(
        62,
        &[&w.bob_admin],
        OpPayload::DeviceRevoked {
            account: w.carol.id,
            device: w.bob_phone.id,
        },
    );
    w.log.extend([by_phone.clone(), names_bob, names_herself]);

    assert!(
        !ScopeState::void_ops(&w.log, AuthorityBase::default()).contains(&by_phone.id()),
        "an op nobody was entitled to make takes away nobody's authority"
    );
}

#[test]
fn an_account_revoking_its_own_device_voids_that_devices_concurrent_ops() {
    let mut w = world();
    let head = w.log.last().unwrap().clone();

    let revocation = w.bob_phone.op(
        60,
        &[&head],
        OpPayload::DeviceRevoked {
            account: w.bob.id,
            device: w.bob_laptop.id,
        },
    );
    let by_laptop = add(
        &w.bob_laptop,
        61,
        &[&head],
        w.carol.id,
        GroupMemberRole::Admin,
    );
    w.log.extend([revocation, by_laptop.clone()]);

    assert!(ScopeState::void_ops(&w.log, AuthorityBase::default()).contains(&by_laptop.id()));
    let _ = &w.alice_device;
}

#[test]
fn an_admin_removed_while_revoking_its_removers_device_is_still_removed() {
    let mut w = world();
    let head = w.log.last().unwrap().clone();

    // Two admins who are not the owner: Bob and Dana.
    let dana = Person::new(4);
    let dana_device = dana.device(41);
    let link_dana = dana_device.link(70, &[&head]);
    let dana_admin = add(
        &w.bob_phone,
        71,
        &[&link_dana],
        dana.id,
        GroupMemberRole::Admin,
    );
    w.log.extend([link_dana, dana_admin.clone()]);

    // Bob removes Dana. Dana, from the cut before, revokes Bob's phone.
    let removal = w.bob_phone.op(
        80,
        &[&dana_admin],
        OpPayload::MemberRemoved {
            group: group(),
            member: dana.id,
        },
    );
    let revocation = dana_device.op(
        81,
        &[&dana_admin],
        OpPayload::DeviceRevoked {
            account: w.bob.id,
            device: w.bob_phone.id,
        },
    );
    w.log.extend([removal.clone(), revocation.clone()]);

    let void = ScopeState::void_ops(&w.log, AuthorityBase::default());
    assert!(!void.contains(&removal.id()), "Dana is removed");
    assert!(
        !void.contains(&revocation.id()),
        "and Bob's phone is revoked: each removed the other"
    );
}

#[test]
fn a_removal_built_without_an_author_is_seen_from_the_log_s_device_links() {
    let mut w = world();
    let head = w.log.last().unwrap().clone();

    // Alice's node built her removal of Bob after her binding was gone.
    let payload = OpPayload::MemberRemoved {
        group: group(),
        member: w.bob.id,
    };
    let authorship = Authorship::unattributed(w.alice_device.sk.public_key());
    let parents = vec![head.id()];
    let h = hlc(60);
    let id = Op::compute_id(scope(), &parents, &authorship, &h, &payload);
    let removal = Op::new(
        scope(),
        parents,
        authorship,
        h,
        payload,
        [0u8; 32],
        w.alice_device.sk.sign(&id).expect("sign").to_bytes(),
    );
    let dana = Person::new(4);
    let by_bob = add(&w.bob_phone, 61, &[&head], dana.id, GroupMemberRole::Member);
    w.log.extend([removal, by_bob.clone()]);

    assert!(
        ScopeState::void_ops(&w.log, AuthorityBase::default()).contains(&by_bob.id()),
        "the removal is Alice's however her node built it"
    );
}
