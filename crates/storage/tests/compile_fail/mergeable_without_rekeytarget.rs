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
// NOTE: the captured `.stderr` is for the DEFAULT feature set — what CI's
// `cargo test` runs. The "help: the following other types implement trait
// RekeyTarget" list rustc prints is truncated and varies with the active
// feature set (e.g. `--features testing` exposes a slightly different set), so
// running this test with non-default features may report a snapshot mismatch in
// that help block only. Regenerate with `TRYBUILD=overwrite` for the feature
// set you run, or rely on the default-feature run CI uses.

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
