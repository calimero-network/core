//! End-to-end proof that an app's own merge rule decides a real collection
//! entry: insert stamps it, sync applies through `Interface::save_internal`,
//! and `try_merge_non_root` dispatches on the stamp.
//!
//! This is the test PR #1995 lacked. Its dispatch code was correct and
//! unreachable, and only a run through the production path distinguishes those
//! — a unit test handed a fabricated `CrdtType::Custom` passes either way.
//!
//! **Its own test binary, deliberately.** `ROOT_ID` is a process-wide
//! `LazyLock<Id>` derived from `context_id()`, so whichever test touches a
//! collection first freezes it — and a sibling test that does so outside a
//! `RuntimeEnv` freezes it at the no-env fallback `[236; 32]`, after which
//! genesis here fails with `CannotCreateOrphan`. Sharing a binary with the
//! table-level tests is what that costs.

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::rekey::register_rekey_cascade;
use calimero_storage::collections::{MergeStrategy, Mergeable, Root, UnorderedMap};
use calimero_storage::env::{self, RuntimeEnv};
use calimero_storage::interface::ApplyContext;
use calimero_storage::store::Key;
use serial_test::serial;

type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;
//
// Everything above tests a table. This drives the production path — insert
// stamps the entry, sync applies through `Interface::save_internal`, and
// `try_merge_non_root` dispatches on the stamp — with a rule LWW cannot
// imitate. It is the test #1995 lacked: its dispatch was correct and
// unreachable, and only an end-to-end run distinguishes those.

fn env_for(s: &Store, ex: [u8; 32]) -> RuntimeEnv {
    let r = s.clone();
    let reader = Rc::new(move |k: &Key| r.borrow().get(&k.to_bytes()).cloned());
    let w = s.clone();
    let writer =
        Rc::new(move |k: Key, v: &[u8]| w.borrow_mut().insert(k.to_bytes(), v.to_vec()).is_some());
    let rm = s.clone();
    let remover = Rc::new(move |k: &Key| rm.borrow_mut().remove(&k.to_bytes()).is_some());
    let account = {
        let mut a = ex;
        a[1] = 0xAC;
        a
    };
    RuntimeEnv::new(reader, writer, remover, [7u8; 32], ex, account)
}

/// `bid` is a plain `u64` — no CRDT of its own — so whatever decides it is
/// visible in the answer. The app says highest wins.
#[app::mergeable]
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Auction {
    bid: u64,
}

impl Mergeable for Auction {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.bid = self.bid.max(other.bid);
        Ok(())
    }
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct AuctionApp {
    lots: UnorderedMap<String, Auction>,
}

// The root's own re-key: anchor `lots` under the root id, or the map keeps a
// random id and its entries are orphans. `#[app::state]` generates this; the
// harness spells it out because these types are declared in a test.
impl calimero_storage::collections::rekey::RekeyTarget for AuctionApp {
    fn rekey_relative_to(&mut self, parent_id: calimero_storage::address::Id) {
        calimero_storage::rekey_field_if_supported!(
            &mut self.lots,
            calimero_storage::collections::rekey::field_child_id(parent_id, "lots")
        );
    }

    fn register_nested_value_types() {
        register_rekey_cascade::<Auction>();
    }
}

// Structural: a test fixture, merged by the storage layer's own rules.
#[diagnostic::do_not_recommend]
impl MergeStrategy for AuctionApp {
    const DISPATCHED: bool = false;
}

impl Mergeable for AuctionApp {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.lots.merge(&other.lots)
    }
}

/// Two replicas bid concurrently on the same lot. Under LWW one bid is thrown
/// away and the replicas can disagree about which; under the app's rule both
/// land on the higher one.
#[test]
#[serial]
fn the_apps_rule_decides_a_real_entry_across_two_replicas() {
    // The env and both registries are process-global with no reset between
    // tests in a binary; the other tests here register their own types.
    env::reset_environment();
    register_rekey_cascade::<Auction>();
    calimero_storage::merge::register_crdt_merge_for_test::<AuctionApp>();

    let a: Store = Default::default();
    let b: Store = Default::default();

    env::with_runtime_env(env_for(&a, [1; 32]), || {
        Root::new(AuctionApp::default).commit();
    });
    *b.borrow_mut() = a.borrow().clone();

    let bid = |store: &Store, ex: [u8; 32], amount: u64| {
        env::with_runtime_env(env_for(store, ex), || {
            let mut app = Root::<AuctionApp>::fetch().unwrap();
            app.lots
                .insert("lot-1".to_owned(), Auction { bid: amount })
                .unwrap();
            app.commit();
            env::take_last_artifact().unwrap()
        })
    };

    // Low bid second in wall-clock order, so a naive "newest wins" would keep it.
    let da = bid(&a, [1; 32], 900);
    let db = bid(&b, [2; 32], 100);

    let read = |store: &Store, ex: [u8; 32], delta: &[u8]| {
        env::with_runtime_env(env_for(store, ex), || {
            Root::<AuctionApp>::sync(delta, &ApplyContext::empty()).unwrap();
            let app = Root::<AuctionApp>::fetch().unwrap();
            let bid = app.lots.get("lot-1").unwrap().map(|v| v.bid);
            (env::root_hash(), bid)
        })
    };

    let (ha, va) = read(&a, [1; 32], &db);
    let (hb, vb) = read(&b, [2; 32], &da);

    assert_eq!(va, Some(900), "replica A must take the higher bid");
    assert_eq!(vb, Some(900), "replica B must take the higher bid");
    assert_eq!(ha, hb, "replicas must converge");
}
