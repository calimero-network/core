//! Two TEE authorities creating the same `TeeOnly` collection concurrently.
//!
//! A `TeeOnly` cell stores nothing until the TEE authority's first write, which
//! creates the cell and its value together. With two TEE authorities in a
//! namespace, which failover of a TEE trigger produces, both can make that
//! first write before either has seen the other's. When the value is a
//! collection, both geneses must name the same collection, or the entries one
//! TEE wrote land under a collection the merge then drops.
//!
//! Run with:
//! `cargo test -p calimero-storage --features testing --test tee_only_concurrent_genesis`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use calimero_storage::collections::{LwwRegister, TeeOnly, UnorderedMap};
use calimero_storage::env;
use calimero_storage::testing::converge_with;
use serial_test::serial;

type Hands = TeeOnly<UnorderedMap<String, LwwRegister<u8>>>;

/// Each TEE deals one card, keyed by the device that dealt it, into a cell no
/// replica has created yet.
fn deal(hands: &mut Hands) {
    let dealer = hex::encode(env::device_id());
    let _previous = hands
        .get_mut()
        .unwrap()
        .insert(dealer, LwwRegister::new(1))
        .unwrap();
}

#[test]
#[serial]
fn two_tees_creating_one_tee_only_map_keep_both_entries() {
    for replicas in [2, 3] {
        converge_with(Hands::new_tee_only)
            .replicas(replicas)
            .tee_authority()
            .ops(deal)
            .invariant(
                "every TEE's entry survives the merge",
                move |hands: &Hands| {
                    hands
                        .try_get()
                        .unwrap()
                        .is_some_and(|map| map.len().unwrap() == replicas)
                },
            )
            .assert_all_replicas_equal();
    }
}
