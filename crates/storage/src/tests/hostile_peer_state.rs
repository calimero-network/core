//! A peer's root, app-state entry and register stamp meet the rules of every
//! remote write: a stamp within the drift bound, and bytes the entry's type reads.

use core::num::NonZeroU128;

use borsh::{from_slice, to_vec};
use serial_test::serial;

use crate::collections::LwwRegister;
use crate::env;
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use crate::tests::owned_rules::text;

type Reg = LwwRegister<String>;

// ---------------------------------------------------------------------------
// A register's own stamp
// ---------------------------------------------------------------------------

/// A register as a peer sends it, stamped `ntp` whatever the clock says.
fn register_stamped(value: &str, ntp: u64) -> Reg {
    let stamp = HybridTimestamp::new(Timestamp::new(NTP64(ntp), ID::from(NonZeroU128::MIN)));
    from_slice(&to_vec(&(value.to_owned(), stamp)).expect("register"))
        .expect("a register is its value and stamp")
}

#[test]
#[serial]
fn a_register_stamped_far_ahead_does_not_win_a_merge() {
    env::reset_for_testing();
    let stored = text("alice");
    let mut control = stored.clone();
    control.merge(&text("bob"));
    assert_eq!(control.get().as_str(), "bob", "control: a newer write wins");

    let mut merged = stored;
    merged.merge(&register_stamped("mallory", u64::MAX));

    assert_eq!(
        merged.get().as_str(),
        "alice",
        "a stamp beyond the drift bound must not win"
    );
}

#[test]
#[serial]
fn a_register_stamped_far_ahead_merges_the_same_on_either_side() {
    env::reset_for_testing();
    let honest = text("alice");
    let hostile = register_stamped("mallory", u64::MAX);

    let mut stored_honest = honest.clone();
    stored_honest.merge(&hostile);
    let mut stored_hostile = hostile.clone();
    stored_hostile.merge(&honest);

    assert_eq!(
        stored_honest.get(),
        stored_hostile.get(),
        "merge must not depend on which side holds the far-future stamp"
    );
    assert_eq!(stored_hostile.get().as_str(), "alice");
}
