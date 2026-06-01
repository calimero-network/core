//! De-risk probe for #2577: prove that giving a custom struct value's nested
//! collections DETERMINISTIC ids (via a `RekeyTarget` impl + registration) makes
//! concurrent same-key writes converge to the CORRECT value (counter sums),
//! instead of last-writer-wins'ing the struct blob and losing data.
//!
//! `TeamStats` here impls `RekeyTarget` BY HAND and is registered explicitly via
//! the `register_rekey_if_supported!` / `rekey_field_if_supported!` autoref
//! macros — exactly what `#[derive(Mergeable)]` + `#[app::state]` will generate.
//!
//! Own integration binary (not a `src/tests` module) so it runs isolated from
//! the unit-test suite, which shares process-global state (the rekey registry).

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::rekey::{field_child_id, RekeyTarget};
use calimero_storage::collections::{Counter, Mergeable, Root, UnorderedMap};
use calimero_storage::env::{self, RuntimeEnv};
use calimero_storage::interface::ApplyContext;
use calimero_storage::store::Key;
use calimero_storage::{
    register_crdt_merge_for_test, register_rekey_if_supported, rekey_field_if_supported,
};

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct TeamStats {
    wins: Counter,
    losses: Counter,
}

impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)
    }
}

// What the macro will generate: re-key each collection field under a
// field-namespaced child of the entry id, so every replica derives identical
// ids and the nested counters converge as child entities.
impl RekeyTarget for TeamStats {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.wins, field_child_id(parent_id, "wins"));
        rekey_field_if_supported!(&mut self.losses, field_child_id(parent_id, "losses"));
    }
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct App {
    teams: UnorderedMap<String, TeamStats>,
}

impl Mergeable for App {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.teams.merge(&other.teams)
    }
}

impl App {
    fn record_win(&mut self, team: &str) {
        let mut s = self.teams.get(team).unwrap().unwrap_or_default();
        s.wins.increment().unwrap();
        self.teams.insert(team.to_owned(), s).unwrap();
    }
    fn wins(&self, team: &str) -> u64 {
        self.teams
            .get(team)
            .unwrap()
            .map(|s| s.wins.value().unwrap())
            .unwrap_or(0)
    }
}

type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

fn env_for(s: &Store, ex: [u8; 32]) -> RuntimeEnv {
    let r = s.clone();
    let reader = Rc::new(move |k: &Key| r.borrow().get(&k.to_bytes()).cloned());
    let w = s.clone();
    let writer =
        Rc::new(move |k: Key, v: &[u8]| w.borrow_mut().insert(k.to_bytes(), v.to_vec()).is_some());
    let rm = s.clone();
    let remover = Rc::new(move |k: &Key| rm.borrow_mut().remove(&k.to_bytes()).is_some());
    RuntimeEnv::new(reader, writer, remover, [7u8; 32], ex)
}

#[test]
fn deterministic_rekey_makes_struct_value_counters_converge() {
    env::reset_environment();
    register_crdt_merge_for_test::<App>();
    // What the macro will emit for each collection-field value type. Autoref
    // makes this a no-op for non-RekeyTarget value types (proven below).
    register_rekey_if_supported!(TeamStats);
    register_rekey_if_supported!(String); // leaf value type: must be a safe no-op

    let a: Store = Default::default();
    let b: Store = Default::default();
    env::with_runtime_env(env_for(&a, [1; 32]), || {
        Root::new(App::default).commit();
    });
    *b.borrow_mut() = a.borrow().clone();

    let da = env::with_runtime_env(env_for(&a, [1; 32]), || {
        let mut app = Root::<App>::fetch().unwrap();
        app.record_win("liverpool");
        app.commit();
        env::take_last_artifact().unwrap()
    });
    let db = env::with_runtime_env(env_for(&b, [2; 32]), || {
        let mut app = Root::<App>::fetch().unwrap();
        app.record_win("liverpool");
        app.commit();
        env::take_last_artifact().unwrap()
    });

    let (ha, wa) = env::with_runtime_env(env_for(&a, [1; 32]), || {
        Root::<App>::sync(&db, &ApplyContext::empty()).unwrap();
        (
            env::root_hash(),
            Root::<App>::fetch().unwrap().wins("liverpool"),
        )
    });
    let (hb, wb) = env::with_runtime_env(env_for(&b, [2; 32]), || {
        Root::<App>::sync(&da, &ApplyContext::empty()).unwrap();
        (
            env::root_hash(),
            Root::<App>::fetch().unwrap().wins("liverpool"),
        )
    });

    println!("wins a={wa} b={wb}; converged={}", ha == hb);
    assert_eq!(wa, 2, "replica A: both increments must survive");
    assert_eq!(wb, 2, "replica B: both increments must survive");
    assert_eq!(ha, hb, "replicas must converge to the same root hash");
}
