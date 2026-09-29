//! An `#[app::mergeable]` value in a SIGNED entry converges on its own merge
//! rule however two concurrent writes of it arrive.
//!
//! The rule here keeps the LOWER value, so it disagrees with last-writer-wins:
//! the earlier write (lower nonce) is the one that must survive. The apply path
//! used to drop a signed write whose nonce was below the stored one before the
//! merge could see it, so the replica that received the newer write first kept
//! it and the other kept the merge: two values, two roots.
//!
//! Each test has two devices write the same entry concurrently, then replays
//! both delivery orders on a fresh replica and asserts one root and the merged
//! value.
//!
//! Run with: `cargo test -p calimero-storage --features testing --test converge_signed_mergeable`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::rekey::register_rekey_cascade;
use calimero_storage::collections::{AuthoredMap, Mergeable, SharedStorage, UnorderedMap};
use calimero_storage::env;
use calimero_storage::testing::Script;
use serial_test::serial;

/// A plain `u64` the app merges by keeping the lower one.
#[app::mergeable]
#[derive(BorshSerialize, BorshDeserialize, Default, Debug, PartialEq)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Low {
    value: u64,
}

impl Mergeable for Low {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.value = self.value.min(other.value);
        Ok(())
    }
}

fn lot() -> String {
    "lot".to_owned()
}

/// The earlier write is the lower value, so a replica that kept the newer
/// write instead of merging reads `9` and not `3`.
const EARLIER: u64 = 3;
const LATER: u64 = 9;

type Cell = SharedStorage<UnorderedMap<String, Low>>;

/// `SharedMember`: a map entry inside a writer-set cell, written by two devices
/// of the one writer.
#[test]
#[serial]
fn a_mergeable_value_in_a_shared_member_entry_converges_in_every_order() {
    let mut script = Script::new(|| {
        let founder = env::account_id().into();
        Cell::new_with_field_name("bids", BTreeSet::from([founder]), false)
    });
    register_rekey_cascade::<Low>();
    let (desk, phone) = (script.founder(), script.founder());

    let bid = |value| {
        move |cell: &mut Cell| {
            let _prev = cell
                .get_mut()
                .unwrap()
                .insert(lot(), Low { value })
                .unwrap();
        }
    };
    let earlier = script.run(desk, bid(EARLIER)).unwrap();
    let later = script.run(phone, bid(LATER)).unwrap();

    let read = |cell: &Cell| cell.get().unwrap().get(&lot()).unwrap().map(|v| v.value);
    let orders = script.assert_every_order_converges(|cell| read(cell) == Some(EARLIER));
    assert_eq!(orders, 2);

    // Re-delivery, the older write included, is harmless: a replay moves
    // neither the value nor the root.
    for (replica, deltas) in [
        (desk, [later, earlier, later]),
        (phone, [earlier, earlier, later]),
    ] {
        for delta in deltas {
            assert_eq!(script.deliver(replica, delta), 0);
        }
    }
    assert_eq!(script.view(desk, read), Some(EARLIER));
    assert_eq!(script.view(phone, read), Some(EARLIER));
    assert_eq!(
        script.view(desk, |_| env::root_hash()),
        script.view(phone, |_| env::root_hash())
    );
}

/// `User`: an owned entry, written by two devices of its owner. Two different
/// owners write two different entries, so only one account's devices share one.
#[test]
#[serial]
fn a_mergeable_value_in_a_user_entry_converges_in_every_order() {
    let mut script = Script::new(AuthoredMap::<String, Low>::new);
    register_rekey_cascade::<Low>();
    let (desk, phone) = (script.founder(), script.founder());

    let bid = |value| {
        move |map: &mut AuthoredMap<String, Low>| {
            map.insert(lot(), Low { value }).unwrap();
        }
    };
    let _earlier = script.run(desk, bid(EARLIER)).unwrap();
    let _later = script.run(phone, bid(LATER)).unwrap();

    let founder = script.account(desk);
    let orders = script.assert_every_order_converges(|map| {
        map.get_by(&founder, &lot()).unwrap() == Some(Low { value: EARLIER })
    });
    assert_eq!(orders, 2);
}
