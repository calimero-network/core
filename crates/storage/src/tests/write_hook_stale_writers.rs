//! Regression test for a storage-internal invariant flagged in #2265 review:
//! `Index::update_hash_for` does NOT touch `metadata.storage_type.writers`,
//! so the stored writer set stays frozen at bootstrap. The rotation-detection
//! logic in `maybe_append_rotation_log` relies on this — comparing the
//! action's claimed `writers` against the (frozen) stored writers — to avoid
//! falsely flagging stale value-writes as rotations.
//!
//! If `Index::update_hash_for` ever starts updating `storage_type`, this
//! detection logic must move to comparing against `writers_at(causal_parents)`
//! instead. This test catches that future regression at the storage layer
//! without needing a DAG harness — the apply contexts here only carry
//! `delta_id`/`delta_hlc` (no `effective_writers`) so the verifier falls
//! through to the v2 stored-writers path and the write hook still fires.
//!
//! This test was extracted from `p3_dag_causal.rs` per #2266 step 5 — the
//! rest of P3 moved to the node crate (where the DAG lives), but this case
//! is purely about a storage-internal invariant and stays here.

use core::num::NonZeroU128;

use ed25519_dalek::SigningKey;

use std::collections::BTreeSet;

use calimero_primitives::identity::PublicKey;

use crate::address::Id;
use crate::entities::{ChildInfo, Metadata};
use crate::index::Index;
use crate::interface::{ApplyContext, Interface};
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use crate::rotation_log;
use crate::store::{MockedStorage, StorageAdaptor};
use crate::tests::common::{build_signed_shared_action, pubkey_of};

type S<const SCOPE: usize> = MockedStorage<SCOPE>;

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hlc(ns: u64) -> HybridTimestamp {
    let node_id = ID::from(NonZeroU128::new(1).unwrap());
    HybridTimestamp::new(Timestamp::new(NTP64(ns), node_id))
}

fn hlc_at(step: u64) -> u64 {
    crate::env::time_now().saturating_add(step.saturating_mul(1_000_000_000))
}

fn setup_root<S: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    // Fetch the post-`add_root` full_hash so the returned `ChildInfo`'s
    // merkle_hash matches what the apply path's `verify_ancestor_integrity`
    // will read from the index. Without this, every test using `setup_root`
    // as an ancestor would fail with `TreeStateMismatch`.
    let (full_hash, _) = Index::<S>::get_hashes_for(root_id).unwrap().unwrap();
    ChildInfo::new(root_id, full_hash, root_meta)
}

fn entity_id(seed: u8) -> Id {
    Id::new([seed; 32])
}

/// Apply context with delta_id/delta_hlc populated (so the write hook fires)
/// but no `effective_writers` (verifier falls through to v2 stored-writers).
fn ctx(delta_id: [u8; 32], delta_hlc_ns: u64) -> ApplyContext {
    ApplyContext {
        effective_writers: None,
        delta_id: Some(delta_id),
        delta_hlc: Some(hlc(delta_hlc_ns)),
    }
}

/// Documents a known design fragility: `maybe_append_rotation_log` decides
/// whether to append by comparing the action's claimed `writers` against the
/// currently-stored writers. But `Index::update_hash_for` only touches
/// `own_hash`/`full_hash`/`updated_at` — it never updates
/// `metadata.storage_type`, so the stored writers stay frozen at the
/// bootstrap set forever.
///
/// As a result, a value-write that *happens* to claim the bootstrap writers
/// (e.g., authored by a peer with stale view) is correctly NOT logged as a
/// rotation, even after a real rotation has updated the writer set logically.
/// The rotation-detection logic depends on this stale-stored-writers behavior
/// to avoid false positives.
///
/// If `update_hash_for` is ever changed to also update `storage_type`, the
/// rotation-detection logic must switch to comparing against
/// `writers_at(causal_parents)` instead, or stale value-writes will be
/// falsely flagged as rotations.
#[test]
fn write_hook_relies_on_stale_stored_writers_for_rotation_detection() {
    crate::env::reset_for_testing();
    let root = setup_root::<S<408>>();

    let alice_sk = make_signing_key(0xAB);
    let bob_sk = make_signing_key(0xBB);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x48);

    // Bootstrap with {Alice, Bob}.
    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<408>>::apply_action(bootstrap, &ctx([0xE0; 32], hlc_at(0))).unwrap();

    // D1: Alice rotates Bob out → writers = {Alice}. Logged as a rotation.
    let rotation = build_signed_shared_action(
        false,
        id,
        b"v1".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    Interface::<S<408>>::apply_action(rotation, &ctx([0xE1; 32], hlc_at(1))).unwrap();
    assert_eq!(
        rotation_log::load::<S<408>>(id)
            .unwrap()
            .unwrap()
            .entries
            .len(),
        2,
        "post-D1 baseline: bootstrap + rotation"
    );

    // D2: Alice value-write claiming the BOOTSTRAP writers {Alice, Bob}.
    // She's authorized (signature verifies against the authoritative set
    // — here that's {Alice} via the v2 fallback because the verifier
    // falls through to stored writers, but since `Bob` would normally be
    // rejected, we sign with Alice). The rotation-detection compares
    // action.writers `{Alice, Bob}` against `pre_apply_writers` — which is
    // STILL `{Alice, Bob}` because `Index::update_hash_for` never updated
    // `storage_type` after D1. So `is_rotation = false` and no log entry
    // is appended.
    //
    // If the index had updated to `{Alice}` after D1, this comparison would
    // be `{Alice, Bob} != {Alice}` → IS rotation → falsely append. This
    // assertion catches that future regression.
    let value_write_with_stale_writers = build_signed_shared_action(
        false,
        id,
        b"v2".to_vec(),
        [alice, bob].into_iter().collect(), // claims bootstrap writers
        hlc_at(2),
        &alice_sk,
        vec![],
    );
    Interface::<S<408>>::apply_action(value_write_with_stale_writers, &ctx([0xE2; 32], hlc_at(2)))
        .unwrap();

    let log = rotation_log::load::<S<408>>(id).unwrap().unwrap();
    assert_eq!(
        log.entries.len(),
        2,
        "value-write with stale writer claim must NOT append \
         (relies on stored writers staying frozen at bootstrap)"
    );
}

/// Regression: two distinct writers in the same `Shared` writer set each
/// write a *different* value with the *same* nonce. Both nodes must converge
/// to the SAME value regardless of the order the two writes arrive in.
///
/// Reproduces the intermittent `shared-storage` e2e split-brain ("Wait for
/// post-rotation value to sync" — job 78652650934): after the writer set
/// rotates from node-1 to node-2, node-2's post-rotation write was assigned
/// a nonce equal to the value already stored on node-1, so node-1's
/// Shared-upsert replay guard (`new_nonce <= last_nonce`) silently dropped
/// it as an "authentic but no-op" stale action, leaving the two nodes
/// permanently diverged on the same DAG heads.
///
/// The guard's correctness comment assumes `equal nonce + valid signature ⇒
/// equal payload`. That holds for a SINGLE writer, but not across a writer
/// set: a second writer can sign different bytes with the same nonce and its
/// signature verifies too, so the equal-nonce silent-skip drops a
/// genuinely-new write.
///
/// The required invariant is **order-independent convergence**: a node that
/// applies A-then-B must end on the same value as a node that applies
/// B-then-A. (The canonical equal-timestamp tiebreak in this codebase is
/// "higher node_id wins" — see `LwwRegister::merge`.) Asserting "B always
/// wins" would be wrong: LWW with "incoming wins on equal ts" flips
/// symmetrically and still diverges.
fn apply_two_shared_writes_in_order<const SCOPE: usize>(
    first_data: &[u8],
    first_sk: &SigningKey,
    second_data: &[u8],
    second_sk: &SigningKey,
    writers: &BTreeSet<PublicKey>,
    nonce: u64,
) -> Vec<u8> {
    crate::env::reset_for_testing();
    let root = setup_root::<S<SCOPE>>();
    let id = entity_id(0x49);

    let first = build_signed_shared_action(
        true,
        id,
        first_data.to_vec(),
        writers.clone(),
        nonce,
        first_sk,
        vec![root.clone()],
    );
    Interface::<S<SCOPE>>::apply_action(first, &ctx([0xA0; 32], nonce)).unwrap();

    let second = build_signed_shared_action(
        false,
        id,
        second_data.to_vec(),
        writers.clone(),
        nonce,
        second_sk,
        vec![],
    );
    Interface::<S<SCOPE>>::apply_action(second, &ctx([0xB0; 32], nonce)).unwrap();

    Interface::<S<SCOPE>>::find_by_id_raw(id).expect("entity must exist after two writes")
}

// Regression guard for the shared-storage post-rotation split-brain
// (e2e flake job 78652650934): fixed by the equal-HLC content-hash tiebreak
// in `interface.rs` (`try_merge_non_root`'s `lww_pick`) + letting equal-nonce
// writes fall through the Shared/User replay guard instead of being skipped.
#[test]
fn shared_equal_nonce_different_writers_converge_regardless_of_order() {
    let alice_sk = make_signing_key(0xA1);
    let bob_sk = make_signing_key(0xB2);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let writers: BTreeSet<PublicKey> = [alice, bob].into_iter().collect();
    let nonce = hlc_at(0);

    // Node X applies Alice's write then Bob's (same nonce, different data).
    let x = apply_two_shared_writes_in_order::<409>(
        b"alice-value",
        &alice_sk,
        b"bob-value",
        &bob_sk,
        &writers,
        nonce,
    );
    // Node Y applies them in the opposite order.
    let y = apply_two_shared_writes_in_order::<410>(
        b"bob-value",
        &bob_sk,
        b"alice-value",
        &alice_sk,
        &writers,
        nonce,
    );

    assert_eq!(
        x, y,
        "two writers at the same nonce must converge to the SAME value \
         regardless of apply order (shared-storage post-rotation split-brain): \
         A-then-B gave {x:?}, B-then-A gave {y:?}"
    );
}
