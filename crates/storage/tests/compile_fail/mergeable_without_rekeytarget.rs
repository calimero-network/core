// Rejection (#D5): a HAND-WRITTEN `impl Mergeable` that does NOT also implement
// `RekeyTarget`. `RekeyTarget` is a supertrait of `Mergeable`, so this is a hard
// compile error — turning the "nested collection silently gets a per-replica
// random id because its owning struct's Mergeable impl never registered a
// re-key" footgun (the #2577 data loss that removed RGA from mero-drive) into a
// compile-time failure instead of silent divergence.
//
// The fix the author must apply is to add the matching `impl RekeyTarget`
// (re-keying each nested collection field) — exactly what `#[derive(Mergeable)]`
// generates automatically. A struct that derives Mergeable can never hit this.
//
// NOTE: the captured `.stderr` is blessed for the `testing`-ON feature set,
// which is what CI sees: `cargo test` builds the whole workspace, and feature
// unification (`calimero-dag`/`calimero-node` enable `calimero-storage/testing`)
// turns `testing` ON for this crate's own test binary. The "help: the following
// other types implement trait `RekeyTarget`" list rustc prints is truncated
// after 8 entries and its membership shifts with the active feature set (e.g.
// `testing` brings `tests::common::EmptyData` into the window), so a single
// literal snapshot can't match both feature sets. The harness therefore gates
// this case behind `#[cfg(feature = "testing")]` (see `compile_fail.rs`) so it
// only runs against the snapshot it was blessed for. Regenerate with
// `TRYBUILD=overwrite cargo test -p calimero-storage --test compile_fail --features testing`.

use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{Counter, Mergeable};

#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
struct BadStruct {
    // Nests a collection, so a missing re-key would silently lose concurrent
    // increments — the precise footgun the supertrait bound forbids.
    counter: Counter,
}

// Hand-written `Mergeable` with NO `RekeyTarget` impl: must fail to compile.
impl Mergeable for BadStruct {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.counter.merge(&other.counter)
    }
}

fn main() {}
