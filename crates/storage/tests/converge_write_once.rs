//! Written-once entries converge when one account writes them from two devices.
//!
//! Keys of an owned collection are per owner, so two ACCOUNTS writing one key
//! hold two entries (`converge_owned.rs`). Two DEVICES of one account do not:
//! they write the same entry, and a device that has not yet seen the other's
//! write takes the key as fresh. Each node then holds its own device's value and
//! meets the other's as a rewrite of an immutable entry. Keeping whichever
//! arrived first leaves the nodes on different values for good, so every node
//! must settle on the same one of the two, whatever order they arrive in.
//!
//! Every replica here is a device of one account (`one_account`), and each
//! writes a value naming its device. The losing write is refused, which the
//! harness counts as a dropped action, so each run allows drops and asserts
//! convergence by root hash and by value.
//!
//! Run with: `cargo test -p calimero-storage --features testing --test converge_write_once`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{
    Frozen, IndexValue, Indexed, IndexedMap, MergeStrategy, Mergeable, ModeratedOnce, SortedMap,
    UnorderedMap, WriteOnce,
};
use calimero_storage::testing::converge;
use calimero_storage::{env, rekey_field_if_supported};
use serial_test::serial;

const REPLICAS: usize = 3;

/// Seeds for the shuffles: each is a different set of interleavings.
const SEEDS: [u64; 4] = [1, 7, 0x5EED, 0xDEAD_BEEF];

/// A value an `IndexedMap` can index.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Vote {
    choice: u64,
}

impl Indexed for Vote {
    const INDEXES: &'static [&'static str] = &["choice"];

    fn index_keys(&self, index: usize, out: &mut Vec<Vec<u8>>) {
        if index == 0 {
            self.choice.encode_index(out);
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Ballots {
    unordered: WriteOnce<UnorderedMap<String, u64>>,
    sorted: WriteOnce<SortedMap<String, u64>>,
    indexed: WriteOnce<IndexedMap<String, Vote>>,
    moderated: ModeratedOnce<UnorderedMap<String, u64>>,
    frozen: UnorderedMap<String, Frozen<u64>>,
}

// Structural: a test fixture, merged by the storage layer's own rules.
#[diagnostic::do_not_recommend]
impl MergeStrategy for Ballots {
    const DISPATCHED: bool = false;
}

impl Mergeable for Ballots {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.unordered.merge(&other.unordered)?;
        self.sorted.merge(&other.sorted)?;
        self.indexed.merge(&other.indexed)?;
        self.moderated.merge(&other.moderated)?;
        self.frozen.merge(&other.frozen)
    }
}

impl calimero_storage::collections::rekey::RekeyTarget for Ballots {
    fn rekey_relative_to(&mut self, parent_id: calimero_storage::address::Id) {
        use calimero_storage::collections::rekey::field_child_id;
        rekey_field_if_supported!(&mut self.unordered, field_child_id(parent_id, "unordered"));
        rekey_field_if_supported!(&mut self.sorted, field_child_id(parent_id, "sorted"));
        rekey_field_if_supported!(&mut self.indexed, field_child_id(parent_id, "indexed"));
        rekey_field_if_supported!(&mut self.moderated, field_child_id(parent_id, "moderated"));
        rekey_field_if_supported!(&mut self.frozen, field_child_id(parent_id, "frozen"));
    }
}

/// A value naming the device that wrote it, so two devices' writes differ.
fn mine() -> u64 {
    u64::from(env::device_id()[0])
}

/// Every device of the account casts its ballot at `k` in `write`, and every
/// replica must end holding one of them, the same one.
fn assert_one_ballot_everywhere(
    write: impl Fn(&mut Ballots) + Copy + 'static,
    held: impl Fn(&Ballots) -> Vec<u64> + Copy + 'static,
) {
    for seed in SEEDS {
        converge::<Ballots>()
            .replicas(REPLICAS)
            .one_account()
            .seed(seed)
            .allow_dropped_actions()
            .ops(write)
            .invariant("one ballot at k", move |b: &Ballots| held(b).len() == 1)
            .assert_all_replicas_equal();
    }
}

#[test]
#[serial]
#[ignore = "fixed by the next commit: apply keeps whichever write arrived first"]
fn devices_of_one_account_settle_a_write_once_unordered_map_key() {
    assert_one_ballot_everywhere(
        |b| b.unordered.insert("k".to_owned(), mine()).unwrap(),
        |b| b.unordered.entries().unwrap().map(|(_, v)| v).collect(),
    );
}

#[test]
#[serial]
#[ignore = "fixed by the next commit: apply keeps whichever write arrived first"]
fn devices_of_one_account_settle_a_write_once_sorted_map_key() {
    assert_one_ballot_everywhere(
        |b| b.sorted.insert("k".to_owned(), mine()).unwrap(),
        |b| b.sorted.entries().unwrap().map(|(_, v)| v).collect(),
    );
}

#[test]
#[serial]
#[ignore = "fixed by the next commit: apply keeps whichever write arrived first"]
fn devices_of_one_account_settle_a_write_once_indexed_map_key() {
    assert_one_ballot_everywhere(
        |b| {
            b.indexed
                .insert("k".to_owned(), Vote { choice: mine() })
                .unwrap();
        },
        |b| {
            let choices: Vec<u64> = b
                .indexed
                .entries()
                .unwrap()
                .map(|(_, v)| v.choice)
                .collect();
            // The index must hold exactly the stored value's row.
            let indexed = choices
                .iter()
                .map(|c| b.indexed.query("choice").eq(c).count().unwrap())
                .sum::<usize>();
            if indexed == choices.len() {
                choices
            } else {
                Vec::new()
            }
        },
    );
}

#[test]
#[serial]
#[ignore = "fixed by the next commit: apply keeps whichever write arrived first"]
fn devices_of_one_account_settle_a_moderated_once_key() {
    assert_one_ballot_everywhere(
        |b| b.moderated.insert("k".to_owned(), mine()).unwrap(),
        |b| b.moderated.entries().unwrap().map(|(_, v)| v).collect(),
    );
}

#[test]
#[serial]
fn devices_of_one_account_settle_a_frozen_value_at_one_key() {
    assert_one_ballot_everywhere(
        |b| {
            let _ = b
                .frozen
                .insert("k".to_owned(), Frozen::new(mine()))
                .unwrap();
        },
        |b| {
            b.frozen
                .entries()
                .unwrap()
                .map(|(_, v)| *v.get().unwrap())
                .collect()
        },
    );
}
