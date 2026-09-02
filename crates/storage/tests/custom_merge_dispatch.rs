//! `#[app::mergeable]` declares a type and registers its merge (links 1 and 3
//! of #3785). Stamping the declaration onto collection entries is link 2 and is
//! NOT here — `dispatch_is_declared_but_not_yet_reached` pins that gap so this
//! change cannot be mistaken for a working feature.
//!
//! Every id here is read back off the type via `CrdtMeta::crdt_type()` rather
//! than written out by hand. Hand-built ids are what let PR #1995 ship a
//! dispatch nothing could reach: its tests passed a fabricated
//! `CrdtType::Custom("TestType")` straight into the merge function, proving the
//! dispatch worked while nothing in production ever produced that value.
//!
//! Own integration binary (`required-features = ["testing"]`) because the
//! custom-merge registry is process-global here, with no reset — so each test
//! uses a DISTINCT type.

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use calimero_sdk::app;
use calimero_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::{CustomTypeId, MergeError};
use calimero_storage::collections::{Counter, CrdtMeta, CrdtType, Mergeable};
use calimero_storage::merge::{merge_by_crdt_type, merge_custom};
use calimero_storage::register_custom_merge_if_supported;

/// The id a `CrdtType::Custom` is carrying, or a failure naming what it was.
fn custom_id_of<T: CrdtMeta>() -> CustomTypeId {
    match T::crdt_type() {
        CrdtType::Custom(id) => id,
        other => panic!("expected the type to declare itself Custom, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------

/// `max` on a plain field is the point: field-by-field delegation — what
/// `#[derive(Mergeable)]` generates, and what the entry blob's LWW would do —
/// cannot produce it. If the assertion holds, the app's own rule ran.
#[app::mergeable]
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct HighestBid {
    amount: u64,
    wins: Counter,
}

impl Mergeable for HighestBid {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.wins.merge(&other.wins)?;
        self.amount = self.amount.max(other.amount);
        Ok(())
    }
}

#[app::mergeable(id = "pinned::Identity")]
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct PinnedId {
    amount: u64,
}

impl Mergeable for PinnedId {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.amount = self.amount.max(other.amount);
        Ok(())
    }
}

#[app::mergeable]
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct NeverRegistered {
    amount: u64,
}

impl Mergeable for NeverRegistered {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.amount = self.amount.max(other.amount);
        Ok(())
    }
}

// ---------------------------------------------------------------------------

/// Link 1: the type declares itself, which is what an entry can be stamped from.
#[test]
fn the_attribute_makes_the_type_declare_itself() {
    assert_eq!(
        custom_id_of::<HighestBid>(),
        CustomTypeId::of(concat!(module_path!(), "::HighestBid")),
        "the default id digests the type's declared path"
    );
}

/// The default id follows the path, so a type that moves changes identity. The
/// override is the escape hatch, and it has to actually override.
#[test]
fn a_pinned_id_ignores_the_module_path() {
    assert_eq!(
        custom_id_of::<PinnedId>(),
        CustomTypeId::of("pinned::Identity")
    );
    assert_ne!(
        custom_id_of::<PinnedId>(),
        CustomTypeId::of(concat!(module_path!(), "::PinnedId"))
    );
}

#[test]
fn distinct_types_get_distinct_ids() {
    assert_ne!(
        custom_id_of::<HighestBid>(),
        custom_id_of::<NeverRegistered>()
    );
}

/// Link 3: registration wires the declared id to the app's rule, and dispatch
/// through that id runs it. `amount` proves it — 7 and 3 merge to 7 under the
/// app's `max`, where delegation or LWW would yield whichever side won.
#[test]
fn registration_makes_the_apps_own_rule_reachable() {
    register_custom_merge_if_supported!(HighestBid);

    let low = borsh::to_vec(&HighestBid {
        amount: 3,
        ..Default::default()
    })
    .unwrap();
    let high = borsh::to_vec(&HighestBid {
        amount: 7,
        ..Default::default()
    })
    .unwrap();

    let id = custom_id_of::<HighestBid>();
    let merged: HighestBid = borsh::from_slice(&merge_custom(id, &low, &high).unwrap()).unwrap();
    assert_eq!(merged.amount, 7, "app rule should have taken the max");

    // Commutative, as the contract requires — and the direction LWW would differ on.
    let flipped: HighestBid = borsh::from_slice(&merge_custom(id, &high, &low).unwrap()).unwrap();
    assert_eq!(flipped.amount, 7, "merge must not depend on argument order");
}

/// An id nothing claimed is app-upgrade skew, not a merge failure, and must say
/// so rather than silently resolving.
#[test]
fn an_unregistered_type_reports_the_id_it_could_not_resolve() {
    let id = custom_id_of::<NeverRegistered>();
    let bytes = borsh::to_vec(&NeverRegistered { amount: 1 }).unwrap();

    assert!(
        matches!(merge_custom(id, &bytes, &bytes), Err(MergeError::WasmRequired { type_id }) if type_id == id),
    );
}

/// **The gap this change does not close.** `merge_by_crdt_type` is what
/// `Interface::try_merge_non_root` calls, and it still refuses a `Custom`
/// instead of consulting the registry — and nothing stamps an entry with a
/// `Custom` in the first place. Both are link 2, and land together in the next
/// change; this assertion is expected to be inverted then, not deleted.
#[test]
fn dispatch_is_declared_but_not_yet_reached() {
    register_custom_merge_if_supported!(HighestBid);

    let bytes = borsh::to_vec(&HighestBid::default()).unwrap();
    let result = merge_by_crdt_type(&HighestBid::crdt_type(), &bytes, &bytes);

    assert!(
        matches!(result, Err(MergeError::WasmRequired { .. })),
        "storage still refuses Custom rather than dispatching it"
    );
}
