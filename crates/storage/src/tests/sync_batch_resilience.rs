//! Regression for the core#2716 split-brain: a single rejected action in a
//! sync-merge batch must NOT abort the whole batch.
//!
//! [`Root::sync`] runs inside the app's `__calimero_sync_next` export, whose
//! host-side caller does `.expect("fatal: sync failed")`. So if applying ONE
//! action in a [`StorageDelta::CausalActions`] batch returns `Err`, that error
//! becomes a fatal guest panic that aborts the ENTIRE delta — every sibling
//! action is lost and the node re-attempts the same delta forever.
//!
//! In the failing run (CI 26999653188) a `Shared` action arrived **unsigned**
//! (`signature_data: None`) during a concurrent-writer-rotation window. Its
//! `apply_action` returned `InvalidData("Remote Shared action must be signed")`,
//! which panic-looped every receiver (750+ times) — they never applied the
//! peer's branch and never converged, while the branch's author sat at a
//! divergent root.
//!
//! The fix (`crate::collections::root`'s `apply_child_action_lenient`) drops a
//! per-action *verification rejection* (unsigned / forged / stale /
//! unauthorized — never authoritative) and lets the batch continue. The
//! per-action `apply_action` contract is unchanged (it still returns the same
//! `Err` — the security tests rely on that); only the batch wrapper stops
//! treating it as fatal.

use core::num::NonZeroU128;
use std::collections::{BTreeMap, BTreeSet};

use borsh::to_vec;
use calimero_primitives::identity::PublicKey;
use ed25519_dalek::SigningKey;

use crate::action::Action;
use crate::address::Id;
use crate::collections::Root;
use crate::delta::StorageDelta;
use crate::entities::{ChildInfo, Metadata, OpMask, StorageType};
use crate::index::Index;
use crate::interface::{ApplyContext, Interface};
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use crate::store::MockedStorage;
use crate::tests::common::{build_signed_shared_action, pubkey_of, EmptyData};
use crate::{env, merge};

type S = MockedStorage<4716>;

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hlc(ns: u64) -> HybridTimestamp {
    let node_id = ID::from(NonZeroU128::new(1).unwrap());
    HybridTimestamp::new(Timestamp::new(NTP64(ns), node_id))
}

fn entity_id(seed: u8) -> Id {
    Id::new([seed; 32])
}

/// Register the root state and seed the root index entry, returning the root
/// `ChildInfo` to use as the `ancestors` of the child actions (so they pass the
/// v2 `verify_ancestor_integrity` check at apply time).
fn setup() -> ChildInfo {
    env::reset_for_testing();
    merge::register_crdt_merge::<EmptyData>();

    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    let (full_hash, _) = Index::<S>::get_hashes_for(root_id).unwrap().unwrap();
    ChildInfo::new(root_id, full_hash, root_meta)
}

/// A `Shared` `Add` action carrying NO signature (`signature_data: None`) — the
/// exact shape that triggered the #2716 fatal `InvalidData` on apply. Built by
/// hand because the signing helpers always attach a signature.
fn unsigned_shared_add(
    id: Id,
    writers: BTreeSet<PublicKey>,
    hlc_ns: u64,
    root: &ChildInfo,
) -> Action {
    Action::Add {
        id,
        data: b"unsigned".to_vec(),
        ancestors: vec![root.clone()],
        metadata: Metadata {
            created_at: hlc_ns,
            updated_at: hlc_ns.into(),
            storage_type: StorageType::Shared {
                writers: crate::entities::full_mask(writers.clone()),
                signature_data: None, // <- the bug trigger
            },
            crdt_type: None,
            field_name: None,
            schema_version: None,
        },
    }
}

/// #2716: an unsigned `Shared` action sitting FIRST in a `CausalActions` batch
/// must not prevent a valid sibling action (applied later in the same batch)
/// from being applied. Before the fix, the unsigned action's `InvalidData`
/// propagated out of `Root::sync` (→ fatal panic in `__calimero_sync_next`) and
/// the whole batch — including the valid sibling — was lost.
#[test]
fn unsigned_shared_action_does_not_abort_the_sync_batch() {
    let root = setup();

    let alice_sk = make_signing_key(0xA1);
    let alice = pubkey_of(&alice_sk);
    let writers: BTreeSet<PublicKey> = [alice].into_iter().collect();

    let bad_id = entity_id(0xBA); // unsigned Shared — must be skipped
    let good_id = entity_id(0x60); // valid signed Shared — must apply

    // Bad action FIRST, so a batch-aborting error would prevent the good one.
    let bad = unsigned_shared_add(bad_id, writers.clone(), 100, &root);
    let good = build_signed_shared_action(
        true,
        good_id,
        b"good".to_vec(),
        writers.clone(),
        200,
        &alice_sk,
        vec![root.clone()],
    );

    // The receiver's per-action `effective_writers` map (#2266). The good
    // action verifies against {Alice}; the bad one is rejected before any
    // writer check, so its entry is irrelevant.
    let mut effective_writers: BTreeMap<Id, BTreeMap<PublicKey, OpMask>> = BTreeMap::new();
    let _ = effective_writers.insert(good_id, crate::entities::full_mask(writers.clone()));

    let payload = to_vec(&StorageDelta::CausalActions {
        actions: vec![bad, good],
        delta_id: [0xD1; 32],
        delta_hlc: hlc(300),
        effective_writers,
    })
    .unwrap();

    // Must NOT return Err — that is what becomes the fatal panic in production.
    Root::<EmptyData, S>::sync(&payload, &ApplyContext::empty())
        .expect("a single unsigned action must not abort the whole sync batch (#2716)");

    // The valid sibling applied despite the unsigned action ahead of it.
    assert!(
        Interface::<S>::find_by_id_raw(good_id).is_some(),
        "the valid signed action must apply even though an unsigned action \
         preceded it in the batch (#2716)"
    );
    // The unsigned action was dropped — never authoritative.
    assert!(
        Interface::<S>::find_by_id_raw(bad_id).is_none(),
        "the unsigned Shared action must be dropped, not stored"
    );
}
