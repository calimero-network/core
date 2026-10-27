//! The key-arrival retry loop: how many rounds it runs, and what follows a failed one.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_op::Op;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database, InMemoryDB};
use calimero_store::iter::Iter;
use calimero_store::slice::Slice;
use calimero_store::tx::Transaction;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::NamespaceGovernance;
use crate::authorizer::{AtCutAuthorizer, AtCutMembershipPath};
use crate::void_ledger::VoidLedger;
use crate::void_tests::{World, NS};
use crate::GroupKeyring;

/// An in-memory backend whose `Group` column refuses reads once `armed` is set.
#[derive(Debug)]
struct GroupReadsFail<D> {
    inner: D,
    armed: Arc<AtomicBool>,
}

impl<'a, D: Database<'a>> Database<'a> for GroupReadsFail<D> {
    fn open(_config: &StoreConfig) -> EyreResult<Self> {
        unimplemented!("test-only backend is constructed directly")
    }

    fn has(&self, col: Column, key: Slice<'_>) -> EyreResult<bool> {
        self.inner.has(col, key)
    }

    fn get(&self, col: Column, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>> {
        if col == Column::Group && self.armed.load(Ordering::SeqCst) {
            eyre::bail!("injected group read failure");
        }
        self.inner.get(col, key)
    }

    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> EyreResult<()> {
        self.inner.put(col, key, value)
    }

    fn delete(&self, col: Column, key: Slice<'_>) -> EyreResult<()> {
        self.inner.delete(col, key)
    }

    fn iter(&self, col: Column) -> EyreResult<Iter<'_>> {
        self.inner.iter(col)
    }

    fn apply(&self, tx: &Transaction<'a>) -> EyreResult<()> {
        self.inner.apply(tx)
    }
}

/// [`crate::test_fixtures::test_store`] on a [`GroupReadsFail`] backend.
fn failing_store(armed: &Arc<AtomicBool>) -> Store {
    let store = Store::new(Arc::new(GroupReadsFail {
        inner: InMemoryDB::owned(),
        armed: Arc::clone(armed),
    }));
    let _ = crate::NodeDeviceRepository::new(&store)
        .provision_account_root()
        .expect("account root");
    store
}

/// Answers every gate live and counts what the loop asks of it. Once the namespace's
/// undecided parked ops fall to `fail_at`, it arms the store and settles a stand-in per fold.
struct Probe<'s> {
    store: &'s Store,
    forgets: AtomicUsize,
    reconciles: AtomicUsize,
    fail_at: Option<(usize, Arc<AtomicBool>)>,
    stand_ins: Mutex<Vec<[u8; 32]>>,
    voided: BTreeSet<[u8; 32]>,
}

impl<'s> Probe<'s> {
    fn new(store: &'s Store) -> Self {
        Self {
            store,
            forgets: AtomicUsize::new(0),
            reconciles: AtomicUsize::new(0),
            fail_at: None,
            stand_ins: Mutex::default(),
            voided: BTreeSet::new(),
        }
    }

    fn ledger(&self) -> VoidLedger<'s> {
        VoidLedger::new(self.store, NS.into())
    }

    fn forgets(&self) -> usize {
        self.forgets.load(Ordering::SeqCst)
    }

    fn reconciles(&self) -> usize {
        self.reconciles.load(Ordering::SeqCst)
    }

    fn retry(&self, depth: u8) -> EyreResult<Option<crate::DivergenceReport>> {
        NamespaceGovernance::new(self.store, NS.into())
            .with_apply_auth(&[], self)
            .replay(NS, depth)
            .map(|replay| replay.divergence)
    }
}

impl AtCutAuthorizer for Probe<'_> {
    fn is_admin_at_cut(&self, _: &ContextGroupId, _: &PublicKey, _: &[[u8; 32]]) -> Option<bool> {
        None
    }
    fn is_admin_or_capability_at_cut(
        &self,
        _: &ContextGroupId,
        _: &PublicKey,
        _: u32,
        _: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }
    fn is_admin_or_capability_account_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: u32,
        _: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }
    fn is_admin_account_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }
    fn is_last_admin_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }
    fn membership_path_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: &[[u8; 32]],
    ) -> Option<AtCutMembershipPath> {
        None
    }
    fn effective_role_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: &[[u8; 32]],
    ) -> Option<Option<GroupMemberRole>> {
        None
    }
    fn context_rotation_group_at_cut(
        &self,
        _: &ContextGroupId,
        _: &ContextId,
        _: &[[u8; 32]],
    ) -> Option<Result<Option<ContextGroupId>, crate::Undecidable>> {
        None
    }

    fn forget(&self) {
        let _ = self.forgets.fetch_add(1, Ordering::SeqCst);
        let Some((at, armed)) = &self.fail_at else {
            return;
        };
        if !armed.load(Ordering::SeqCst) && self.ledger().undecided_parked().unwrap() <= *at {
            armed.store(true, Ordering::SeqCst);
        }
        if armed.load(Ordering::SeqCst) {
            if let Some(op) = self.stand_ins.lock().unwrap().pop() {
                let _ = self.ledger().settle_parked(op, true).unwrap();
            }
        }
    }

    fn voided_ops(
        &self,
        _: &ContextGroupId,
        applied: Option<&Op>,
        _: &[([u8; 32], ContextGroupId)],
    ) -> Option<Result<BTreeSet<[u8; 32]>, crate::Undecidable>> {
        if applied.is_some() {
            return None;
        }
        let _ = self.reconciles.fetch_add(1, Ordering::SeqCst);
        Some(Ok(self.voided.clone()))
    }
}

/// Parks ops no replay will ever judge, so they count as undecided until settled by hand.
fn park_stand_ins(store: &Store, count: u8) -> Vec<[u8; 32]> {
    let ledger = VoidLedger::new(store, NS.into());
    (0..count)
        .map(|i| {
            let op = [0xE0 + i; 32];
            ledger.note_parked(op).unwrap();
            op
        })
        .collect()
}

/// A keyless replica of a founded namespace on `store`, its three ops parked, then the key.
fn keyed_after_parking(store: Store) -> World {
    let w = World::new();
    let replica = w.replica_on(store, false);
    for op in &w.founded() {
        replica.apply(op).expect("parked until the key arrives");
    }
    let _ = GroupKeyring::new(&replica.store, ContextGroupId::from(NS))
        .store_key(&replica.k0)
        .expect("the key arrives");
    replica
}

#[test]
fn a_round_that_settles_no_parked_op_is_the_last() {
    let w = World::new();
    let one_round = Probe::new(&w.store);
    let _ = one_round.retry(0).expect("nothing parked: one round");

    let _ = park_stand_ins(&w.store, 3);
    let probe = Probe::new(&w.store);
    let _ = probe.retry(0).expect("retry");

    assert_eq!(
        probe.forgets(),
        one_round.forgets(),
        "three undecided ops allow three rounds, but the first settled none"
    );
}

#[test]
fn a_reentrant_retry_joins_the_running_one_and_does_not_rebuild() {
    let replica = keyed_after_parking(crate::test_fixtures::test_store());
    let probe = Probe::new(&replica.store);
    let _ = probe.retry(1).expect("retry");

    assert_eq!(probe.forgets(), 0, "no round of its own");
    assert_eq!(probe.reconciles(), 0, "no rebuild below depth zero");
    assert_eq!(
        probe.ledger().undecided_parked().unwrap(),
        3,
        "left to the outer replay"
    );

    let _ = probe.retry(0).expect("retry");
    assert_eq!(probe.reconciles(), 1, "depth zero rebuilds what it applied");
}

#[test]
fn a_failed_round_after_a_productive_one_still_rebuilds_and_returns_its_error() {
    // With a void op the rebuild reads the op log too, which the armed store refuses.
    for voided in [BTreeSet::new(), BTreeSet::from([[0xEE; 32]])] {
        let armed = Arc::new(AtomicBool::new(false));
        let replica = keyed_after_parking(failing_store(&armed));
        let stand_ins = park_stand_ins(&replica.store, 6);
        let probe = Probe {
            fail_at: Some((stand_ins.len(), Arc::clone(&armed))),
            stand_ins: Mutex::new(stand_ins),
            voided: voided.clone(),
            ..Probe::new(&replica.store)
        };

        let err = probe.retry(0).expect_err("the second round fails");

        assert!(
            err.to_string().ends_with("injected group read failure"),
            "{err:#} (voided: {voided:?})"
        );
        assert_eq!(
            probe.reconciles(),
            1,
            "what the first round applied is rebuilt (voided: {voided:?})"
        );
        assert!(
            probe.ledger().undecided_parked().unwrap() > 0,
            "no round runs after the failed one, though ops were still settling (voided: {voided:?})"
        );
    }
}
