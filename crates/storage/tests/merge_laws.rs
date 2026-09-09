//! The merge contract, checked rather than documented.
//!
//! `Mergeable`'s docs, `#[app::mergeable]`'s docs and the reference app's README
//! all state that a merge rule must be deterministic, commutative, associative,
//! idempotent and total. Nothing enforced any of it until this file.
//!
//! Own binary because `assert_merge_laws` runs under `with_merge_mode`, which is
//! process-global state the convergence harness also drives.

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::rekey::RekeyTarget;
use calimero_storage::collections::{MergeStrategy, Mergeable};
use calimero_storage::testing::assert_merge_laws;

/// A well-behaved rule: bitwise-OR is a grow-only set.
#[derive(Clone, Default, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Badges {
    mask: u64,
}

#[diagnostic::do_not_recommend]
impl MergeStrategy for Badges {
    const DISPATCHED: bool = false;
}
impl RekeyTarget for Badges {
    fn rekey_relative_to(&mut self, _parent_id: Id) {}
}
impl Mergeable for Badges {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.mask |= other.mask;
        Ok(())
    }
}

/// The rule people actually write when they mean "latest wins", and the one the
/// laws exist to reject. `battleships` ships this exact shape.
#[derive(Clone, Default, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct TakeOther {
    value: u64,
}

#[diagnostic::do_not_recommend]
impl MergeStrategy for TakeOther {
    const DISPATCHED: bool = false;
}
impl RekeyTarget for TakeOther {
    fn rekey_relative_to(&mut self, _parent_id: Id) {}
}
impl Mergeable for TakeOther {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        *self = other.clone();
        Ok(())
    }
}

fn badges(mask: u64) -> Badges {
    Badges { mask }
}

#[test]
fn a_union_rule_satisfies_every_law() {
    assert_merge_laws(&[badges(0b001), badges(0b010), badges(0b100)]);
}

/// The whole point. "Just take the other side" is the most natural thing to
/// write and it is not a merge: `merge(a, b)` and `merge(b, a)` disagree, so two
/// replicas seeing the same pair in different orders settle on different values
/// and never converge.
///
/// Without this assertion the helper could be vacuous — passing everything —
/// and nothing would say so.
#[test]
#[should_panic(expected = "not COMMUTATIVE")]
fn take_other_is_rejected_as_non_commutative() {
    assert_merge_laws(&[TakeOther { value: 1 }, TakeOther { value: 2 }]);
}

/// A single sample satisfies every law trivially, so the helper refuses rather
/// than reporting a pass that means nothing.
#[test]
#[should_panic(expected = "at least two DIFFERENT samples")]
fn one_sample_is_refused() {
    assert_merge_laws(&[badges(0b001)]);
}
