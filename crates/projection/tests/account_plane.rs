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

// ------------------------------------------------------------ convergence --

#[test]
fn the_account_plane_converges_under_every_delivery_order() {
    // The plane uses no last-writer-wins stamps — grow-only maps and monotone
    // epochs only — so this should hold structurally. It is here to catch a
    // later change that quietly introduces order-dependence.
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
