#![no_main]
//! Pipeline specs: a member sends YAML (or JSON) that `calimero_pipeline::compile`
//! parses and checks before the node admits it, and the compiled pipeline is
//! then replicated in the app's log, where every replica decodes it.
//!
//! The input is the source. What compiles must survive the log: encoded and
//! decoded it is the same pipeline (the same canonical bytes, which a run is
//! named by), and the operator's checks and estimates answer without a panic.

use std::collections::BTreeMap;

use calimero_pipeline::{
    canonical_json, check, compile, worst_case_cost, worst_case_fuel, Caps, Env, Pipeline,
    PriceTable,
};
use libfuzzer_sys::fuzz_target;

fn env() -> Env {
    let caps = Caps {
        fuel: 1_000_000,
        memory_mib: 256,
        wall_secs: 60,
    };
    let methods = ["run", "extract", "score", "report"]
        .map(|m| (m.to_owned(), caps))
        .into();
    Env {
        default_module: Some("m".into()),
        modules: BTreeMap::from([("m".to_owned(), methods)]),
        pools: BTreeMap::from([("standard".to_owned(), 512), ("highmem".to_owned(), 4096)]),
    }
}

const PRICES: PriceTable = PriceTable {
    version: 1,
    fuel_per_vcpu_second: 1_000_000_000,
    vcpu_second: u64::MAX,
    gib_second: u64::MAX,
    connector_gib: 1,
    storage_gib_month: 1,
    egress_gib: 1,
};

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(p) = compile(source) else {
        return;
    };
    let canonical = canonical_json(&p);
    let logged = serde_json::to_vec(&p).expect("a pipeline encodes");
    let back: Pipeline = match serde_json::from_slice(&logged) {
        Ok(back) => back,
        Err(e) => panic!(
            "a compiled pipeline does not decode from the log: {e}\n{}",
            String::from_utf8_lossy(&logged)
        ),
    };
    assert_eq!(
        canonical_json(&back),
        canonical,
        "a pipeline changed on its way through the log"
    );

    let env = env();
    let _ = check(&p, &env);
    let _ = worst_case_fuel(&p, &env);
    let _ = worst_case_cost(&p, &env, &PRICES);
});
