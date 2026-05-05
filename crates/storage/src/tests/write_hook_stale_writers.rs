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
    ChildInfo::new(root_id, [0; 32], root_meta)
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
