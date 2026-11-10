use std::collections::BTreeMap;

use calimero_pipeline::{compile, priced_cost, run_cost, worst_case_cost, Caps, Env, PriceTable};

const TABLE: PriceTable = PriceTable {
    version: 1,
    fuel_per_vcpu_second: 1_000_000_000,
    vcpu_second: 100,
    gib_second: 10,
    connector_gib: 0,
    storage_gib_month: 0,
    egress_gib: 0,
};

fn caps(fuel: u64, memory_mib: u32, wall_secs: u32) -> Caps {
    Caps {
        fuel,
        memory_mib,
        wall_secs,
    }
}

fn env(modules: &[(&str, &[(&str, Caps)])]) -> Env {
    Env {
        default_module: None,
        modules: modules
            .iter()
            .map(|(m, methods)| {
                let methods = methods.iter().map(|(n, c)| (n.to_string(), *c)).collect();
                (m.to_string(), methods)
            })
            .collect(),
        pools: BTreeMap::new(),
    }
}

#[test]
fn run_cost_rounds_vcpu_seconds_and_gib_seconds_up() {
    // fuel: ceil(1_500_000_001 / 1e9) = 2 -> 200; memory: ceil(256 * 30 / 1024) = 8 -> 80
    assert_eq!(run_cost(1_500_000_001, 256, 30, &TABLE), 280);
    assert_eq!(run_cost(0, 0, 0, &TABLE), 0);
}

#[test]
fn a_step_without_for_each_costs_exactly_one_run() {
    let p = compile(
        "apiVersion: compute.calimero.network/v1\nkind: Pipeline\nname: p\nmodule: m\nsteps:\n  a: { method: run }\n",
    )
    .unwrap();
    let c = caps(2_000_000_000, 512, 60);
    let e = env(&[("m", &[("run", c)])]);
    assert_eq!(
        worst_case_cost(&p, &e, &TABLE),
        run_cost(c.fuel, c.memory_mib, c.wall_secs, &TABLE)
    );
    assert_eq!(worst_case_cost(&p, &e, &TABLE), 500);
}

#[test]
fn the_spec_example_costs_a_hand_computed_amount() {
    let v1: String = include_str!("invoice-risk.yaml")
        .lines()
        .map(|l| {
            l.replace(", when: \"score.results.exists(r, r.flagged)\"", "")
                .replace(", runIf: failure", "")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let p = compile(&v1).unwrap_or_else(|e| panic!("{e:#?}"));
    let e = env(&[
        (
            "calimero/pdf-extract@1",
            &[("extract", caps(2_000_000_000, 512, 60))],
        ),
        (
            "sha256:7a9c…41d2",
            &[
                ("score", caps(10_000_000_000, 4096, 120)),
                ("escalate", caps(1_000_000_000, 256, 30)),
                ("report", caps(3_000_000_000, 1024, 60)),
                ("incident_summary", caps(1_000_000_000, 256, 30)),
            ],
        ),
    ]);
    // Per run: vcpu_second * ceil(fuel / 1e9) + gib_second * ceil(MiB * secs / 1024).
    // extract: 2 * 100 + 30 * 10 = 500, 1000 runs      = 500_000
    // score: 10 * 100 + 480 * 10 = 5800, ceil(1000 / 25) = 40 runs = 232_000
    // escalate: 100 + 8 * 10 = 180; report: 300 + 60 * 10 = 900; incident: 180
    assert_eq!(worst_case_cost(&p, &e, &TABLE), 733_260);
}

#[test]
fn a_step_with_unknown_method_adds_nothing_and_u64_max_inputs_do_not_overflow() {
    let p = compile(
        "apiVersion: compute.calimero.network/v1\nkind: Pipeline\nname: p\nmodule: m\ninputs:\n  x: { kind: files }\nsteps:\n  a: { method: max, forEach: inputs.x, maxItems: 10000 }\n  b: { method: ghost }\n",
    )
    .unwrap();
    let e = env(&[("m", &[("max", caps(u64::MAX, u32::MAX, u32::MAX))])]);
    let price = |vcpu_second, gib_second| PriceTable {
        fuel_per_vcpu_second: 1,
        vcpu_second,
        gib_second,
        ..TABLE
    };
    // 1 per fuel unit, no memory price: u64::MAX * 10_000 runs is exact in u128.
    assert_eq!(
        worst_case_cost(&p, &e, &price(1, 0)),
        u128::from(u64::MAX) * 10_000
    );
    // Both prices at u64::MAX exceed even u128: the bound saturates, it never wraps or panics.
    assert_eq!(
        worst_case_cost(&p, &e, &price(u64::MAX, u64::MAX)),
        u128::MAX
    );
}

#[test]
fn a_zero_fuel_rate_is_refused() {
    let bad = PriceTable {
        fuel_per_vcpu_second: 0,
        ..TABLE
    };
    assert_eq!(bad.validate().unwrap_err().code, "BAD_PRICES");
    TABLE.validate().unwrap();
}

#[test]
fn the_table_is_camel_case_json_and_refuses_unknown_fields() {
    let j = serde_json::to_value(TABLE).unwrap();
    assert_eq!(j["fuelPerVcpuSecond"], 1_000_000_000u64);
    assert_eq!(j["storageGibMonth"], 0);
    let back: PriceTable = serde_json::from_value(j.clone()).unwrap();
    assert_eq!(back, TABLE);
    let mut extra = j;
    extra["surcharge"] = 1.into();
    assert!(serde_json::from_value::<PriceTable>(extra).is_err());
}

#[test]
fn limits_override_field_by_field() {
    use calimero_pipeline::Limits;
    let own = Limits {
        fuel: Some(7),
        memory_mib: None,
        wall_secs: Some(9),
    };
    let over = Limits {
        fuel: Some(70),
        memory_mib: Some(128),
        wall_secs: None,
    };
    let merged = own.overridden_by(over);
    assert_eq!(
        (merged.fuel, merged.memory_mib, merged.wall_secs),
        (Some(70), Some(128), Some(9))
    );
    assert_eq!(own.overridden_by(Limits::default()), own);
}

#[test]
fn priced_cost_rounds_up_and_never_passes_the_caps() {
    let c = caps(2_000_000_000, 1024, 60);
    // fuel 1 -> 1 vCPU-second (100); 1 GiB-microsecond -> 1 GiB-second (10)
    assert_eq!(priced_cost(1, 1, 0, c, &TABLE), 110);
    assert_eq!(priced_cost(0, 0, 0, c, &TABLE), 0);
    // CPU time above what the fuel says prices the CPU time: 3 s -> 300
    assert_eq!(priced_cost(1, 0, 2_500_000, c, &TABLE), 300);
    let bound = run_cost(c.fuel, c.memory_mib, c.wall_secs, &TABLE);
    assert_eq!(priced_cost(u64::MAX, u64::MAX, u64::MAX, c, &TABLE), bound);
    assert!(priced_cost(c.fuel, 60_000_000, 60_000_000, c, &TABLE) <= bound);
}
