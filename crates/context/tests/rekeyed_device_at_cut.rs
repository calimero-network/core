//! A device's re-key supersedes its earlier certificate, and its withdrawal ends it,
//! only at the cuts that hold them, so every replica judges a join alike.

use std::sync::Arc;

use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, KemPublicKey};
use calimero_context::scope_projection::ScopeProjections;
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use core::num::NonZeroU64;

const DEVICE_NONCE: [u8; 16] = [0x7D; 16];

struct Namespace {
    store: Store,
    ns: ContextGroupId,
    proj: ScopeProjections,
    root_sk: PrivateKey,
    genesis: AccountGenesis,
    clock: u64,
}

impl Namespace {
    fn new() -> Self {
        let root_sk = PrivateKey::from([0x5A; 32]);
        Self {
            store: Store::new(Arc::new(InMemoryDB::owned())),
            ns: ContextGroupId::from([0x51; 32]),
            proj: ScopeProjections::new(),
            genesis: AccountGenesis::new(root_sk.public_key()),
            root_sk,
            clock: 0,
        }
    }

    /// A certificate binding the account's device to `key` at `device_epoch`.
    fn cert(&self, key: &PublicKey, device_epoch: u32) -> DeviceCert {
        DeviceCert::sign(
            &self.root_sk,
            self.genesis.account_id(),
            DeviceId::mint(self.genesis.account_id(), DEVICE_NONCE),
            key,
            &KemPublicKey::from([0x2B; 32]),
            0,
            device_epoch,
        )
        .expect("sign the device cert")
    }

    /// Fold a join that binds the account's device to `key` at `device_epoch`; returns its id.
    fn join(&mut self, parent: Option<[u8; 32]>, key: &PublicKey, device_epoch: u32) -> [u8; 32] {
        let payload = OpPayload::MemberJoinedWithDevice {
            group: self.ns,
            member: self.genesis.account_id(),
            role: GroupMemberRole::Member,
            genesis: self.genesis,
            chain: vec![],
            cert: self.cert(key, device_epoch),
        };
        self.fold(parent, key, payload)
    }

    /// Fold a link of the account's device to `key` under `scope_epoch`; returns its id.
    fn link(&mut self, parent: [u8; 32], key: &PublicKey, scope_epoch: u32) -> [u8; 32] {
        let payload = OpPayload::DeviceLinked {
            genesis: self.genesis,
            chain: vec![],
            cert: self.cert(key, 0),
            scope_epoch,
        };
        self.fold(Some(parent), key, payload)
    }

    /// Fold `payload`, authored by this account's device under `key`; returns its id.
    fn fold(&mut self, parent: Option<[u8; 32]>, key: &PublicKey, payload: OpPayload) -> [u8; 32] {
        self.clock += 1;
        let op = Op::new(
            ScopeId::from(self.ns.to_bytes()),
            parent.into_iter().collect(),
            Authorship {
                account: self.genesis.account_id(),
                device: DeviceId::mint(self.genesis.account_id(), DEVICE_NONCE),
                device_key: *key,
            },
            HybridTimestamp::new(Timestamp::new(
                NTP64(self.clock),
                ID::from(NonZeroU64::new(1).unwrap()),
            )),
            payload,
            [0; 32],
            [0; 64],
        );
        self.proj.ingest_op(&op);
        op.id()
    }

    fn withdrawn(&self, cut: [u8; 32]) -> Option<Option<u32>> {
        self.proj.device_withdrawn_at_cut(
            &self.store,
            self.ns,
            &self.genesis.account_id(),
            &DeviceId::mint(self.genesis.account_id(), DEVICE_NONCE),
            &[cut],
        )
    }

    fn superseded(&self, device_epoch: u32, cut: [u8; 32]) -> Option<bool> {
        self.superseded_for(&self.genesis.account_id(), device_epoch, cut)
    }

    fn superseded_for(
        &self,
        account: &AccountId,
        device_epoch: u32,
        cut: [u8; 32],
    ) -> Option<bool> {
        self.proj.device_epoch_superseded_at_cut(
            &self.store,
            self.ns,
            account,
            &DeviceId::mint(self.genesis.account_id(), DEVICE_NONCE),
            device_epoch,
            &[cut],
        )
    }
}

#[test]
fn a_device_key_is_superseded_only_at_cuts_that_hold_its_rekey() {
    let mut n = Namespace::new();
    let old_key = PrivateKey::from([0x01; 32]).public_key();
    let new_key = PrivateKey::from([0x02; 32]).public_key();
    let first = n.join(None, &old_key, 0);
    let rekeyed = n.join(Some(first), &new_key, 1);

    assert_eq!(n.superseded(0, rekeyed), Some(true));
    assert_eq!(
        n.superseded(0, first),
        Some(false),
        "an op citing the cut before the re-key is judged by that cut"
    );
    assert_eq!(n.superseded(1, rekeyed), Some(false), "the current key");
    assert_eq!(
        n.superseded_for(&AccountId::from([0x66; 32]), 0, rekeyed),
        Some(false),
        "another account's certificate for the same device id is not this device's re-key"
    );
    assert_eq!(
        n.superseded(0, [0x99; 32]),
        None,
        "a cut this node has not folded is undecided, never a verdict"
    );
}

#[test]
fn a_device_is_withdrawn_only_at_cuts_that_hold_its_withdrawal() {
    let mut n = Namespace::new();
    let key = PrivateKey::from([0x01; 32]).public_key();
    let account = n.genesis.account_id();
    let joined = n.join(None, &key, 0);
    let revoked = n.fold(
        Some(joined),
        &key,
        OpPayload::DeviceRevoked {
            account,
            device: DeviceId::mint(account, DEVICE_NONCE),
        },
    );

    assert_eq!(n.withdrawn(joined), Some(None));
    assert_eq!(n.withdrawn(revoked), Some(Some(0)));

    let mut n = Namespace::new();
    let joined = n.join(None, &key, 0);
    let descope = |n: &mut Namespace, parent, scope_epoch| {
        n.fold(
            Some(parent),
            &key,
            OpPayload::DeviceDescoped {
                account,
                device: DeviceId::mint(account, DEVICE_NONCE),
                scope_epoch,
            },
        )
    };
    let descoped = descope(&mut n, joined, 1);

    assert_eq!(n.withdrawn(joined), Some(None));
    assert_eq!(
        n.withdrawn(descoped),
        Some(Some(0)),
        "a narrowing past every link of the device withdraws it"
    );

    let widened = n.link(descoped, &key, 2);
    assert_eq!(n.withdrawn(widened), Some(None), "a wider link lifts it");
    let descoped_again = descope(&mut n, widened, 3);
    assert_eq!(
        n.withdrawn(descoped_again),
        Some(Some(2)),
        "the widest link folded at the cut is what the rows' floor must reach"
    );
    assert_eq!(
        n.withdrawn([0x99; 32]),
        None,
        "a cut this node has not folded is undecided, never a verdict"
    );
}
