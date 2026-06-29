// Rejection: a hand-written `impl Mergeable` with NO `RekeyTarget` impl must fail
// to compile, since `RekeyTarget` is a supertrait of `Mergeable`. Fix = add the
// matching `impl RekeyTarget` (what `#[derive(Mergeable)]` does automatically).
//
// `.stderr` is blessed for the `testing` feature (see compile_fail.rs); regenerate:
//   TRYBUILD=overwrite cargo test -p calimero-storage --test compile_fail --features testing

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
