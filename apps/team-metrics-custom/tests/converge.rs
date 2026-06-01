//! #2577 end-to-end for the HAND-WRITTEN `Mergeable` + manual `RekeyTarget` app.
//! Two replicas concurrently record wins for the same team; the counters must
//! SUM (not be lost to LWW). Own integration binary for isolation.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use calimero_storage::collections::Root;
use calimero_storage::env::{self, RuntimeEnv};
use calimero_storage::interface::ApplyContext;
use calimero_storage::register_crdt_merge_for_test;
use calimero_storage::store::Key;
use team_metrics_custom::TeamMetricsApp;

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
fn team_stats_converge_to_summed_value_custom() {
    env::reset_environment();
    register_crdt_merge_for_test::<TeamMetricsApp>();
    calimero_sdk::event::register::<TeamMetricsApp>();
    // `#[app::state]`-generated: registers re-key thunks for the value types of
    // the root's collection fields. `teams: UnorderedMap<String, TeamStats>` →
    // `TeamStats` is registered (and its hand-written `RekeyTarget` impl used).
    // This is the WASM-load / TestHost-bridge path; we call it directly here.
    // (One level deep — see `generate_rekey_register_method` for the scope.)
    TeamMetricsApp::__calimero_register_rekey();

    let a: Store = Default::default();
    let b: Store = Default::default();
    env::with_runtime_env(env_for(&a, [1; 32]), || {
        Root::new(TeamMetricsApp::init).commit();
    });
    *b.borrow_mut() = a.borrow().clone();

    let da = env::with_runtime_env(env_for(&a, [1; 32]), || {
        let mut app = Root::<TeamMetricsApp>::fetch().unwrap();
        let _ = app.record_win("liverpool".into());
        app.commit();
        env::take_last_artifact().unwrap()
    });
    let db = env::with_runtime_env(env_for(&b, [2; 32]), || {
        let mut app = Root::<TeamMetricsApp>::fetch().unwrap();
        let _ = app.record_win("liverpool".into());
        app.commit();
        env::take_last_artifact().unwrap()
    });

    let (ha, wa) = env::with_runtime_env(env_for(&a, [1; 32]), || {
        Root::<TeamMetricsApp>::sync(&db, &ApplyContext::empty()).unwrap();
        let app = Root::<TeamMetricsApp>::fetch().unwrap();
        (env::root_hash(), app.get_wins("liverpool".into()).unwrap())
    });
    let (hb, wb) = env::with_runtime_env(env_for(&b, [2; 32]), || {
        Root::<TeamMetricsApp>::sync(&da, &ApplyContext::empty()).unwrap();
        let app = Root::<TeamMetricsApp>::fetch().unwrap();
        (env::root_hash(), app.get_wins("liverpool".into()).unwrap())
    });

    println!("wins a={wa} b={wb}; converged={}", ha == hb);
    assert_eq!(
        wa, 2,
        "replica A: both replicas' wins must survive (no LWW data loss)"
    );
    assert_eq!(
        wb, 2,
        "replica B: both replicas' wins must survive (no LWW data loss)"
    );
    assert_eq!(ha, hb, "replicas must converge to the same root hash");
}
