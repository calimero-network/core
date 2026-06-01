//! #2577 end-to-end: the REAL `#[app::state]` + `#[derive(Mergeable)]` app, with
//! NO hand-written rekey code — only the macro-generated `RekeyTarget` impl and
//! `__calimero_register_rekey()` registration. Two replicas concurrently record
//! wins for the same team; the counters must SUM (not be lost to LWW).
//!
//! Own integration binary so it runs isolated from process-global state.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use calimero_storage::collections::Root;
use calimero_storage::env::{self, RuntimeEnv};
use calimero_storage::interface::ApplyContext;
use calimero_storage::register_crdt_merge_for_test;
use calimero_storage::store::Key;
use team_metrics_macro::TeamMetricsApp;

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
fn team_stats_converge_to_summed_value() {
    env::reset_environment();
    // Exactly what the WASM module load / TestHost bridge do:
    register_crdt_merge_for_test::<TeamMetricsApp>();
    calimero_sdk::event::register::<TeamMetricsApp>(); // record_win emits events
    TeamMetricsApp::__calimero_register_rekey(); // macro-generated (#2577)

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
