//! `migrate_my_entries()` converts the caller's own entries, and only those.
//!
//! Keys of an owned collection are per owner, so another account can hold an
//! entry at a key the caller also holds. The generated sweep walks the caller's
//! own entries: walking every entry would re-write the caller's entry with the
//! other account's value, once per such key, and count entries it cannot
//! convert.

#![allow(clippy::unwrap_used)]

use calimero_sdk::app;
use calimero_sdk::event::NoEvent;
use calimero_sdk::state::{AppState, AppStateInit};
use calimero_sdk::AccountId;
use calimero_storage::collections::AuthoredMap;
use calimero_storage::env;

#[app::state]
#[derive(Default)]
pub struct Notes {
    notes: AuthoredMap<String, u64>,
}

impl AppStateInit for Notes {
    type Return = Notes;
}

/// The binary after an upgrade: every entry below this is stale.
#[derive(calimero_sdk::borsh::BorshSerialize, calimero_sdk::borsh::BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct V2;
impl AppStateInit for V2 {
    type Return = V2;
}
impl AppState for V2 {
    type Event<'a> = NoEvent;
    const SCHEMA_VERSION: u32 = 2;
}

/// The unit default, to leave the process-global target as it was.
#[derive(calimero_sdk::borsh::BorshSerialize, calimero_sdk::borsh::BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Unversioned;
impl AppStateInit for Unversioned {
    type Return = Unversioned;
}
impl AppState for Unversioned {
    type Event<'a> = NoEvent;
}

const ALICE: [u8; 32] = [0x11; 32];
const BOB: [u8; 32] = [0x22; 32];

#[test]
fn the_sweep_converts_only_the_caller_s_entries_at_a_shared_key() {
    env::reset_environment();
    let mut app = Notes::default();
    env::with_account_id(ALICE, || app.notes.insert("k".to_owned(), 1).unwrap());
    env::with_account_id(BOB, || {
        app.notes.insert("k".to_owned(), 2).unwrap();
        app.notes.insert("j".to_owned(), 3).unwrap();
    });

    app::register_schema_version::<V2>();
    let (alice, bob) = (AccountId::from(ALICE), AccountId::from(BOB));
    let k = "k".to_owned();

    let (pending_alice, summary, after_alice) = env::with_account_id(ALICE, || {
        let pending = app.__calimero_count_my_pending();
        let summary = app.__calimero_migrate_my_entries();
        (pending, summary, app.__calimero_count_my_pending())
    });
    let pending_bob = env::with_account_id(BOB, || app.__calimero_count_my_pending());
    app::register_schema_version::<Unversioned>();

    assert_eq!(pending_alice, 1, "alice holds one stale entry");
    assert_eq!((summary.converted, summary.remaining), (1, 0));
    assert_eq!(after_alice, 0);
    assert_eq!(pending_bob, 2, "bob's entries are his to convert");

    assert_eq!(
        app.notes.entry_schema_version_by(&alice, &k).unwrap(),
        Some(2)
    );
    assert_eq!(
        app.notes.get_by(&alice, &k).unwrap(),
        Some(1),
        "her value kept"
    );
    assert_eq!(
        app.notes.entry_schema_version_by(&bob, &k).unwrap(),
        Some(0)
    );
    assert_eq!(app.notes.get_by(&bob, &k).unwrap(), Some(2));
}
