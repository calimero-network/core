//! Two devices of ONE account writing one `SharedStorage` cell — core#3965.
//!
//! The issue reported this as a `calimero-storage` CRDT bug: every replica held
//! the same value, and the root hashes still differed. It was not one. The
//! convergence harness had no signing identity, so every `SharedMember` delta it
//! exchanged failed signature verification — and a verification failure is
//! *dropped* by the sync merge (`apply_child_action_lenient`), not raised. Each
//! replica therefore kept only its own local write: individually valid, so the
//! value invariant passed, and the roots differed because nothing had merged.
//!
//! That shape — values agree, hashes don't — is exactly what a real CRDT
//! divergence looks like, which is why the misreading was a reasonable one. The
//! two are told apart by asking whether anything was applied at all, which the
//! harness now asserts (a dropped action fails the run, naming the cause).
//!
//! What these tests hold down, now that the harness signs:
//!
//!   1. the reported case converges, and the writes were really exchanged;
//!   2. the control rows from the issue still behave (one device; a writer set
//!      that covers nobody), so a future regression can be placed.
//!
//! The guard has no direct test here: forcing a delta to be refused needs
//! crate-internal state (every write these tests can make from outside is either
//! authorized or rejected before a delta exists), and a test that passes by
//! doing nothing would be worse than none. What holds it down instead is the
//! first test above — before the harness could sign, it failed.
//!
//! What this does NOT cover: the causal cut. The harness resolves writers from
//! settled local state, never from the rotation log at a delta's parents, so
//! rotation ORDERING lives in merobox — see
//! `apps/scaffolding-e2e/workflows/shared-storage-account-writers-concurrent.yml`
//! for the same two-devices-of-one-account story over the real wire, where the
//! nodes sign for themselves.
//!
//! Run with:
//! `cargo test -p calimero-storage --features testing --test shared_two_devices_one_account`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use calimero_storage::collections::{LwwRegister, SharedStorage};
use calimero_storage::env;
use calimero_storage::testing::converge_with;
use serial_test::serial;

type Cell = SharedStorage<LwwRegister<String>>;

/// The executor's ACCOUNT is the sole writer — what an app's `init` does.
///
/// Read at genesis, so under `one_account()` this grants the one account every
/// replica writes as, and without it the account only genesis holds.
fn build() -> Cell {
    SharedStorage::new(BTreeSet::from([env::account_id().into()]), false)
}

/// **The reported case: two devices of one account, writing the same cell.**
///
/// Both replicas are writers (the grant names their shared account), so both
/// writes are authorized and genuinely concurrent — the first configuration in
/// which two signed writes to one `SharedStorage` cell actually race. The
/// harness's own drop guard carries the other half of this: it fails the run if
/// either delta was refused, so a green result means the two really did
/// exchange writes rather than both sitting on their own.
#[test]
#[serial]
fn two_devices_of_one_account_writing_the_same_value() {
    converge_with(build)
        .replicas(2)
        .one_account()
        .ops(|c: &mut Cell| {
            let _previous = c.insert(LwwRegister::new("v".to_owned())).unwrap();
        })
        .invariant("every replica holds the write", |c: &Cell| {
            c.get().is_ok_and(|r| r.get() == "v")
        })
        .assert_all_replicas_equal();
}

/// **Two devices writing DIFFERENT values still converge.**
///
/// The test above cannot separate "merged" from "never exchanged anything":
/// with one value, a replica that dropped its peer's delta holds the right
/// string anyway. Distinct values per device make the merge observable — both
/// replicas must land on the SAME one, chosen by the register's rule rather
/// than by which replica is asking.
#[test]
#[serial]
fn two_devices_of_one_account_writing_different_values() {
    converge_with(build)
        .replicas(2)
        .one_account()
        .ops(|c: &mut Cell| {
            // Distinct per device: `device_id` is the replica.
            let mine = format!("v-{}", hex::encode(env::device_id()));
            let _previous = c.insert(LwwRegister::new(mine)).unwrap();
        })
        .invariant("the surviving write is one of the two", |c: &Cell| {
            c.get().is_ok_and(|r| r.get().starts_with("v-"))
        })
        .assert_all_replicas_equal();
}

/// **One device writing.** The issue's control row: nothing concurrent, so
/// nothing to merge. Kept so a future failure can be placed — if this one goes
/// red too, the fault is not in concurrent merge.
#[test]
#[serial]
fn a_single_device_writing() {
    converge_with(build)
        .replicas(1)
        .one_account()
        .ops(|c: &mut Cell| {
            let _previous = c.insert(LwwRegister::new("v".to_owned())).unwrap();
        })
        .invariant("the replica holds the write", |c: &Cell| {
            c.get().is_ok_and(|r| r.get() == "v")
        })
        .assert_all_replicas_equal();
}

/// **The issue's "two distinct accounts" row passed VACUOUSLY, and still does.**
///
/// Without `one_account()` the writer set names only the genesis account, so no
/// replica is a writer and every `insert` is refused at the API — before any
/// delta exists. Nothing is written anywhere, so the replicas agree on the
/// genesis state. That row was evidence of nothing, and is recorded here as
/// such: the assertion is that the write is REFUSED, not that it converged.
///
/// This is also why the bug needed `one_account()` to surface at all — it is the
/// only setup in which two writes to one guarded cell are both authorized.
#[test]
#[serial]
fn distinct_accounts_are_not_writers_at_all() {
    converge_with(build)
        .replicas(2)
        .ops(|c: &mut Cell| {
            assert!(
                c.insert(LwwRegister::new("v".to_owned())).is_err(),
                "a replica whose account is not in the writer set must be refused \
                 locally; if this starts succeeding, the row below stops being vacuous \
                 and this test needs rewriting rather than deleting"
            );
        })
        .invariant("the genesis value is untouched", |c: &Cell| {
            c.get().is_ok_and(|r| r.get().is_empty())
        })
        .assert_all_replicas_equal();
}
