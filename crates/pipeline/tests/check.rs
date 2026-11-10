use std::collections::BTreeMap;

use calimero_pipeline::{check, compile, worst_case_fuel, Caps, Env};

const HEAD: &str = "apiVersion: compute.calimero.network/v1\nkind: Pipeline\nname: p\nmodule: m\n";

const CAPS: Caps = Caps {
    fuel: 1_000,
    memory_mib: 256,
    wall_secs: 60,
};

fn env(pools: &[(&str, u32)]) -> Env {
    Env {
        modules: BTreeMap::from([(
            "m".to_string(),
            BTreeMap::from([
                ("run".to_string(), CAPS),
                (
                    "big".to_string(),
                    Caps {
                        memory_mib: 4096,
                        ..CAPS
                    },
                ),
            ]),
        )]),
        pools: pools.iter().map(|(n, m)| (n.to_string(), *m)).collect(),
        ..Env::default()
    }
}

fn pipeline(steps: &str) -> calimero_pipeline::Pipeline {
    compile(&format!(
        "{HEAD}inputs:\n  x: {{ kind: files }}\nsteps:\n{steps}"
    ))
    .unwrap()
}

fn codes(steps: &str, env: &Env) -> Vec<(&'static str, String)> {
    check(&pipeline(steps), env)
        .expect_err("expected check errors")
        .into_iter()
        .map(|e| (e.code, e.path))
        .collect()
}

#[test]
fn clean_pipeline_passes() {
    check(
        &pipeline("  a: { method: run, limits: { fuel: 10, memoryMib: 8, wallSecs: 1 } }\n"),
        &env(&[("standard", 512)]),
    )
    .unwrap();
}

#[test]
fn each_apply_check_has_a_code_and_path() {
    let e = env(&[("standard", 512), ("small", 64)]);
    let cases = [
        (
            "  a: { module: other, method: run }\n",
            "UNKNOWN_MODULE",
            "steps.a.module",
        ),
        (
            "  a: { method: ghost }\n",
            "UNKNOWN_METHOD",
            "steps.a.method",
        ),
        (
            "  a: { method: run, limits: { fuel: 1001 } }\n",
            "LIMIT_EXCEEDS_CAP",
            "steps.a.limits.fuel",
        ),
        (
            "  a: { method: run, limits: { memoryMib: 257 } }\n",
            "LIMIT_EXCEEDS_CAP",
            "steps.a.limits.memoryMib",
        ),
        (
            "  a: { method: run, limits: { wallSecs: 61 } }\n",
            "LIMIT_EXCEEDS_CAP",
            "steps.a.limits.wallSecs",
        ),
        (
            "  a: { method: run, pool: ghost }\n",
            "UNKNOWN_POOL",
            "steps.a.pool",
        ),
        (
            "  a: { method: run, pool: small }\n",
            "POOL_MEMORY",
            "steps.a.pool",
        ),
        ("  a: { method: big }\n", "POOL_MEMORY", "steps.a.pool"),
    ];
    for (steps, code, path) in cases {
        assert_eq!(codes(steps, &e), [(code, path.to_string())], "{steps}");
    }
}

#[test]
fn limits_lower_memory_can_fit_a_small_pool() {
    let e = env(&[("standard", 512), ("small", 64)]);
    check(
        &pipeline("  a: { method: run, pool: small, limits: { memoryMib: 64 } }\n"),
        &e,
    )
    .unwrap();
}

#[test]
fn pools_are_ignored_when_the_env_has_none() {
    check(&pipeline("  a: { method: big, pool: ghost }\n"), &env(&[])).unwrap();
}

#[test]
fn a_step_without_pool_runs_in_any_pool_that_fits() {
    let e = env(&[("standard", 512), ("highmem", 8192)]);
    check(
        &pipeline("  a: { method: big }\n  b: { method: run }\n"),
        &e,
    )
    .unwrap();
}

#[test]
fn every_error_is_reported() {
    let e = env(&[("standard", 512)]);
    assert_eq!(
        codes(
            "  a: { method: ghost }\n  b: { module: nope, method: run }\n",
            &e
        )
        .len(),
        2
    );
}

#[test]
fn worst_case_fuel_is_cap_times_runs() {
    let e = env(&[]);
    let p = pipeline(
        "  a: { method: run }\n  b: { method: run, forEach: inputs.x, maxItems: 100, batch: 25, limits: { fuel: 10 } }\n  c: { method: run, forEach: inputs.x, maxItems: 10 }\n  d: { method: run, forEach: inputs.x, maxItems: 7, batch: 2 }\n",
    );
    // a: 1000, b: 10 * 4, c: 1000 * 10, d: 1000 * ceil(7 / 2)
    assert_eq!(worst_case_fuel(&p, &e), 1_000 + 40 + 10_000 + 4_000);
}

#[test]
fn worst_case_fuel_cannot_overflow_and_skips_unknown_methods() {
    let mut e = env(&[]);
    e.modules.get_mut("m").unwrap().insert(
        "max".into(),
        Caps {
            fuel: u64::MAX,
            ..CAPS
        },
    );
    let p = pipeline(
        "  a: { method: max, forEach: inputs.x, maxItems: 10000 }\n  b: { method: max, forEach: inputs.x, maxItems: 10000 }\n  c: { method: ghost }\n",
    );
    assert_eq!(worst_case_fuel(&p, &e), 2 * u128::from(u64::MAX) * 10_000);
}

#[test]
fn a_step_without_any_module_is_missing_one_unless_the_env_has_a_default() {
    let source = "apiVersion: compute.calimero.network/v1\nkind: Pipeline\nname: p\nsteps:\n  a: { method: run }\n";
    let p = compile(source).unwrap();
    let mut e = env(&[]);
    let errs = check(&p, &e).unwrap_err();
    assert_eq!(
        (errs[0].code, errs[0].path.as_str()),
        ("MISSING_MODULE", "steps.a.module")
    );
    e.default_module = Some("m".into());
    check(&p, &e).unwrap();
    assert_eq!(worst_case_fuel(&p, &e), 1_000);
}
