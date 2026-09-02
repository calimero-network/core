//! The convergence harness must vary the executing identity at **both** layers.
//!
//! `calimero-storage`'s `RuntimeEnv` and the SDK's mock host keep separate copies
//! of "who is executing", and setting one does not set the other. The harness
//! used to set only the storage copy, so `calimero_sdk::env::device_id()` — the
//! function app code actually calls — returned the same process default on every
//! replica.
//!
//! That does not fail loudly. It makes any app rule that varies by writer
//! degenerate into one that does not, so the rule appears broken while the
//! harness is what never varied its input. It is why the `#[app::mergeable]`
//! custom-merge assertion had to be written as a two-node merobox workflow
//! instead of an in-process test.
//!
//! These tests read the identity from inside an op closure — the same vantage an
//! app method has — rather than inspecting the harness's own plumbing, because
//! the defect was precisely that the plumbing was right on one side and never
//! reached the other.
//!
//! Run with: `cargo test -p calimero-storage --features testing --test converge_identity`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::sync::Mutex;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{Counter, MergeStrategy, Mergeable};
use calimero_storage::testing::converge;
use calimero_storage::{env, rekey_field_if_supported};

const REPLICAS: usize = 3;

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Edits {
    count: Counter,
}

// Structural: a test fixture, merged by the storage layer's own rules.
#[diagnostic::do_not_recommend]
impl MergeStrategy for Edits {
    const DISPATCHED: bool = false;
}

impl Mergeable for Edits {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.count.merge(&other.count)
    }
}

impl calimero_storage::collections::rekey::RekeyTarget for Edits {
    fn rekey_relative_to(&mut self, parent_id: calimero_storage::address::Id) {
        use calimero_storage::collections::rekey::field_child_id;
        rekey_field_if_supported!(&mut self.count, field_child_id(parent_id, "count"));
    }
}

/// Identities observed from inside an op, one set per test so the tests do not
/// share collectors (the harness serializes runs, but not these statics).
struct Seen {
    storage_devices: Mutex<BTreeSet<[u8; 32]>>,
    sdk_devices: Mutex<BTreeSet<[u8; 32]>>,
    sdk_accounts: Mutex<BTreeSet<[u8; 32]>>,
}

impl Seen {
    const fn new() -> Self {
        Self {
            storage_devices: Mutex::new(BTreeSet::new()),
            sdk_devices: Mutex::new(BTreeSet::new()),
            sdk_accounts: Mutex::new(BTreeSet::new()),
        }
    }

    fn record(&self) {
        let _ = self
            .storage_devices
            .lock()
            .unwrap()
            .insert(env::device_id());
        let _ = self
            .sdk_devices
            .lock()
            .unwrap()
            .insert(calimero_sdk::env::device_id());
        let _ = self
            .sdk_accounts
            .lock()
            .unwrap()
            .insert(calimero_sdk::env::account_id());
    }

    fn counts(&self) -> (usize, usize, usize) {
        (
            self.storage_devices.lock().unwrap().len(),
            self.sdk_devices.lock().unwrap().len(),
            self.sdk_accounts.lock().unwrap().len(),
        )
    }
}

/// **An app method sees a different device on every replica.**
///
/// The SDK half is the assertion that mattered: the storage half already passed
/// before the fix, which is why the gap survived so long.
#[test]
fn both_layers_see_a_distinct_device_per_replica() {
    static SEEN: Seen = Seen::new();

    converge::<Edits>()
        .replicas(REPLICAS)
        .ops(|s: &mut Edits| {
            SEEN.record();
            s.count.increment().unwrap();
        })
        .assert_all_replicas_equal();

    let (storage, sdk, accounts) = SEEN.counts();
    assert_eq!(
        storage, REPLICAS,
        "storage layer should report one device per replica"
    );
    assert_eq!(
        sdk, REPLICAS,
        "SDK layer should report one device per replica; \
         a count of 1 means the harness never left the host default"
    );
    assert_eq!(
        accounts, REPLICAS,
        "each replica writes as its own account unless `one_account()` is set"
    );
}

/// **`one_account()` shares the person, not the device.**
///
/// The distinction the harness exists to preserve: N devices of one account must
/// keep N per-writer slots. Flattening the SDK account onto the device (or vice
/// versa) would make an account-keyed gate and a device-keyed one behave alike.
#[test]
fn one_account_shares_the_account_but_not_the_device() {
    static SEEN: Seen = Seen::new();

    converge::<Edits>()
        .replicas(REPLICAS)
        .one_account()
        .ops(|s: &mut Edits| {
            SEEN.record();
            s.count.increment().unwrap();
        })
        .assert_all_replicas_equal();

    let (storage, sdk, accounts) = SEEN.counts();
    assert_eq!(storage, REPLICAS, "devices stay distinct under one_account");
    assert_eq!(
        sdk, REPLICAS,
        "SDK devices stay distinct under one_account too"
    );
    assert_eq!(
        accounts, 1,
        "one_account means every replica writes as the same account"
    );
}

/// **The harness restores the ambient identity it borrowed.**
///
/// A run that left its last replica's device installed would silently key any
/// later test in the same binary to that replica.
#[test]
fn a_run_does_not_leak_its_last_replica_identity() {
    let before = (
        calimero_sdk::env::device_id(),
        calimero_sdk::env::account_id(),
    );

    converge::<Edits>()
        .replicas(REPLICAS)
        .ops(|s: &mut Edits| {
            s.count.increment().unwrap();
        })
        .assert_all_replicas_equal();

    let after = (
        calimero_sdk::env::device_id(),
        calimero_sdk::env::account_id(),
    );
    assert_eq!(
        before, after,
        "the SDK host identity must be restored after a converge run"
    );
}
