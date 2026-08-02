//! End-to-end behaviour of the account plane: linking devices, revoking them,
//! rotating account root keys, and the convergence and authorization properties
//! that have to hold while all of that happens concurrently on several nodes.
//!
//! The unit tests in `calimero-account` prove a credential is well-formed in
//! isolation. These prove the parts fit together — that a credential which
//! verifies is also *admitted*, that admission is identical on every node
//! regardless of delivery order, and that the ways a device can lose authority
//! actually take effect.
//!
//! Two properties carry most of the weight:
//!
//! - **Convergence.** Same ops, any order → same `scope_root`. The account
//!   plane deliberately uses no last-writer-wins stamps (grow-only maps plus
//!   monotone epochs), so this should hold structurally rather than by
//!   tie-break; these tests are what would catch it if some later change
//!   quietly introduced order-dependence.
//! - **Causal honor.** An op is judged against the state at *its own* causal
//!   cut, never the receiver's latest. So a write authored before a revocation
//!   stays valid even on a node that already applied the revocation.

use std::collections::BTreeMap;

use calimero_account::{
    AccountGenesis, AccountId, DeviceCert, DeviceId, KemPublicKey, RootKeyHandoff,
};
use calimero_authz::{authorize, Rejected};
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_projection::ScopeState;
use calimero_storage::address::Id;
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use core::num::NonZeroU128;

fn scope() -> ScopeId {
    ScopeId::from([7u8; 32])
}

// ---------------------------------------------------------------- fixtures --

/// Deterministic keypair, so any failure reproduces exactly.
fn key(seed: u8) -> PrivateKey {
    PrivateKey::from([seed; 32])
}

fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

/// One person: a root key they keep offline, and the devices they enroll.
struct Account {
    genesis: AccountGenesis,
    id: AccountId,
    /// Signed handoffs, in chain order. Empty until the root key rotates.
    chain: Vec<RootKeyHandoff>,
    /// The root key currently in force, and its epoch.
    root: PrivateKey,
    epoch: u32,
}

impl Account {
    fn new(root_seed: u8) -> Self {
        let root = key(root_seed);
        let genesis = AccountGenesis::new(root.public_key(), [root_seed; 16]);
        Self {
            id: genesis.account_id(),
            genesis,
            chain: Vec::new(),
            root,
            epoch: 0,
        }
    }

    /// Roll the root key, returning the handoff to publish.
    fn rotate_to(&mut self, new_seed: u8) -> RootKeyHandoff {
        let new_root = key(new_seed);
        let handoff = calimero_account::sign_root_key_handoff(
            &self.root,
            self.id,
            self.epoch,
            &new_root.public_key(),
        )
        .expect("sign handoff");
        self.chain.push(handoff);
        self.root = new_root;
        self.epoch += 1;
        handoff
    }

    /// Enroll a device, certified by the root key currently in force.
    fn enroll(&self, device_seed: u8, device_epoch: u32) -> Device {
        self.enroll_signed_by(device_seed, device_epoch, &self.root, self.epoch)
    }

    /// Enroll certified by an explicit key/epoch — for the adversarial cases
    /// where a superseded root key tries to mint a certificate.
    fn enroll_signed_by(
        &self,
        device_seed: u8,
        device_epoch: u32,
        signer: &PrivateKey,
        key_epoch: u32,
    ) -> Device {
        let sk = key(device_seed);
        let id = DeviceId::mint(self.id, [device_seed; 16]);
        Device {
            id,
            cert: calimero_account::sign_device_cert(
                signer,
                self.id,
                id,
                &sk.public_key(),
                &KemPublicKey::from([device_seed; 32]),
                key_epoch,
                device_epoch,
            )
            .expect("sign cert"),
            sk,
            account: self.id,
        }
    }

    /// The `DeviceLinked` op this device would author into a scope.
    fn link_op(&self, device: &Device, ns: u64, parents: Vec<[u8; 32]>) -> Op {
        device.sign_op(
            ns,
            parents,
            OpPayload::DeviceLinked {
                genesis: self.genesis,
                chain: self.chain.clone(),
                cert: device.cert,
            },
        )
    }
}

struct Device {
    id: DeviceId,
    cert: DeviceCert,
    sk: PrivateKey,
    account: AccountId,
}

impl Device {
    fn authorship(&self) -> Authorship {
        Authorship {
            account: self.account,
            device: self.id,
            device_key: self.sk.public_key(),
        }
    }

    /// Author and sign an op as this device.
    fn sign_op(&self, ns: u64, parents: Vec<[u8; 32]>, payload: OpPayload) -> Op {
        let authorship = self.authorship();
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
}

/// A group whose membership the tests grant against.
fn group() -> ContextGroupId {
    ContextGroupId::from([0x33; 32])
}

/// The op that makes `account` a member — authored by the scope's root admin.
fn grant_membership(admin: &Device, account: AccountId, ns: u64, parents: Vec<[u8; 32]>) -> Op {
    admin.sign_op(
        ns,
        parents,
        OpPayload::MemberAdded {
            group: group(),
            member: account,
            role: GroupMemberRole::Member,
        },
    )
}

/// Authorize `op` at its own causal cut over `log` — the real decision path.
fn decide(log: &[Op], op: &Op) -> Result<(), Rejected> {
    let view = ScopeState::acl_view_at(log, &op.parents);
    authorize(op, &view)
}

/// A scope bootstrapped with an admin account whose device is linked, so tests
/// can start from "there is somebody who can grant membership".
struct Fixture {
    admin: Device,
    log: Vec<Op>,
    head: Vec<[u8; 32]>,
}

impl Fixture {
    fn new() -> Self {
        let admin_account = Account::new(1);
        let admin = admin_account.enroll(2, 0);

        // Genesis: the admin's device link, then the admin as root admin. The
        // link is authored first because every later op needs the binding.
        let link = admin_account.link_op(&admin, 10, vec![]);
        let mut log = vec![link.clone()];
        let mut head = vec![link.id()];

        let admin_op = admin.sign_op(
            20,
            head.clone(),
            OpPayload::AdminChanged {
                new_admin: admin_account.id,
            },
        );
        head = vec![admin_op.id()];
        log.push(admin_op);

        Self { admin, log, head }
    }

    fn push(&mut self, op: Op) {
        self.head = vec![op.id()];
        self.log.push(op);
    }

    fn root(&self) -> [u8; 32] {
        ScopeState::from_ops(&self.log).root()
    }
}

// ------------------------------------------------------------- happy paths --

#[test]
fn a_first_device_links_itself_once_its_account_is_a_member() {
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone = alice.enroll(11, 0);

    // Before the grant the device has no business here: linking would let any
    // stranger write link ops into a scope they have no relationship with.
    let premature = alice.link_op(&phone, 30, fx.head.clone());
    assert_eq!(
        decide(&fx.log, &premature),
        Err(Rejected::AccountNotMember),
        "a device must not link itself into a scope its account isn't in"
    );

    // The admin grants the ACCOUNT, naming no device.
    let grant = grant_membership(&fx.admin, alice.id, 30, fx.head.clone());
    fx.push(grant);

    // Now the device links itself — no admin action, no new grant.
    let link = alice.link_op(&phone, 40, fx.head.clone());
    assert_eq!(decide(&fx.log, &link), Ok(()));
    fx.push(link);

    // And it can write.
    let write = phone.sign_op(
        50,
        fx.head.clone(),
        OpPayload::Put {
            entity: Id::new([0xAA; 32]),
            value: b"hi".to_vec(),
        },
    );
    assert_eq!(decide(&fx.log, &write), Ok(()));
}

#[test]
fn a_second_device_links_with_no_further_grant() {
    // The whole point of accounts: Alice buys a laptop and the admin does
    // nothing. Linking is not a privilege escalation because the account
    // already holds every right the device gains.
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone = alice.enroll(11, 0);
    let laptop = alice.enroll(12, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    let link_phone = alice.link_op(&phone, 40, fx.head.clone());
    fx.push(link_phone);

    let link_laptop = alice.link_op(&laptop, 50, fx.head.clone());
    assert_eq!(
        decide(&fx.log, &link_laptop),
        Ok(()),
        "a second device needs no new membership grant"
    );
    fx.push(link_laptop);

    // Both devices author as the SAME account...
    let from_phone = phone.sign_op(
        60,
        fx.head.clone(),
        OpPayload::Put {
            entity: Id::new([1; 32]),
            value: vec![1],
        },
    );
    let from_laptop = laptop.sign_op(
        61,
        fx.head.clone(),
        OpPayload::Put {
            entity: Id::new([2; 32]),
            value: vec![2],
        },
    );
    assert_eq!(decide(&fx.log, &from_phone), Ok(()));
    assert_eq!(decide(&fx.log, &from_laptop), Ok(()));
    assert_eq!(from_phone.author(), from_laptop.author());

    // ...but remain DISTINCT replicas. This is the invariant the whole design
    // exists to protect: sharing a replica id is what silently loses counter
    // increments and RGA characters.
    assert_ne!(
        from_phone.device(),
        from_laptop.device(),
        "one account, two devices — the replica ids must not collide"
    );
}

#[test]
fn two_devices_sharing_a_replica_seed_converge_on_the_lower_id() {
    // The seed rule must be a function of the folded SET, not of arrival order.
    // `admit_device_link` used to reject an incoming device only when an
    // already-folded one compared LOWER, which is order-dependent in the
    // direction it did not check: low-then-high left one device live, but
    // high-then-low admitted BOTH — and two replicas sharing an HLC seed mint
    // colliding RGA ids and lose characters silently, which is the whole reason
    // the rule exists.
    let alice = Account::new(10);

    // Forge two ids sharing an hlc_seed (the id's first 16 bytes) rather than
    // hunting for a `mint` nonce collision.
    let mut low_id = [0u8; 32];
    low_id[..16].copy_from_slice(&[0xAA; 16]);
    let mut high_id = low_id;
    high_id[31] = 0xFF;
    let (low_id, high_id) = (DeviceId::from(low_id), DeviceId::from(high_id));
    assert_eq!(low_id.hlc_seed(), high_id.hlc_seed());
    assert!(low_id < high_id);

    let forge = |device_seed: u8, id: DeviceId| {
        let sk = key(device_seed);
        Device {
            id,
            cert: calimero_account::sign_device_cert(
                &alice.root,
                alice.id,
                id,
                &sk.public_key(),
                &KemPublicKey::from([device_seed; 32]),
                alice.epoch,
                0,
            )
            .expect("sign cert"),
            sk,
            account: alice.id,
        }
    };
    let low = forge(11, low_id);
    let high = forge(12, high_id);

    // Fold both links in each order and compare the resulting live view. Using
    // `root()` compares the whole account plane, not just the device map, so a
    // divergence anywhere in the fold shows up.
    let live_and_root = |first: &Device, second: &Device| {
        let mut fx = Fixture::new();
        fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
        let a = alice.link_op(first, 40, fx.head.clone());
        fx.push(a);
        let b = alice.link_op(second, 50, fx.head.clone());
        fx.push(b);

        let view = ScopeState::acl_view_at(&fx.log, &fx.head);
        let mut devices: Vec<DeviceId> = view
            .devices
            .keys()
            .copied()
            .filter(|d| *d == low_id || *d == high_id)
            .collect();
        devices.sort_unstable();
        (devices, fx.root())
    };

    let (low_first, root_low_first) = live_and_root(&low, &high);
    let (high_first, root_high_first) = live_and_root(&high, &low);

    assert_eq!(
        low_first, high_first,
        "the live device set must not depend on which link folded first"
    );
    assert_eq!(
        low_first,
        vec![low_id],
        "the lower device id is the arbitrary-but-fixed winner"
    );
    assert_eq!(
        root_low_first, root_high_first,
        "the account plane folds into the root hash, so an order-dependent live \
         set would also split the root"
    );
}

#[test]
fn a_stranger_cannot_suppress_another_accounts_root_key_rotation() {
    // `absorb_handoff` keys by the HANDOFF's own account field and runs before
    // the credential is verified, gated only on `genesis.account_id() ==
    // cert.account` — which anyone can satisfy, because a genesis is public data.
    //
    // So a stranger can author a DeviceLinked op naming the victim's account,
    // carrying a bogus handoff for the victim at epoch 0. The link itself is
    // refused (they cannot sign a cert under the victim's root), but the handoff
    // is absorbed first. `absorb_handoff` breaks ties on the raw new_root_sign_pk
    // bytes, so a ground key can win the slot — and `resolved_accounts` STOPS at
    // the first handoff whose signature does not verify. The victim's real
    // rotation is then never reached, reverting them to the old root key they
    // rotated away from. That is a rotation rollback, and it converges, so it
    // never shows up as a divergence.
    let mut fx = Fixture::new();
    let mut victim = Account::new(10);
    let phone = victim.enroll(11, 0);
    fx.push(grant_membership(&fx.admin, victim.id, 30, fx.head.clone()));
    fx.push(victim.link_op(&phone, 40, fx.head.clone()));

    // The victim rotates its root key away from key(10) onto key(12).
    let real = victim.rotate_to(12);
    let rotate = phone.sign_op(
        50,
        fx.head.clone(),
        OpPayload::AccountKeysRotated { handoff: real },
    );
    fx.push(rotate);
    let rotated = ScopeState::from_ops(&fx.log);
    assert!(
        rotated
            .acl_view()
            .accounts
            .get(&victim.id)
            .is_some_and(|a| a.epoch == 1),
        "precondition: the victim's rotation took effect"
    );

    // A stranger forges a handoff for the victim's epoch 0. It cannot verify
    // (they lack the victim's root key), but it only has to WIN THE SLOT.
    let mallory = Account::new(20);
    let mallory_device = mallory.enroll(21, 0);
    fx.push(grant_membership(&fx.admin, mallory.id, 60, fx.head.clone()));
    fx.push(mallory.link_op(&mallory_device, 70, fx.head.clone()));

    let mut forged = real;
    forged.new_root_sign_pk = calimero_primitives::identity::PublicKey::from([0u8; 32]);
    let poison = mallory_device.sign_op(
        80,
        fx.head.clone(),
        OpPayload::DeviceLinked {
            genesis: victim.genesis, // public data
            chain: vec![forged],
            cert: phone.cert, // not signable by Mallory; the link WILL be refused
        },
    );
    fx.push(poison);

    let after = ScopeState::from_ops(&fx.log);
    assert!(
        after
            .acl_view()
            .accounts
            .get(&victim.id)
            .is_some_and(|a| a.epoch == 1),
        "a stranger must not be able to roll the victim's root key back to a \
         superseded epoch by crowding out its handoff"
    );
}

#[test]
fn an_overlong_handoff_chain_absorbs_nothing() {
    // Absorption runs BEFORE the credential is verified, so the cap inside
    // `root_key_at_epoch` does not protect it — without one in the fold, a single
    // op grows `handoffs` without limit, and this crate has no wire-bounds layer.
    // Refusing the whole op's absorption rather than truncating keeps the decision
    // a function of the op, so every replica absorbs the same set.
    let mut fx = Fixture::new();
    let mut alice = Account::new(10);
    let phone = alice.enroll(11, 0);
    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));

    let real = alice.rotate_to(12);
    let padded = vec![real; calimero_account::MAX_ROOT_KEY_HANDOFFS + 1];
    let link = phone.sign_op(
        40,
        fx.head.clone(),
        OpPayload::DeviceLinked {
            genesis: alice.genesis,
            chain: padded,
            cert: phone.cert,
        },
    );
    fx.push(link);

    let after = ScopeState::from_ops(&fx.log);
    let view = after.acl_view();
    assert!(
        !view.accounts.contains_key(&alice.id),
        "an op whose chain exceeds the cap must absorb nothing at all — not its \
         genesis, and not a truncated prefix of its handoffs"
    );
    // And the link itself must not land either. Absorption and admission are
    // separate steps, so skipping only the former would leave a device bound to an
    // account whose genesis was never learned — live in `devices` while absent from
    // `accounts`, which `live_devices` reads as unrestricted.
    assert!(
        !view.devices.contains_key(&phone.id),
        "an over-cap chain must not admit the link either"
    );
}

#[test]
fn a_forged_handoff_reusing_the_real_new_key_cannot_displace_it() {
    // The gap the first rollback fix left. Candidates were keyed by the new-root
    // key alone — but that key is broadcast in the clear, so an attacker can
    // author a handoff reusing it with a GARBAGE signature, land on the identical
    // map key, and overwrite the legitimate correctly-signed entry. The walk then
    // finds only the corrupted candidate, fails to verify it, and stops — the same
    // rollback, straight through the defence.
    //
    // The earlier test only covered a forged key that DIFFERED from the real one,
    // which lands in its own slot and was already handled.
    let mut fx = Fixture::new();
    let mut victim = Account::new(10);
    let phone = victim.enroll(11, 0);
    fx.push(grant_membership(&fx.admin, victim.id, 30, fx.head.clone()));
    fx.push(victim.link_op(&phone, 40, fx.head.clone()));

    let real = victim.rotate_to(12);
    fx.push(phone.sign_op(
        50,
        fx.head.clone(),
        OpPayload::AccountKeysRotated { handoff: real },
    ));

    let mallory = Account::new(20);
    let mallory_device = mallory.enroll(21, 0);
    fx.push(grant_membership(&fx.admin, mallory.id, 60, fx.head.clone()));
    fx.push(mallory.link_op(&mallory_device, 70, fx.head.clone()));

    // Same account, same from_epoch, SAME new_root_sign_pk — only the signature
    // differs. This is what collided.
    let mut forged = real;
    forged.signature = [0u8; 64];
    fx.push(mallory_device.sign_op(
        80,
        fx.head.clone(),
        OpPayload::DeviceLinked {
            genesis: victim.genesis,
            chain: vec![forged],
            cert: phone.cert,
        },
    ));

    let after = ScopeState::from_ops(&fx.log);
    assert!(
        after
            .acl_view()
            .accounts
            .get(&victim.id)
            .is_some_and(|a| a.epoch == 1),
        "a forged handoff reusing the real new-root key must not displace the \
         legitimate one and roll the account back"
    );
}

/// The same convergence property over a workload of ops that DISAGREE with each
/// other — which is the only kind that has ever found an order-dependence bug
/// here.
///
/// Every order-dependence defect in this plane came from a rule that read
/// "whatever has folded so far": seed collisions resolved against the devices
/// already present, the revocation tombstone's value against whether the link had
/// folded, the handoff slot against which candidate arrived first. A workload of
/// mutually-consistent ops cannot expose any of them, because there is nothing for
/// the answer to depend on. So this one is deliberately built from the shapes that
/// broke:
///
///   * two device links whose ids share an HLC seed — at most one may be live,
///     and which one cannot depend on arrival order;
///   * a revocation naming an account the device is NOT bound to — the mismatch
///     is what made the tombstone's hashed value order-dependent;
///   * a forged handoff reusing a real rotation's new-root key with a garbage
///     signature — displacement bait for the candidate map;
///   * the legitimate rotation it is trying to displace.
///
/// **When a new order-dependence bug is found, add its shape here** rather than
/// only writing a targeted regression test. The targeted test pins the instance;
/// this pins the class.
#[test]
fn the_adversarial_account_workload_converges() {
    let mut fx = Fixture::new();
    let mut alice = Account::new(10);
    let mallory = Account::new(20);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(grant_membership(&fx.admin, mallory.id, 31, fx.head.clone()));

    let mallory_device = mallory.enroll(21, 0);
    fx.push(mallory.link_op(&mallory_device, 32, fx.head.clone()));

    // Rotate FIRST, so the devices below are certified under the epoch the
    // rotation establishes. Certifying them at epoch 0 and then folding a rotation
    // to epoch 1 in the same workload would supersede them — correct behaviour,
    // but it would mask the collision property this test is here to pin.
    let base = fx.head.clone();
    let real_handoff = alice.rotate_to(14);
    let mut forged_handoff = real_handoff;
    forged_handoff.signature = [0u8; 64];

    // Two ids sharing an hlc_seed (the id's first 16 bytes).
    let mut low_id = [0u8; 32];
    low_id[..16].copy_from_slice(&[0xAA; 16]);
    let mut high_id = low_id;
    high_id[31] = 0xFF;
    let forge = |device_seed: u8, id: [u8; 32]| {
        let sk = key(device_seed);
        Device {
            id: DeviceId::from(id),
            cert: calimero_account::sign_device_cert(
                &alice.root,
                alice.id,
                DeviceId::from(id),
                &sk.public_key(),
                &KemPublicKey::from([device_seed; 32]),
                alice.epoch,
                0,
            )
            .expect("sign cert"),
            sk,
            account: alice.id,
        }
    };
    let colliding_low = forge(11, low_id);
    let colliding_high = forge(12, high_id);
    let honest = alice.enroll(13, 0);

    let ops = vec![
        // Colliding pair — only the lower id may end up live.
        alice.link_op(&colliding_low, 40, base.clone()),
        alice.link_op(&colliding_high, 41, base.clone()),
        // An honest device of the same account.
        alice.link_op(&honest, 42, base.clone()),
        // A revocation naming the WRONG account for this device.
        mallory_device.sign_op(
            50,
            base.clone(),
            OpPayload::DeviceRevoked {
                account: mallory.id,
                device: honest.id,
            },
        ),
        // The legitimate rotation...
        honest.sign_op(
            60,
            base.clone(),
            OpPayload::AccountKeysRotated {
                handoff: real_handoff,
            },
        ),
        // ...and a forged handoff reusing its new-root key, as displacement bait.
        mallory_device.sign_op(
            61,
            base.clone(),
            OpPayload::DeviceLinked {
                genesis: alice.genesis,
                chain: vec![forged_handoff],
                cert: honest.cert,
            },
        ),
    ];

    let expected = {
        let mut s = ScopeState::from_ops(&fx.log);
        for op in &ops {
            s.apply(op);
        }
        s.root()
    };

    let mut order: Vec<usize> = (0..ops.len()).collect();
    let mut checked = 0;
    permute(&mut order, 0, &mut |perm| {
        let mut s = ScopeState::from_ops(&fx.log);
        for &i in perm {
            s.apply(&ops[i]);
        }
        assert_eq!(
            s.root(),
            expected,
            "delivery order {perm:?} produced a different scope_root"
        );
        checked += 1;
    });
    assert_eq!(checked, 720, "all 6! orders should have been exercised");

    // Convergence alone is not enough — a plane that dropped every op would
    // converge too. Pin the outcome as well.
    let view = {
        let mut s = ScopeState::from_ops(&fx.log);
        for op in &ops {
            s.apply(op);
        }
        s.acl_view()
    };
    assert!(
        view.devices.contains_key(&colliding_low.id),
        "the lower colliding id must be the one left live"
    );
    assert!(
        !view.devices.contains_key(&colliding_high.id),
        "the higher colliding id must not also be live"
    );
    assert!(
        view.accounts.get(&alice.id).is_some_and(|a| a.epoch == 1),
        "the forged handoff must not have displaced the real rotation"
    );
}

/// A device's signing key must authorize as the account it speaks for.
///
/// This is the property the whole feature rests on and it was missing until last:
/// `AclView` is account-keyed, so anything resolving a signer had to map key →
/// account, and the only mapping available derived an account *from the bare key*.
/// For a device key that names somebody who does not exist, so a second device
/// received scope keys and then could not author with them — delivery resolved the
/// device, authorization did not.
///
/// Asserted here at the `AclView` level, which is what both the at-cut authorizer
/// and the governance apply gates read.
#[test]
fn a_devices_signing_key_authorizes_as_its_account() {
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(alice.link_op(&phone, 40, fx.head.clone()));

    let view = ScopeState::from_ops(&fx.log).acl_view();

    // The binding is folded, and it names the account the device speaks for.
    // Without this, nothing can map the device's key to Alice.
    let bound = view
        .devices
        .values()
        .find(|b| b.sign_pk == phone.sk.public_key())
        .expect("the folded view must expose the device's signing key");
    assert_eq!(
        bound.account, alice.id,
        "the device's signing key must resolve to the account it speaks for"
    );

    // And that account is a member, so resolving through the binding authorizes
    // while deriving an account from the bare key does not.
    assert!(
        view.is_scope_member(&alice.id),
        "precondition: the account is a member"
    );
    // The other half — that an account DERIVED from the device key is not a
    // member, which is why the binding must be consulted — is asserted in
    // `calimero-op-adapter`, where that derivation lives. It cannot be tested from
    // here without a dev-dependency cycle.
}

// ------------------------------------------------------------- revocation --

#[test]
fn revocation_stops_later_writes_but_honors_earlier_ones() {
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone = alice.enroll(11, 0);
    let laptop = alice.enroll(12, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(alice.link_op(&phone, 40, fx.head.clone()));
    fx.push(alice.link_op(&laptop, 50, fx.head.clone()));

    // A write the phone authored BEFORE the revocation.
    let before = phone.sign_op(
        60,
        fx.head.clone(),
        OpPayload::Put {
            entity: Id::new([3; 32]),
            value: vec![3],
        },
    );
    fx.push(before.clone());

    // Alice loses the phone; the laptop withdraws it. No admin needed.
    let revoke = laptop.sign_op(
        70,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: alice.id,
            device: phone.id,
        },
    );
    assert_eq!(decide(&fx.log, &revoke), Ok(()));
    fx.push(revoke);

    // The thief's later write is refused...
    let after = phone.sign_op(
        80,
        fx.head.clone(),
        OpPayload::Put {
            entity: Id::new([4; 32]),
            value: vec![4],
        },
    );
    assert_eq!(
        decide(&fx.log, &after),
        Err(Rejected::DeviceRevoked { device: phone.id })
    );

    // ...while the pre-revocation write is STILL valid when re-judged, even
    // though the log now contains the revocation. Causal honor: an op is
    // judged at its own cut, not the receiver's latest.
    assert_eq!(
        decide(&fx.log, &before),
        Ok(()),
        "a write authored before the revocation must not be invalidated by it"
    );

    // The laptop is unaffected — Alice keeps her identity and her history.
    let laptop_write = laptop.sign_op(
        90,
        fx.head.clone(),
        OpPayload::Put {
            entity: Id::new([5; 32]),
            value: vec![5],
        },
    );
    assert_eq!(decide(&fx.log, &laptop_write), Ok(()));
}

#[test]
fn a_revocation_that_arrives_before_its_link_still_wins() {
    // The ordering hazard the grow-only tombstone set exists for. If revocation
    // were a flag on the binding, folding revoke-then-link would resurrect the
    // device — and two nodes that received the pair in different orders would
    // disagree about who may write.
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    let link = alice.link_op(&phone, 40, fx.head.clone());
    let revoke = fx.admin.sign_op(
        50,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: alice.id,
            device: phone.id,
        },
    );

    let link_then_revoke = {
        let mut s = ScopeState::from_ops(&fx.log);
        s.apply(&link);
        s.apply(&revoke);
        s
    };
    let revoke_then_link = {
        let mut s = ScopeState::from_ops(&fx.log);
        s.apply(&revoke);
        s.apply(&link);
        s
    };

    assert_eq!(
        link_then_revoke.root(),
        revoke_then_link.root(),
        "revoke/link order must not change the projection"
    );
    assert!(
        !revoke_then_link.acl_view().devices.contains_key(&phone.id),
        "an early revocation must not be undone by the link it withdraws"
    );
}

#[test]
fn a_revocation_naming_the_wrong_account_still_converges() {
    // The tombstone used to record the revoked device's account, resolved as
    // `devices.get(device).map_or(payload.account, ..)` — which reads whether the
    // link had folded yet. Link-then-revoke stored the binding's account,
    // revoke-then-link stored the payload's claim, and that value was hashed into
    // governance_hash. So an op naming an account that disagreed with the binding
    // split the root purely by arrival order.
    //
    // Nothing ever read the value, so the tombstone is now a set. This test names
    // a deliberately wrong account, which is what the old code needed to diverge.
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let mallory = Account::new(20);
    let phone = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    let link = alice.link_op(&phone, 40, fx.head.clone());
    let revoke = fx.admin.sign_op(
        50,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: mallory.id, // NOT the account the device is bound to
            device: phone.id,
        },
    );

    let link_then_revoke = {
        let mut s = ScopeState::from_ops(&fx.log);
        s.apply(&link);
        s.apply(&revoke);
        s
    };
    let revoke_then_link = {
        let mut s = ScopeState::from_ops(&fx.log);
        s.apply(&revoke);
        s.apply(&link);
        s
    };

    assert_eq!(
        link_then_revoke.root(),
        revoke_then_link.root(),
        "a revocation naming the wrong account must not split the root by arrival order"
    );
    assert!(!revoke_then_link.acl_view().devices.contains_key(&phone.id));
}

#[test]
fn a_member_cannot_revoke_an_unbound_device_by_claiming_it() {
    // Self-service revocation needs a folded binding that PROVES the device
    // speaks for the author. Trusting the payload's own `account` field when no
    // binding exists made the claim unfalsifiable: any linked member could name
    // its own account beside an arbitrary unbound device id and be authorized.
    // A tombstone is terminal AND an early revocation beats the link it
    // withdraws, so that permanently spent a device id the attacker had no
    // relationship to — observe a link op, revoke at an earlier cut, done.
    let mut fx = Fixture::new();
    let mallory = Account::new(20);
    let mallory_device = mallory.enroll(21, 0);
    let victim = Account::new(10);
    let victim_phone = victim.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, mallory.id, 30, fx.head.clone()));
    fx.push(mallory.link_op(&mallory_device, 40, fx.head.clone()));

    // The victim's device id is public the moment its link op is gossiped, but
    // at THIS cut it has no binding.
    let poison = mallory_device.sign_op(
        50,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: mallory.id, // Mallory's own account — the unfalsifiable claim
            device: victim_phone.id,
        },
    );
    assert_eq!(
        decide(&fx.log, &poison),
        Err(Rejected::NotRootAdmin),
        "a self-service revocation must not authorize against a device with no binding"
    );

    // Revoking a device that IS bound to the author still works.
    let own = mallory_device.sign_op(
        51,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: mallory.id,
            device: mallory_device.id,
        },
    );
    assert_eq!(
        decide(&fx.log, &own),
        Ok(()),
        "an account must still be able to withdraw its own device without an admin"
    );

    // And an admin may still eject a device this cut has not folded a link for —
    // which is why 'no binding' is not simply a refusal.
    let by_admin = fx.admin.sign_op(
        52,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: victim.id,
            device: victim_phone.id,
        },
    );
    assert_eq!(decide(&fx.log, &by_admin), Ok(()));
}

#[test]
fn a_revoked_device_id_can_never_be_relinked() {
    // Revocation is terminal for the id, so a replica id is never reused. That
    // is what keeps the one-writer-per-replica invariant across a revoke/re-add
    // cycle; recovering a device means enrolling a fresh id.
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(alice.link_op(&phone, 40, fx.head.clone()));
    fx.push(fx.admin.sign_op(
        50,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: alice.id,
            device: phone.id,
        },
    ));

    // Even a freshly certified cert at a higher device epoch cannot revive it.
    let reissued = alice.enroll(11, 5);
    let relink = alice.link_op(&reissued, 60, fx.head.clone());
    assert_eq!(
        decide(&fx.log, &relink),
        Err(Rejected::DeviceRevoked { device: phone.id })
    );
}

// -------------------------------------------------------- key rotation ----

#[test]
fn rotating_the_root_key_withdraws_the_old_key_s_authority() {
    // A rotation has to *remove* authority, not merely add a new key beside the
    // old one — otherwise a stolen root key stays useful forever.
    let mut fx = Fixture::new();
    let mut alice = Account::new(10);
    let phone = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(alice.link_op(&phone, 40, fx.head.clone()));

    let compromised = key(10); // the epoch-0 root
    let handoff = alice.rotate_to(13);
    let rotate = phone.sign_op(
        50,
        fx.head.clone(),
        OpPayload::AccountKeysRotated { handoff },
    );
    assert_eq!(decide(&fx.log, &rotate), Ok(()));
    fx.push(rotate);

    // A certificate minted by the superseded key is refused.
    let forged = alice.enroll_signed_by(14, 0, &compromised, 0);
    let forged_link = alice.link_op(&forged, 60, fx.head.clone());
    assert_eq!(
        decide(&fx.log, &forged_link),
        Err(Rejected::CredentialSuperseded {
            signed: 0,
            current: 1
        })
    );

    // The new key works.
    let tablet = alice.enroll(15, 0);
    assert_eq!(
        decide(&fx.log, &alice.link_op(&tablet, 70, fx.head.clone())),
        Ok(())
    );
}

#[test]
fn concurrent_rotations_from_the_same_epoch_converge() {
    // Two devices holding the same root key can rotate concurrently. Both
    // handoffs are validly signed, so neither is "wrong" — but every node must
    // pick the same winner or the accounts' chains diverge.
    let mut fx = Fixture::new();
    let mut alice_a = Account::new(10);
    let mut alice_b = Account::new(10); // same genesis, same starting key
    let phone = alice_a.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice_a.id, 30, fx.head.clone()));
    fx.push(alice_a.link_op(&phone, 40, fx.head.clone()));

    let rot_x = phone.sign_op(
        50,
        fx.head.clone(),
        OpPayload::AccountKeysRotated {
            handoff: alice_a.rotate_to(20),
        },
    );
    let rot_y = phone.sign_op(
        51,
        fx.head.clone(),
        OpPayload::AccountKeysRotated {
            handoff: alice_b.rotate_to(21),
        },
    );

    let xy = {
        let mut s = ScopeState::from_ops(&fx.log);
        s.apply(&rot_x);
        s.apply(&rot_y);
        s
    };
    let yx = {
        let mut s = ScopeState::from_ops(&fx.log);
        s.apply(&rot_y);
        s.apply(&rot_x);
        s
    };
    assert_eq!(
        xy.root(),
        yx.root(),
        "concurrent same-epoch rotations must resolve identically on every node"
    );
}

// ------------------------------------------------------------ adversarial --

#[test]
fn a_device_cannot_claim_an_account_it_is_not_bound_to() {
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let mallory = Account::new(20);
    let phone = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(alice.link_op(&phone, 40, fx.head.clone()));

    // Mallory's device key signs, but the op claims Alice's account.
    let forged = Op::new(
        scope(),
        fx.head.clone(),
        Authorship {
            account: alice.id,
            device: phone.id,
            device_key: key(99).public_key(),
        },
        hlc(50),
        OpPayload::Put {
            entity: Id::new([6; 32]),
            value: vec![6],
        },
        [0u8; 32],
        [0u8; 64],
    );
    assert_eq!(
        decide(&fx.log, &forged),
        Err(Rejected::DeviceKeyStale { device: phone.id }),
        "the binding pins the key, not just the account"
    );
    assert!(
        !forged.verify(),
        "and the signature does not check out either"
    );

    // A device with no binding at all speaks for nobody — there is no implicit
    // account for a bare key.
    let stranger = mallory.enroll(21, 0);
    let unbound = stranger.sign_op(
        60,
        fx.head.clone(),
        OpPayload::Put {
            entity: Id::new([7; 32]),
            value: vec![7],
        },
    );
    assert_eq!(
        decide(&fx.log, &unbound),
        Err(Rejected::DeviceNotLinked {
            device: stranger.id
        })
    );
}

#[test]
fn a_link_must_be_signed_by_the_device_it_enrolls() {
    // Otherwise anyone who observed a certificate could replay it and mint a
    // binding on the real device's behalf.
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone = alice.enroll(11, 0);
    let laptop = alice.enroll(12, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(alice.link_op(&phone, 40, fx.head.clone()));

    // The phone tries to link the laptop using the laptop's certificate.
    let replayed = phone.sign_op(
        50,
        fx.head.clone(),
        OpPayload::DeviceLinked {
            genesis: alice.genesis,
            chain: alice.chain.clone(),
            cert: laptop.cert,
        },
    );
    assert_eq!(
        decide(&fx.log, &replayed),
        Err(Rejected::DeviceKeyStale { device: laptop.id })
    );
}

#[test]
fn a_certificate_cannot_be_replayed_onto_another_account() {
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let mallory = Account::new(20);
    let phone = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));

    // Mallory presents Alice's certificate under his own genesis.
    let stolen = phone.sign_op(
        40,
        fx.head.clone(),
        OpPayload::DeviceLinked {
            genesis: mallory.genesis,
            chain: vec![],
            cert: phone.cert,
        },
    );
    assert!(
        matches!(
            decide(&fx.log, &stolen),
            Err(Rejected::CredentialInvalid { .. })
        ),
        "a genesis that doesn't address the certificate's account must be refused"
    );
}

#[test]
fn a_stale_certificate_cannot_reinstate_a_retired_device_key() {
    // Device key rotation only means something if the retired key stops
    // working — so a re-link must strictly advance the device epoch.
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone_v0 = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(alice.link_op(&phone_v0, 40, fx.head.clone()));

    // Same device id, fresh keypair, epoch 1.
    let phone_v1 = alice.enroll(12, 1);
    let mut rotated = phone_v1;
    rotated.id = phone_v0.id;
    rotated.cert.device = phone_v0.id;
    let payload = DeviceCert::signing_payload(
        alice.id,
        phone_v0.id,
        &rotated.cert.sign_pk,
        &rotated.cert.kem_pk,
        0,
        1,
    );
    rotated.cert.signature = alice.root.sign(&payload).expect("sign").to_bytes();
    fx.push(alice.link_op(&rotated, 50, fx.head.clone()));

    // The retired key can no longer author.
    let with_old_key = phone_v0.sign_op(
        60,
        fx.head.clone(),
        OpPayload::Put {
            entity: Id::new([8; 32]),
            value: vec![8],
        },
    );
    assert_eq!(
        decide(&fx.log, &with_old_key),
        Err(Rejected::DeviceKeyStale {
            device: phone_v0.id
        })
    );

    // And replaying the epoch-0 certificate does not bring it back.
    let replay = alice.link_op(&phone_v0, 70, fx.head.clone());
    assert_eq!(
        decide(&fx.log, &replay),
        Err(Rejected::DeviceEpochNotAdvanced {
            offered: 0,
            folded: 1
        })
    );
}

#[test]
fn a_device_cannot_be_moved_between_accounts() {
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let mallory = Account::new(20);
    let phone = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(grant_membership(&fx.admin, mallory.id, 31, fx.head.clone()));
    fx.push(alice.link_op(&phone, 40, fx.head.clone()));

    // Mallory certifies a device whose id collides with Alice's bound device.
    let mut hijack = mallory.enroll(11, 1);
    hijack.id = phone.id;
    hijack.cert.device = phone.id;
    hijack.cert.account = mallory.id;
    let payload = DeviceCert::signing_payload(
        mallory.id,
        phone.id,
        &hijack.cert.sign_pk,
        &hijack.cert.kem_pk,
        0,
        1,
    );
    hijack.cert.signature = mallory.root.sign(&payload).expect("sign").to_bytes();

    let op = mallory.link_op(&hijack, 50, fx.head.clone());
    assert_eq!(
        decide(&fx.log, &op),
        Err(Rejected::DeviceAccountReassignment)
    );
}

// -------------------------------------------- the fold under attack --------

/// How many forged candidates the padding shape files into one epoch slot.
///
/// Deliberately well above `MAX_HANDOFF_CANDIDATES` (private to the projection,
/// 8 at the time of writing): the point is to overflow the slot so the cap has to
/// evict, and overshooting means tightening the cap cannot silently defang this
/// test. Raising the cap past this number would, so the two are worth keeping
/// apart by an order of magnitude.
const SLOT_PAD: u8 = 32;

/// The victim's slice of the account plane — everything a fold could write on
/// their behalf.
///
/// Compared as a whole rather than field by field, so an arm added later that
/// writes some new per-account map is covered by this test without anyone
/// remembering to extend it.
#[derive(Debug, PartialEq)]
struct AccountPlaneSlice {
    binding: Option<calimero_authz::AccountBinding>,
    devices: BTreeMap<DeviceId, calimero_authz::DeviceBinding>,
    revoked: Vec<DeviceId>,
}

fn account_plane_slice(log: &[Op], victim: AccountId) -> AccountPlaneSlice {
    let view = ScopeState::from_ops(log).acl_view();
    AccountPlaneSlice {
        binding: view.accounts.get(&victim).copied(),
        devices: view
            .devices
            .iter()
            .filter(|(_, b)| b.account == victim)
            .map(|(d, b)| (*d, *b))
            .collect(),
        revoked: view.revoked_devices.iter().copied().collect(),
    }
}

/// **No unauthorized op writes another account's plane state.**
///
/// The mechanical version of the `absorb_handoff` bug: a precondition enforced
/// in a different layer than the invariant depending on it is not a
/// precondition. `authorize` refuses every op below — but the fold is reachable
/// without it (`from_ops` and the sync convergence path both fold raw logs), so
/// any arm that trusts the authz layer for a *security* property is only as safe
/// as the next caller that folds a log directly.
///
/// So each shape is checked twice: `authorize` refuses it, AND folding it raw
/// leaves the victim's account-plane slice untouched. The second half is the one
/// that would have caught the original bug without a reviewer noticing it.
///
/// Each shape is also folded in two delivery orders. The bug being guarded
/// against was order-sensitive in the worst way — it converged, so it produced
/// no divergence to notice.
#[test]
fn no_unauthorized_op_writes_another_accounts_plane_state() {
    let mut fx = Fixture::new();

    // The victim: a member, one linked device, one legitimate root-key rotation
    // (so there is a *superseded* epoch for an attacker to try to roll back to).
    let mut victim = Account::new(10);
    let phone = victim.enroll(11, 0);
    fx.push(grant_membership(&fx.admin, victim.id, 30, fx.head.clone()));
    fx.push(victim.link_op(&phone, 40, fx.head.clone()));
    let genuine_handoff = victim.rotate_to(12);
    fx.push(phone.sign_op(
        50,
        fx.head.clone(),
        OpPayload::AccountKeysRotated {
            handoff: genuine_handoff,
        },
    ));
    // A device certified under the NEW root. The rotation supersedes any binding
    // certified by the old one, so without this the victim would hold no live
    // device and the fold would have nothing to protect.
    let post_rotation = victim.enroll(13, 1);
    fx.push(victim.link_op(&post_rotation, 55, fx.head.clone()));

    // Mallory: a member with a linked device, so her ops carry a resolvable
    // authorship. Without that she would be refused for being nobody, which
    // proves nothing about the account plane.
    let mallory = Account::new(20);
    let mallory_device = mallory.enroll(21, 0);
    fx.push(grant_membership(&fx.admin, mallory.id, 60, fx.head.clone()));
    fx.push(mallory.link_op(&mallory_device, 70, fx.head.clone()));

    let baseline = account_plane_slice(&fx.log, victim.id);
    assert!(
        baseline.binding.is_some_and(|b| b.epoch == 1),
        "precondition: the victim's own rotation took effect, so there is a \
         superseded epoch 0 to be rolled back to"
    );
    assert_eq!(
        baseline.devices.len(),
        1,
        "precondition: the victim holds exactly one live device"
    );

    // A handoff that cannot verify but only has to WIN THE SLOT: the slot is
    // capped and evicts by key order, so a ground key crowds the real rotation
    // out and the chain walk stops before reaching it.
    let mut forged = genuine_handoff;
    forged.new_root_sign_pk = calimero_primitives::identity::PublicKey::from([0u8; 32]);

    // Grouped, because the sharpest shape needs more than one op: a single forged
    // candidate is absorbed harmlessly (the walk skips what does not verify), so
    // the attack only bites once the slot is padded to its cap and the real
    // rotation is evicted by key order.
    let shapes: Vec<(&str, Vec<Op>)> = vec![
        (
            "a stranger rotates the victim's root key",
            vec![mallory_device.sign_op(
                80,
                fx.head.clone(),
                OpPayload::AccountKeysRotated { handoff: forged },
            )],
        ),
        (
            // Genuinely signed by the victim's root, just replayed by somebody
            // else. Gated on authorship rather than on validity, so this is
            // refused too — the narrower rule would be "does it verify", and
            // that rule cannot stop the forged one above.
            "a stranger replays the victim's own genuine handoff",
            vec![mallory_device.sign_op(
                90,
                fx.head.clone(),
                OpPayload::AccountKeysRotated {
                    handoff: genuine_handoff,
                },
            )],
        ),
        (
            // A genesis is public data, so naming the victim's account is free.
            // Absorption runs BEFORE the credential is verified, which is what
            // made this reachable at all.
            "a stranger carries a forged handoff on a link naming the victim",
            vec![mallory_device.sign_op(
                100,
                fx.head.clone(),
                OpPayload::DeviceLinked {
                    genesis: victim.genesis,
                    chain: vec![forged],
                    cert: phone.cert,
                },
            )],
        ),
        (
            "a stranger links their own device under the victim's genesis",
            vec![mallory_device.sign_op(
                110,
                fx.head.clone(),
                OpPayload::DeviceLinked {
                    genesis: victim.genesis,
                    chain: vec![],
                    cert: mallory_device.cert,
                },
            )],
        ),
        (
            // The full bug: fill the victim's epoch-0 slot to its cap with keys
            // that sort below the real one. Absorption keys by (new key,
            // signature) so nothing is displaced, but the cap has to evict
            // something, and it evicts by key order. The victim's rotation is
            // then unreachable and their chain freezes at the root key they
            // rotated AWAY from — permanently, on every replica, with no
            // divergence to notice.
            "a stranger pads the victim's epoch slot to the cap",
            (0..SLOT_PAD)
                .map(|i| {
                    let mut pad = genuine_handoff;
                    let mut key_bytes = [0u8; 32];
                    // Low bytes, so every one of these sorts below a real key and
                    // the eviction keeps them over the victim's rotation.
                    key_bytes[31] = i;
                    pad.new_root_sign_pk =
                        calimero_primitives::identity::PublicKey::from(key_bytes);
                    mallory_device.sign_op(
                        120 + u64::from(i),
                        fx.head.clone(),
                        OpPayload::AccountKeysRotated { handoff: pad },
                    )
                })
                .collect(),
        ),
    ];

    for (what, ops) in shapes {
        for op in &ops {
            assert!(
                decide(&fx.log, op).is_err(),
                "{what}: authorize must refuse it — if this ever starts passing, \
                 the fold assertion below is guarding a door that is no longer \
                 locked"
            );
        }

        for order in ["appended", "prepended"] {
            let mut log = fx.log.clone();
            if order == "appended" {
                log.extend(ops.iter().cloned());
            } else {
                log.splice(0..0, ops.iter().cloned());
            }
            assert_eq!(
                account_plane_slice(&log, victim.id),
                baseline,
                "{what} ({order}): folding it raw must not touch the victim's \
                 account-plane state"
            );
        }
    }
}

/// **The one documented exception: a revocation tombstone lands unconditionally,
/// so `authorize` is the only thing standing between a stranger and permanently
/// spending somebody else's device id.**
///
/// Asserted rather than left as a comment, because it is the sole arm where the
/// test above would fail, and a reader finding it absent cannot tell "safe" from
/// "untested".
///
/// It genuinely cannot be gated in the fold, and the asymmetry with
/// `AccountKeysRotated` is the point. A rotation has exactly one legitimate
/// author — the account itself — which is a property of the op, so the fold can
/// check it. A revocation has two (the account, or any root admin), and whether
/// the author is an admin is a question about the *cut*: the admin set is not
/// final mid-fold, and a streaming fold that answered it would answer
/// differently depending on how much had folded, which is a split root.
#[test]
fn a_revocation_tombstone_is_written_unconditionally_and_only_authz_stops_it() {
    let mut fx = Fixture::new();
    let victim = Account::new(10);
    let phone = victim.enroll(11, 0);
    fx.push(grant_membership(&fx.admin, victim.id, 30, fx.head.clone()));
    fx.push(victim.link_op(&phone, 40, fx.head.clone()));

    let mallory = Account::new(20);
    let mallory_device = mallory.enroll(21, 0);
    fx.push(grant_membership(&fx.admin, mallory.id, 50, fx.head.clone()));
    fx.push(mallory.link_op(&mallory_device, 60, fx.head.clone()));

    let baseline = account_plane_slice(&fx.log, victim.id);
    assert_eq!(
        baseline.devices.len(),
        1,
        "precondition: the victim's device is live"
    );

    let steal = mallory_device.sign_op(
        70,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: victim.id,
            device: phone.id,
        },
    );

    // The gate that actually holds. If this ever returns `Ok`, a stranger can
    // permanently withdraw anyone's device — the fold will not stop them.
    assert!(
        decide(&fx.log, &steal).is_err(),
        "authorize is the ONLY thing refusing a stranger's revocation"
    );

    // And the fold, given the op anyway, writes the tombstone.
    let mut log = fx.log.clone();
    log.push(steal);
    let after = account_plane_slice(&log, victim.id);
    assert!(
        after.revoked.contains(&phone.id),
        "the tombstone is unconditional by design — a revocation that folds \
         before its link must still win"
    );
    assert!(
        after.devices.is_empty(),
        "and it withdraws the binding, which is exactly why the authz gate above \
         is load-bearing rather than defence in depth"
    );
}

// ------------------------------------------------------------ convergence --

#[test]
fn the_account_plane_converges_under_every_delivery_order() {
    // The plane uses no last-writer-wins stamps — grow-only maps and monotone
    // epochs only — so this should hold structurally. It is here to catch a
    // later change that quietly introduces order-dependence.
    //
    // This workload is the WELL-FORMED one, and on its own it was not enough: it
    // missed three real order-dependence bugs (seed collisions, the tombstone's
    // hashed value, handoff displacement) because every op in it is honest and
    // agrees with every other. `the_adversarial_account_workload_converges` below
    // carries the ops that broke, and is the one to extend when a new
    // order-dependence bug is found.
    let mut fx = Fixture::new();
    let mut alice = Account::new(10);
    let bob = Account::new(20);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    fx.push(grant_membership(&fx.admin, bob.id, 31, fx.head.clone()));

    let a_phone = alice.enroll(11, 0);
    let a_laptop = alice.enroll(12, 0);
    let b_phone = bob.enroll(21, 0);

    let base = fx.head.clone();
    let mut ops = vec![
        alice.link_op(&a_phone, 40, base.clone()),
        alice.link_op(&a_laptop, 41, base.clone()),
        bob.link_op(&b_phone, 42, base.clone()),
    ];
    ops.push(a_laptop.sign_op(
        50,
        base.clone(),
        OpPayload::DeviceRevoked {
            account: alice.id,
            device: a_phone.id,
        },
    ));
    ops.push(a_laptop.sign_op(
        60,
        base.clone(),
        OpPayload::AccountKeysRotated {
            handoff: alice.rotate_to(13),
        },
    ));

    let expected = {
        let mut s = ScopeState::from_ops(&fx.log);
        for op in &ops {
            s.apply(op);
        }
        s.root()
    };

    // Every permutation of the five account-plane ops must fold identically.
    let mut order: Vec<usize> = (0..ops.len()).collect();
    let mut checked = 0;
    permute(&mut order, 0, &mut |perm| {
        let mut s = ScopeState::from_ops(&fx.log);
        for &i in perm {
            s.apply(&ops[i]);
        }
        assert_eq!(
            s.root(),
            expected,
            "delivery order {perm:?} produced a different scope_root"
        );
        checked += 1;
    });
    assert_eq!(checked, 120, "all 5! orders should have been exercised");
}

#[test]
fn a_device_link_moves_the_scope_root() {
    // If linking were hash-neutral, sync would report "converged" while two
    // nodes disagreed about which devices may author.
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone = alice.enroll(11, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    let before = fx.root();
    fx.push(alice.link_op(&phone, 40, fx.head.clone()));
    let after_link = fx.root();
    assert_ne!(before, after_link, "a device link must move the root");

    fx.push(fx.admin.sign_op(
        50,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: alice.id,
            device: phone.id,
        },
    ));
    let after_revoke = fx.root();
    assert_ne!(
        after_link, after_revoke,
        "a revocation must move the root too"
    );
    assert_ne!(
        before, after_revoke,
        "revoking must not return the root to its pre-link value — the \
         tombstone is state, and a node that never saw either op must not \
         look converged with one that saw both"
    );
}

#[test]
fn two_nodes_that_saw_the_same_ops_agree_on_who_may_write() {
    // The end-to-end property: convergence of the root implies agreement on
    // authorization, which is the reason the account plane is hashed in at all.
    let mut fx = Fixture::new();
    let alice = Account::new(10);
    let phone = alice.enroll(11, 0);
    let laptop = alice.enroll(12, 0);

    fx.push(grant_membership(&fx.admin, alice.id, 30, fx.head.clone()));
    let l1 = alice.link_op(&phone, 40, fx.head.clone());
    let l2 = alice.link_op(&laptop, 41, fx.head.clone());
    let rv = laptop.sign_op(
        50,
        fx.head.clone(),
        OpPayload::DeviceRevoked {
            account: alice.id,
            device: phone.id,
        },
    );

    // Node A receives link, link, revoke. Node B receives revoke, link, link.
    let node_a: Vec<Op> = fx
        .log
        .iter()
        .cloned()
        .chain([l1.clone(), l2.clone(), rv.clone()])
        .collect();
    let node_b: Vec<Op> = fx.log.iter().cloned().chain([rv, l2.clone(), l1]).collect();

    let view_a = ScopeState::from_ops(&node_a).acl_view();
    let view_b = ScopeState::from_ops(&node_b).acl_view();
    assert_eq!(view_a.devices, view_b.devices);
    assert_eq!(view_a.revoked_devices, view_b.revoked_devices);
    assert_eq!(
        ScopeState::from_ops(&node_a).root(),
        ScopeState::from_ops(&node_b).root()
    );

    // Both agree the laptop may write and the phone may not.
    let bindings: BTreeMap<_, _> = view_a
        .devices
        .iter()
        .map(|(d, b)| (*d, b.account))
        .collect();
    assert_eq!(bindings.get(&laptop.id), Some(&alice.id));
    assert!(!bindings.contains_key(&phone.id));
}

/// Visit every permutation of `slice`, calling `f` on each.
fn permute<T: Clone>(slice: &mut Vec<T>, k: usize, f: &mut impl FnMut(&[T])) {
    if k == slice.len() {
        f(slice);
        return;
    }
    for i in k..slice.len() {
        slice.swap(k, i);
        permute(slice, k + 1, f);
        slice.swap(k, i);
    }
}

#[test]
fn a_padded_candidate_slot_stays_bounded_and_order_independent() {
    // Keying candidates by signature stopped a forged handoff from DISPLACING a
    // real one, but it let one `(account, epoch)` slot grow without limit instead —
    // and every candidate in a slot costs an Ed25519 verification on every
    // `resolved_accounts` walk, i.e. on every projection read. So the slot is
    // trimmed to a fixed size.
    //
    // Trimming is the dangerous half: drop the WRONG candidates and two replicas
    // that saw the same ops in different orders keep different sets, resolve
    // different root keys, and split `scope_root`. Dropping the highest keys makes
    // the retained set the lowest-N of everything offered — a function of the set,
    // not the order — and that is what this asserts.
    let mut fx = Fixture::new();
    let mut victim = Account::new(10);
    let phone = victim.enroll(11, 0);
    fx.push(grant_membership(&fx.admin, victim.id, 30, fx.head.clone()));
    fx.push(victim.link_op(&phone, 40, fx.head.clone()));

    let real = victim.rotate_to(12);
    fx.push(phone.sign_op(
        50,
        fx.head.clone(),
        OpPayload::AccountKeysRotated { handoff: real },
    ));

    // Well past the cap, all at the same `(account, from_epoch)`, each a distinct
    // map key because only the signature differs. Authored by the account's own
    // device: a stranger cannot reach this slot at all (a bare rotation is refused
    // unless its author is the account, and a device-link chain absorbs only
    // entries naming its own certificate's account), so an over-full slot always
    // means the account padded it itself.
    for i in 0..32u8 {
        let mut forged = real;
        forged.signature = [i; 64];
        fx.push(phone.sign_op(
            100 + u64::from(i),
            fx.head.clone(),
            OpPayload::AccountKeysRotated { handoff: forged },
        ));
    }

    let forward = ScopeState::from_ops(&fx.log).root();

    let mut reversed = fx.log.clone();
    reversed.reverse();
    assert_eq!(
        ScopeState::from_ops(&reversed).root(),
        forward,
        "trimming a padded candidate slot must keep the retained set a function of \
         the op set, not of arrival order — otherwise the trim itself splits the root"
    );

    let mut rotated = fx.log.clone();
    rotated.rotate_left(fx.log.len() / 2);
    assert_eq!(
        ScopeState::from_ops(&rotated).root(),
        forward,
        "same set, third delivery order, same root"
    );
}

#[test]
fn a_stranger_cannot_freeze_an_account_by_padding_its_epoch_slot() {
    // The exploit that the candidate-slot CAP introduced, and the reason the
    // ownership check has to live in the fold rather than only in `authorize`.
    //
    // A slot is bounded and the bound evicts by key order. So a stranger who can
    // absorb into someone else's `(account, epoch)` slot does not merely waste
    // work — they fill it with forged candidates keyed BELOW the real rotation,
    // evict the real one, and the chain walk then finds nothing that verifies at
    // that epoch and stops. The account is frozen at the previous root key,
    // permanently and on every replica, which is strictly worse than the unbounded
    // growth the cap was added to prevent.
    //
    // `authorize` does refuse a rotation authored by another account, but the fold
    // is reachable without it — `from_ops` and the sync convergence path both fold
    // raw logs — so the gate has to be here.
    let mut fx = Fixture::new();
    let mut victim = Account::new(10);
    let phone = victim.enroll(11, 0);
    fx.push(grant_membership(&fx.admin, victim.id, 30, fx.head.clone()));
    fx.push(victim.link_op(&phone, 40, fx.head.clone()));

    // The victim genuinely rotates onto key(12).
    let real = victim.rotate_to(12);
    fx.push(phone.sign_op(
        50,
        fx.head.clone(),
        OpPayload::AccountKeysRotated { handoff: real },
    ));

    let mallory = Account::new(20);
    let mallory_device = mallory.enroll(21, 0);
    fx.push(grant_membership(&fx.admin, mallory.id, 60, fx.head.clone()));
    fx.push(mallory.link_op(&mallory_device, 70, fx.head.clone()));

    // An all-zero new-root key sorts below any real one, so every forged candidate
    // beats the real rotation in the eviction order. More than the cap, so the real
    // candidate is the one trimmed.
    // Comfortably past MAX_HANDOFF_CANDIDATES (8).
    for i in 0..12u8 {
        let mut forged = real;
        forged.new_root_sign_pk = calimero_primitives::identity::PublicKey::from([0u8; 32]);
        forged.signature = [i; 64];
        fx.push(mallory_device.sign_op(
            100 + u64::from(i),
            fx.head.clone(),
            OpPayload::AccountKeysRotated { handoff: forged },
        ));
    }

    let view = ScopeState::from_ops(&fx.log).acl_view();
    let resolved = view
        .accounts
        .get(&victim.id)
        .expect("the victim's account is still known");
    assert_eq!(
        resolved.epoch, 1,
        "a stranger's forged handoffs must not be absorbed into the victim's slot: \
         the victim's own rotation has to survive, or its chain freezes at the \
         superseded key on every replica"
    );
    assert_eq!(
        resolved.root_pk,
        key(12).public_key(),
        "the victim must resolve to the key it actually rotated onto"
    );
}
