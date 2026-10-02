//! A device's re-key supersedes its earlier certificate only at the cuts that
//! hold the re-key, so every replica judges a join signed with the old key alike.

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

const DEVICE: [u8; 32] = [0x7D; 32];

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

    /// Fold a join that binds `DEVICE` to `key` at `device_epoch`; returns its id.
    fn join(&mut self, parent: Option<[u8; 32]>, key: &PublicKey, device_epoch: u32) -> [u8; 32] {
        let account: AccountId = self.genesis.account_id();
        let cert = DeviceCert::sign(
            &self.root_sk,
            account,
            DeviceId::from(DEVICE),
            key,
            &KemPublicKey::from([0x2B; 32]),
            0,
            device_epoch,
        )
        .expect("sign the device cert");
        self.clock += 1;
        let op = Op::new(
            ScopeId::from(self.ns.to_bytes()),
            parent.into_iter().collect(),
            Authorship {
                account,
                device: DeviceId::from(DEVICE),
                device_key: *key,
            },
            HybridTimestamp::new(Timestamp::new(
                NTP64(self.clock),
                ID::from(NonZeroU64::new(1).unwrap()),
            )),
            OpPayload::MemberJoinedWithDevice {
                group: self.ns,
                member: account,
                role: GroupMemberRole::Member,
                genesis: self.genesis,
                chain: vec![],
                cert,
            },
            [0; 32],
            [0; 64],
        );
        self.proj.ingest_op(&op);
        op.id()
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
            &DeviceId::from(DEVICE),
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
