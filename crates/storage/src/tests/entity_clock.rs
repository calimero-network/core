//! A write follows what it overwrites, so last-write-wins must rank it later,
//! whatever the wall clock says.
//!
//! Every stamp here was read off the wall clock, and the HLC does not help: it
//! starts afresh with each execution. A clock behind a stored stamp (an NTP step
//! back, or a write received from a peer whose clock runs ahead, which the drift
//! check lets in up to 5s) therefore stamped the next write older than the one
//! it replaced. Each test steps the clock back between two writes of one value
//! and checks that the second write wins.

use crate::action::Action;
use crate::collections::{LwwRegister, UnorderedMap};
use crate::delta::{commit_causal_delta, reset_delta_context, set_current_heads};
use crate::env;
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use core::num::NonZeroU64;

const T: u64 = 1_800_000_000_000_000_000;
const THREE_SECONDS: u64 = 3_000_000_000;

/// Runs `f` as one execution, with a fresh HLC and the wall clock at `time`.
fn at<R>(time: u64, f: impl FnOnce() -> R) -> R {
    let previous = env::begin_execution_for_testing(time);
    let out = f();
    env::restore_wall_clock_for_testing(previous);
    out
}

fn kv() -> UnorderedMap<String, LwwRegister<String>> {
    env::reset_for_testing();
    reset_delta_context();
    set_current_heads(vec![[0; 32]]);
    UnorderedMap::new()
}

fn put(map: &mut UnorderedMap<String, LwwRegister<String>>, value: &str) {
    drop(
        map.insert("k".to_owned(), LwwRegister::new(value.to_owned()))
            .unwrap(),
    );
}

fn read(map: &UnorderedMap<String, LwwRegister<String>>) -> Option<String> {
    map.get(&"k".to_owned())
        .unwrap()
        .map(|value| value.get().clone())
}

/// The stamps the actions of the pending delta ship for `map`'s entry.
fn shipped_stamps(map: &UnorderedMap<String, LwwRegister<String>>) -> Vec<u64> {
    let entry = map.entry_id(&"k".to_owned());
    commit_causal_delta(&[0; 32])
        .unwrap()
        .map(|delta| delta.actions)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|action| match action {
            Action::Add { id, metadata, .. } | Action::Update { id, metadata, .. }
                if id == entry =>
            {
                Some(metadata.updated_at())
            }
            Action::DeleteRef { id, deleted_at, .. } if id == entry => Some(deleted_at),
            _ => None,
        })
        .collect()
}

#[test]
fn an_overwrite_wins_after_the_wall_clock_steps_back() {
    let mut map = kv();
    at(T + THREE_SECONDS, || put(&mut map, "first"));
    at(T, || put(&mut map, "second"));

    assert_eq!(read(&map).as_deref(), Some("second"));
}

#[test]
fn an_overwrite_ships_a_stamp_after_the_one_it_replaces() {
    let mut map = kv();
    let first = at(T + THREE_SECONDS, || {
        put(&mut map, "first");
        shipped_stamps(&map)
    });
    let second = at(T, || {
        put(&mut map, "second");
        shipped_stamps(&map)
    });

    // Every replica that holds `first` keeps the newer stamp, so this is what
    // decides whether `second` lands anywhere but here.
    assert!(
        second.last() > first.last(),
        "second write ships {second:?}, first shipped {first:?}"
    );
}

#[test]
fn a_delete_wins_after_the_wall_clock_steps_back() {
    let mut map = kv();
    at(T + THREE_SECONDS, || put(&mut map, "first"));
    let (deleted, stamps) = at(T, || {
        let _ignored = shipped_stamps(&map);
        let deleted = map.remove(&"k".to_owned()).unwrap();
        (deleted, shipped_stamps(&map))
    });

    assert!(deleted.is_some());
    assert_eq!(read(&map), None);
    // A peer refuses a delete whose stamp is older than the value it deletes.
    assert!(stamps.iter().all(|&stamp| stamp > T + THREE_SECONDS));
}

#[test]
fn a_write_after_a_delete_wins_after_the_wall_clock_steps_back() {
    let mut map = kv();
    let deleted = at(T + THREE_SECONDS, || {
        put(&mut map, "first");
        drop(map.remove(&"k".to_owned()).unwrap());
        shipped_stamps(&map)
    });
    let written = at(T, || {
        put(&mut map, "again");
        shipped_stamps(&map)
    });

    assert_eq!(read(&map).as_deref(), Some("again"));
    // A peer holding the tombstone takes the write only if it is newer.
    assert!(
        written.last() > deleted.last(),
        "write ships {written:?}, delete shipped {deleted:?}"
    );
}

#[test]
fn a_register_overwrite_wins_after_the_wall_clock_steps_back() {
    env::reset_for_testing();
    let mut register = at(T + THREE_SECONDS, || LwwRegister::new("first".to_owned()));
    let mut replica = register.clone();

    at(T, || register.set("second".to_owned()));
    replica.merge(&register);

    assert_eq!(replica.get(), "second");
}

#[test]
fn an_in_place_register_edit_wins_after_the_wall_clock_steps_back() {
    env::reset_for_testing();
    let mut register = at(T + THREE_SECONDS, || LwwRegister::new("first".to_owned()));
    let mut replica = register.clone();

    at(T, || register.value_mut().push_str(" edited"));
    replica.merge(&register);

    assert_eq!(replica.get(), "first edited");
}

fn stamp(time: u64, writer: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(time),
        ID::from(NonZeroU64::new(writer).unwrap()),
    ))
}

/// Concurrent writes, neither of which saw the other, still resolve as they
/// did: newer stamp first, then the writer id, the same on every replica.
#[test]
fn concurrent_register_writes_converge_whatever_the_merge_order() {
    for (a, b) in [
        (stamp(100, 1), stamp(200, 2)),
        (stamp(200, 1), stamp(100, 2)),
        (stamp(100, 1), stamp(100, 2)),
        (stamp(100, 2), stamp(100, 1)),
    ] {
        let alice = LwwRegister::new_with_metadata("alice".to_owned(), a);
        let bob = LwwRegister::new_with_metadata("bob".to_owned(), b);

        let mut at_alice = alice.clone();
        at_alice.merge(&bob);
        let mut at_bob = bob.clone();
        at_bob.merge(&alice);

        let expected = if a > b { "alice" } else { "bob" };
        assert_eq!(at_alice.get(), expected, "{a} vs {b}");
        assert_eq!(at_bob.get(), expected, "{a} vs {b}");
        assert_eq!(at_alice.timestamp(), at_bob.timestamp());
    }
}

/// A stamp already later than the one it replaces is kept as is: an ordinary
/// write is stamped exactly as before.
#[test]
fn a_write_on_a_clock_ahead_of_the_stored_stamp_keeps_its_stamp() {
    let later = stamp(500, 7);
    assert_eq!(later.after(stamp(400, 9)), later);
    assert_eq!(later.after(stamp(500, 9)), stamp(501, 7));
    assert_eq!(later.after(stamp(900, 1)), stamp(901, 7));
}
