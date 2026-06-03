//! #2577 storage-level probe: a custom struct stored as a collection VALUE
//! converges to the CORRECT value (counters sum) IFF its nested collections get
//! deterministic ids — via a `RekeyTarget` impl + registration. The positive
//! test proves the fix; the negative test documents the pre-fix data loss (an
//! unregistered value type is last-writer-wins'd as a blob).
//!
//! Own integration binary (gated `required-features = ["testing"]` in Cargo) so
//! it runs isolated from the unit-test suite, which shares the process-global
//! rekey registry. The two structs are DISTINCT types because that registry has
//! no reset — registering one must not affect the other.

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use calimero_sdk::app;
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
use serial_test::serial;

/// A struct that implements `RekeyTarget` (what the macro generates). When
/// registered, its nested counters get deterministic ids and converge.
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct FixedStats {
    wins: Counter,
}

impl Mergeable for FixedStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.wins.merge(&other.wins)
    }
}

impl RekeyTarget for FixedStats {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.wins, field_child_id(parent_id, "wins"));
    }
}

/// Same shape, but NOT a `RekeyTarget` and never registered — i.e. the pre-fix
/// world. Its nested counter keeps a per-replica-random id, so the blob differs
/// and concurrent writes are last-writer-wins'd (data loss).
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct UnfixedStats {
    wins: Counter,
}

impl Mergeable for UnfixedStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.wins.merge(&other.wins)
    }
}

/// Root app generic over the value type, so one driver exercises both.
trait TeamApp: BorshSerialize + BorshDeserialize + Default + Mergeable + 'static {
    fn record_win(&mut self, team: &str) -> app::Result<()>;
    fn wins(&self, team: &str) -> app::Result<u64>;
}

macro_rules! team_app {
    ($app:ident, $val:ty) => {
        #[derive(BorshSerialize, BorshDeserialize, Default)]
        #[borsh(crate = "calimero_sdk::borsh")]
        struct $app {
            teams: UnorderedMap<String, $val>,
        }
        impl Mergeable for $app {
            fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
                self.teams.merge(&other.teams)
            }
        }
        impl TeamApp for $app {
            fn record_win(&mut self, team: &str) -> app::Result<()> {
                // `app::Result` + `?`, exactly as real apps write methods (see
                // apps/team-metrics-custom). The `entry().or_default()` handle is
                // write-back-guarded (core#2576): the increment persists when the
                // guard drops — no explicit re-insert.
                let mut stats = self.teams.entry(team.to_owned())?.or_default()?;
                stats.wins.increment()?;
                Ok(())
            }
            fn wins(&self, team: &str) -> app::Result<u64> {
                match self.teams.get(team)? {
                    Some(s) => Ok(s.wins.value()?),
                    None => Ok(0),
                }
            }
        }
    };
}

team_app!(FixedApp, FixedStats);
team_app!(UnfixedApp, UnfixedStats);

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

/// Two replicas each record one win for the same team under their own executor
/// id, exchange deltas, and we read back each replica's win count + root hash.
/// Returns `(wins_a, wins_b, converged)`.
fn drive<T: TeamApp>() -> (u64, u64, bool) {
    let a: Store = Default::default();
    let b: Store = Default::default();
    env::with_runtime_env(env_for(&a, [1; 32]), || {
        Root::new(T::default).commit();
    });
    *b.borrow_mut() = a.borrow().clone();

    let da = env::with_runtime_env(env_for(&a, [1; 32]), || {
        let mut app = Root::<T>::fetch().unwrap();
        app.record_win("liverpool").unwrap();
        app.commit();
        env::take_last_artifact().unwrap()
    });
    let db = env::with_runtime_env(env_for(&b, [2; 32]), || {
        let mut app = Root::<T>::fetch().unwrap();
        app.record_win("liverpool").unwrap();
        app.commit();
        env::take_last_artifact().unwrap()
    });

    let (ha, wa) = env::with_runtime_env(env_for(&a, [1; 32]), || {
        Root::<T>::sync(&db, &ApplyContext::empty()).unwrap();
        (
            env::root_hash(),
            Root::<T>::fetch().unwrap().wins("liverpool").unwrap(),
        )
    });
    let (hb, wb) = env::with_runtime_env(env_for(&b, [2; 32]), || {
        Root::<T>::sync(&da, &ApplyContext::empty()).unwrap();
        (
            env::root_hash(),
            Root::<T>::fetch().unwrap().wins("liverpool").unwrap(),
        )
    });
    (wa, wb, ha == hb)
}

#[test]
#[serial]
fn registered_rekey_makes_struct_value_counters_converge() {
    env::reset_environment();
    register_crdt_merge_for_test::<FixedApp>();
    // What the macro emits for each collection-field value type. Autoref makes
    // it a no-op for leaf value types (proven by passing `String`).
    register_rekey_if_supported!(FixedStats);
    register_rekey_if_supported!(String);

    let (wa, wb, converged) = drive::<FixedApp>();
    println!("FIXED   wins a={wa} b={wb} converged={converged}");
    assert_eq!(wa, 2, "replica A: both increments must survive");
    assert_eq!(wb, 2, "replica B: both increments must survive");
    assert!(converged, "replicas must converge to the same root hash");
}

#[test]
#[serial]
fn unregistered_value_loses_data_pre_fix() {
    // Regression guard / documentation of the bug #2577 fixes: with no rekey
    // registration (the pre-fix world), the struct value is LWW'd as a blob and
    // concurrent increments are lost. If a future change makes the data survive
    // WITHOUT rekey (e.g. a serialization change), this assertion fires so we
    // re-examine whether the rekey machinery is still doing the work.
    env::reset_environment();
    register_crdt_merge_for_test::<UnfixedApp>();
    // Deliberately NO `register_rekey_if_supported!(UnfixedStats)`.

    let (wa, wb, converged) = drive::<UnfixedApp>();
    println!("UNFIXED wins a={wa} b={wb} converged={converged}");
    // Tight assertions document the EXACT pre-fix failure mode — LWW, not total
    // loss and not partial survival — so a different future regression (e.g.
    // both replicas read 0, or one reads 2) trips this instead of passing
    // silently:
    //   - they still converge, so both agree;
    //   - to the WRONG value: exactly one replica's single increment survives
    //     (1), the other's is dropped — the data loss #2577 fixes.
    //
    // Why `== 1` is deterministic (not flaky): each replica increments under a
    // DISTINCT executor id ([1;32] vs [2;32]). Root-blob LWW breaks ties by
    // executor id, so exactly one whole `UnfixedStats` blob wins on both sides —
    // never a tie that drops both (→0) or keeps both (→2). The increment value
    // is always 1 (each replica did exactly one). If this ever reads 0 or 2,
    // that's a real change in the tiebreak/serialization worth investigating,
    // which is exactly what this guard is for.
    assert!(
        converged,
        "LWW replicas still converge — to the wrong value"
    );
    assert_eq!(wa, wb, "converged replicas must agree on the (wrong) value");
    assert_eq!(
        wa, 1,
        "without rekey, LWW keeps exactly one replica's increment (not summed \
         to 2, not lost to 0); if this changes, re-examine whether deterministic \
         rekey is still what makes #2577 converge"
    );
}

// ---------------------------------------------------------------------------
// Deep custom-struct nesting (#2581): a custom struct reachable only THROUGH
// another custom struct's collection — `App → Map<_, Outer> → Map<_, Inner>`,
// where both `Outer` and `Inner` are app structs.
//
// The root scan only names the type tokens in the ROOT struct's own fields, so
// it registers `Outer` but never `Inner`. Pre-#2581, `Inner` therefore kept
// per-replica-random nested ids and its `score` counter was last-writer-wins'd
// (the exact #2577 loss, one level deeper). The fix makes `#[derive(Mergeable)]`
// register each struct's own value types (`register_nested_value_types`) and has
// `register_rekey_if_supported!` cascade, so registering `Outer` transitively
// registers `Inner`.
//
// These tests register ONLY the root-level tokens (`Outer`, `String`) — exactly
// what the root `#[app::state]` macro emits, NOT `Inner` — and rely on the
// cascade to reach `Inner`. The positive path uses the derive (cascade fires);
// the negative path uses hand-written impls WITHOUT a `register_nested_value_types`
// override (the pre-fix derive shape), so `Inner` stays unregistered and loses
// data. Distinct types throughout, since the rekey registry has no reset.

/// Derive path: cascade should reach `Inner` via `Outer`'s generated
/// `register_nested_value_types`.
#[derive(BorshSerialize, BorshDeserialize, Default, Mergeable)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Inner {
    score: Counter,
}

#[derive(BorshSerialize, BorshDeserialize, Default, Mergeable)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Outer {
    inner: UnorderedMap<String, Inner>,
}

#[derive(BorshSerialize, BorshDeserialize, Default, Mergeable)]
#[borsh(crate = "calimero_sdk::borsh")]
struct DeepApp {
    groups: UnorderedMap<String, Outer>,
}

/// Hand-written impls mirroring the PRE-#2581 derive: a correct
/// `rekey_relative_to` but NO `register_nested_value_types` override (so it
/// takes the trait default no-op). `InnerManual` is consequently never
/// registered when only the root tokens are.
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct InnerManual {
    score: Counter,
}

impl Mergeable for InnerManual {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.score.merge(&other.score)
    }
}

impl RekeyTarget for InnerManual {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.score, field_child_id(parent_id, "score"));
    }
    // Deliberately NO `register_nested_value_types` override — the pre-fix gap.
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct OuterManual {
    inner: UnorderedMap<String, InnerManual>,
}

impl Mergeable for OuterManual {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.inner.merge(&other.inner)
    }
}

impl RekeyTarget for OuterManual {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.inner, field_child_id(parent_id, "inner"));
    }
    // Deliberately NO `register_nested_value_types` override.
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct DeepAppManual {
    groups: UnorderedMap<String, OuterManual>,
}

impl Mergeable for DeepAppManual {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.groups.merge(&other.groups)
    }
}

// Reuse the `drive::<T>()` two-replica harness: `record_win`/`wins` navigate two
// collection levels (`groups[team].inner[team].score`) instead of one.
macro_rules! deep_team_app {
    ($app:ty) => {
        impl TeamApp for $app {
            fn record_win(&mut self, team: &str) -> app::Result<()> {
                let mut outer = self.groups.entry(team.to_owned())?.or_default()?;
                let mut inner = outer.inner.entry(team.to_owned())?.or_default()?;
                inner.score.increment()?;
                Ok(())
            }
            fn wins(&self, team: &str) -> app::Result<u64> {
                match self.groups.get(team)? {
                    Some(outer) => match outer.inner.get(team)? {
                        Some(inner) => Ok(inner.score.value()?),
                        None => Ok(0),
                    },
                    None => Ok(0),
                }
            }
        }
    };
}

deep_team_app!(DeepApp);
deep_team_app!(DeepAppManual);

#[test]
#[serial]
fn cascade_registers_deeply_nested_custom_value() {
    env::reset_environment();
    register_crdt_merge_for_test::<DeepApp>();
    // Register ONLY what the root `#[app::state]` scan would for
    // `DeepApp { groups: UnorderedMap<String, Outer> }`: the value type `Outer`
    // and the key type `String`. `Inner` is NOT named here — the cascade through
    // `Outer::register_nested_value_types` must register it.
    register_rekey_if_supported!(Outer);
    register_rekey_if_supported!(String);

    let (wa, wb, converged) = drive::<DeepApp>();
    println!("DEEP-CASCADE wins a={wa} b={wb} converged={converged}");
    assert_eq!(
        wa, 2,
        "replica A: both increments must survive two levels deep"
    );
    assert_eq!(
        wb, 2,
        "replica B: both increments must survive two levels deep"
    );
    assert!(converged, "replicas must converge to the same root hash");
}

#[test]
#[serial]
fn no_cascade_loses_deeply_nested_data_pre_fix() {
    // Pre-fix shape: `OuterManual` has no `register_nested_value_types`, so
    // registering the root tokens leaves `InnerManual` unregistered and its
    // nested counter is LWW'd — the #2577 loss, one level deeper. Same tight
    // `== 1` assertion (and rationale) as `unregistered_value_loses_data_pre_fix`.
    env::reset_environment();
    register_crdt_merge_for_test::<DeepAppManual>();
    register_rekey_if_supported!(OuterManual);
    register_rekey_if_supported!(String);
    // Deliberately NO `register_rekey_if_supported!(InnerManual)`, and
    // `OuterManual` does not cascade into it.

    let (wa, wb, converged) = drive::<DeepAppManual>();
    println!("DEEP-NOCASCADE wins a={wa} b={wb} converged={converged}");
    assert!(
        converged,
        "LWW replicas still converge — to the wrong value"
    );
    assert_eq!(wa, wb, "converged replicas must agree on the (wrong) value");
    assert_eq!(
        wa, 1,
        "without cascading registration, the deeply-nested counter is LWW'd: \
         exactly one replica's increment survives"
    );
}
