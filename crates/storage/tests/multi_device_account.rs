//! One person, many devices — the invariant a careless account flip breaks.
//!
//! Moving authorization onto accounts is the point of the account plane: grant a
//! person once and every device they hold may write. But **per-writer state must
//! stay per device.** Counter slots, owner stamps and HLC seeds are not
//! authorization — they are the bookkeeping that keeps two concurrent writers
//! from overwriting each other. Key them by account and two devices of one person
//! share a slot, so a merge keeps one write and silently drops the other.
//!
//! That mistake compiles. `AccountId` and `PublicKey` are both 32 bytes, so a
//! blanket rename type-checks and nearly every existing test still passes — the
//! suite was written when each replica was its own principal, and that setup
//! cannot tell the two keyings apart. These tests can: every writer here is a
//! device of the *same* account, so anything keyed by account collapses to one
//! slot and the loss becomes visible.
//!
//! Every test here drives the convergence harness, and that is a constraint on
//! the file rather than a coincidence: `collections::ROOT_ID` is a process-global
//! `LazyLock` fixed by whichever context first touches a collection, so a test
//! using the plain mocked env in this same binary would fail depending on which
//! ran first. The single-store half of this property lives in
//! `collections::authored_map`'s in-crate tests for that reason.
//!
//! Run with: `cargo test -p calimero-storage --features testing --test multi_device_account`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{Counter, Mergeable};
use calimero_storage::testing::converge;
use calimero_storage::{env, rekey_field_if_supported};
use serial_test::serial;

const REPLICAS: usize = 4;

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Edits {
    count: Counter,
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

/// **N devices of one account keep N counter slots.**
///
/// Every replica increments once, concurrently, and then merges. A correct
/// G-Counter sums the per-device slots, so the total is the replica count. Key the
/// slots by account instead and all four replicas write the same slot, whose merge
/// is a max: total 1, three increments gone, and every replica converged on the
/// same wrong number — so the hash check alone would pass. That is why this
/// asserts the value, and why it needs concurrent replicas: applied one after
/// another against a single store, even a shared slot would add up to 4.
#[test]
#[serial]
fn devices_of_one_account_do_not_share_a_counter_slot() {
    converge::<Edits>()
        .replicas(REPLICAS)
        .one_account()
        .ops(|s: &mut Edits| s.count.increment().unwrap())
        .invariant("every device's increment survived", |s: &Edits| {
            s.count.value().unwrap() == REPLICAS as u64
        })
        .assert_all_replicas_equal();
}

/// **The harness really does hand every replica one account and its own device.**
///
/// Without this, the test above could be passing for the boring reason: if the
/// replicas were four separate accounts, or the gate were still device-keyed,
/// nothing about accounts would have been exercised.
#[test]
#[serial]
fn the_harness_gives_one_account_many_devices() {
    RECORDED.lock().unwrap().clear();

    converge::<Edits>()
        .replicas(REPLICAS)
        .one_account()
        // Reads the ambient env, so it records the identity each replica actually
        // writes under rather than one the test asserts against itself.
        .ops(|_: &mut Edits| {
            RECORDED
                .lock()
                .unwrap()
                .push((env::device_id(), env::account_id()));
        })
        .assert_all_replicas_equal();

    let mut devices = Vec::new();
    let mut accounts = Vec::new();
    for (device, account) in RECORDED.lock().unwrap().drain(..) {
        assert_ne!(
            device, account,
            "a device id equal to its account would let a device-keyed gate pass \
             every account-keyed assertion in this file"
        );
        devices.push(device);
        accounts.push(account);
    }

    devices.sort_unstable();
    devices.dedup();
    assert_eq!(devices.len(), REPLICAS, "every replica is its own device");

    accounts.dedup();
    assert_eq!(
        accounts.len(),
        1,
        "but all of them are one account — otherwise this file is testing four \
         strangers, which is the case that already passed before the flip"
    );
}

/// Identities observed from inside the ops, in write order. A `Mutex` rather than
/// a thread-local because the harness may run ops on a different thread than the
/// assertions.
static RECORDED: std::sync::Mutex<Vec<([u8; 32], [u8; 32])>> = std::sync::Mutex::new(Vec::new());
