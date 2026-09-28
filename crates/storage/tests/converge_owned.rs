//! Owned collections converge however their writes interleave.
//!
//! Every replica writes the same key of an owned collection as its own account,
//! and every replica applies the others' deltas in its own shuffled order. Keys
//! are per owner, so each replica's write is its own entry: every replica must
//! end holding every replica's entry, with one root hash, for any order. When an
//! owned entry's id came from its key alone, this is exactly the run that split:
//! each replica kept whichever claim it applied first.
//!
//! Run with: `cargo test -p calimero-storage --features testing --test converge_owned`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{
    Authored, LwwRegister, MergeStrategy, Mergeable, SortedMap, UnorderedMap, UserStorage,
};
use calimero_storage::testing::converge;
use calimero_storage::{env, rekey_field_if_supported};
use serial_test::serial;

const REPLICAS: usize = 3;

/// Seeds for the shuffles: each is a different set of interleavings.
const SEEDS: [u64; 4] = [1, 7, 0x5EED, 0xDEAD_BEEF];

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Board {
    posts: Authored<UnorderedMap<String, u64>>,
    ordered: Authored<SortedMap<String, u64>>,
    profiles: UserStorage<LwwRegister<u64>>,
}

// Structural: a test fixture, merged by the storage layer's own rules.
#[diagnostic::do_not_recommend]
impl MergeStrategy for Board {
    const DISPATCHED: bool = false;
}

impl Mergeable for Board {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.posts.merge(&other.posts)?;
        self.ordered.merge(&other.ordered)?;
        self.profiles.merge(&other.profiles)
    }
}

impl calimero_storage::collections::rekey::RekeyTarget for Board {
    fn rekey_relative_to(&mut self, parent_id: calimero_storage::address::Id) {
        use calimero_storage::collections::rekey::field_child_id;
        rekey_field_if_supported!(&mut self.posts, field_child_id(parent_id, "posts"));
        rekey_field_if_supported!(&mut self.ordered, field_child_id(parent_id, "ordered"));
        rekey_field_if_supported!(&mut self.profiles, field_child_id(parent_id, "profiles"));
    }
}

/// A value that names its writer, so a replica holding someone else's entry
/// in place of its own is visible.
fn mine() -> u64 {
    u64::from(env::account_id()[0])
}

/// Whether `entries` holds one entry at `key` per replica, each its owner's.
fn one_each(entries: &[(calimero_account::AccountId, String, u64)], key: &str) -> bool {
    let owners: BTreeSet<_> = entries
        .iter()
        .filter(|(owner, k, value)| k == key && u64::from(owner.as_bytes()[0]) == *value)
        .map(|(owner, _, _)| *owner)
        .collect();
    owners.len() == REPLICAS && entries.iter().filter(|(_, k, _)| k == key).count() == REPLICAS
}

#[test]
#[serial]
fn every_replica_holds_every_owner_s_entry_at_a_contested_key() {
    for seed in SEEDS {
        converge::<Board>()
            .replicas(REPLICAS)
            .seed(seed)
            .ops(|b: &mut Board| b.posts.insert("k".to_owned(), mine()).unwrap())
            .ops(|b: &mut Board| b.ordered.insert("k".to_owned(), mine()).unwrap())
            .ops(|b: &mut Board| {
                let _ = b.profiles.insert(LwwRegister::new(mine())).unwrap();
            })
            // Shuffled, so on some replicas this runs before the insert and
            // finds nothing of the caller's to update.
            .ops(|b: &mut Board| {
                let _ = b.posts.update(&"k".to_owned(), mine() + 1000);
            })
            .invariant("one posts entry per owner at k", |b: &Board| {
                let entries: Vec<_> = b
                    .posts
                    .entries_with_owners()
                    .unwrap()
                    .into_iter()
                    .map(|(o, k, v)| (o, k, if v >= 1000 { v - 1000 } else { v }))
                    .collect();
                one_each(&entries, "k") && b.posts.len().unwrap() == REPLICAS
            })
            .invariant("one ordered entry per owner at k", |b: &Board| {
                one_each(&b.ordered.entries_with_owners().unwrap(), "k")
                    && b.ordered.keys().unwrap().count() == REPLICAS
            })
            .invariant("one slot per account", |b: &Board| {
                b.profiles.entries().unwrap().count() == REPLICAS
            })
            .assert_all_replicas_equal();
    }
}
