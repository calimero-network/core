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
