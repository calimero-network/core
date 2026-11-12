use std::collections::BTreeMap;

use calimero_pipeline::{
    compile, estimate, run_cost, worst_case_cost, worst_case_fuel, Caps, Env, PriceTable,
};

const TABLE: PriceTable = PriceTable {
    version: 1,
    fuel_per_vcpu_second: 1_000_000_000,
    vcpu_second: 100,
    gib_second: 10,
    connector_gib: 0,
    storage_gib_month: 0,
    egress_gib: 0,
};

const CAPS: Caps = Caps {
    fuel: 1_000,
    memory_mib: 256,
    wall_secs: 60,
};

const SRC: &str = "apiVersion: compute.calimero.network/v1\nkind: Pipeline\nname: p\nmodule: m\ninputs:\n  x: { kind: files }\nsteps:\n  fan: { method: run, forEach: inputs.x, maxItems: 100, batch: 4 }\n  after: { method: run, forEach: fan.outputs, maxItems: 7, batch: 2 }\n  once: { method: run }\n";

fn env() -> Env {
    Env {
        modules: BTreeMap::from([("m".to_string(), BTreeMap::from([("run".to_string(), CAPS)]))]),
        ..Env::default()
    }
}

fn counts(n: u32) -> BTreeMap<String, u32> {
    BTreeMap::from([("x".to_string(), n)])
}

#[test]
fn runs_follow_the_count_and_batch_below_max_items() {
    let p = compile(SRC).unwrap();
    let e = estimate(&p, &env(), &counts(10), None);
    assert_eq!(e.steps["fan"].runs, 3);
    assert_eq!(e.steps["fan"].fuel, 3_000);
    // a fan-out over a step's outputs keeps maxItems, a plain step is one run
    assert_eq!(e.steps["after"].runs, 4);
    assert_eq!(e.steps["once"].runs, 1);
    assert_eq!(e.fuel, 3_000 + 4_000 + 1_000);
}

#[test]
fn a_count_above_max_items_is_capped() {
    let p = compile(SRC).unwrap();
    let e = estimate(&p, &env(), &counts(5_000), None);
    assert_eq!(e.steps["fan"].runs, 25);
}

#[test]
fn no_counts_is_the_worst_case_in_fuel_and_cost() {
    let p = compile(SRC).unwrap();
    let e = estimate(&p, &env(), &BTreeMap::new(), Some(&TABLE));
    assert_eq!(e.steps["fan"].runs, 25);
    assert_eq!(e.fuel, worst_case_fuel(&p, &env()));
    assert_eq!(e.cost, Some(worst_case_cost(&p, &env(), &TABLE)));
    let one = run_cost(CAPS.fuel, CAPS.memory_mib, CAPS.wall_secs, &TABLE);
    assert_eq!(e.steps["fan"].cost, Some(one * 25));
}

#[test]
fn cost_is_none_without_a_table() {
    let p = compile(SRC).unwrap();
    let e = estimate(&p, &env(), &counts(10), None);
    assert_eq!(e.cost, None);
    assert!(e.steps.values().all(|s| s.cost.is_none()));
}

#[test]
fn a_count_of_zero_is_no_runs() {
    let p = compile(SRC).unwrap();
    assert_eq!(estimate(&p, &env(), &counts(0), None).steps["fan"].runs, 0);
}
