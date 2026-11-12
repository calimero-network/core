use std::collections::BTreeMap;

use crate::model::{Caps, Env, Pipeline, PriceTable, SpecError, Step};

fn module_of<'a>(p: &'a Pipeline, s: &'a Step, env: &'a Env) -> Option<&'a str> {
    s.module
        .as_deref()
        .or(p.module.as_deref())
        .or(env.default_module.as_deref())
}

fn caps_of<'a>(p: &Pipeline, s: &Step, env: &'a Env) -> Option<&'a Caps> {
    env.modules.get(module_of(p, s, env)?)?.get(&s.method)
}

/// Apply-time checks: modules, methods, limits against caps, pools and their memory.
pub fn check(p: &Pipeline, env: &Env) -> Result<(), Vec<SpecError>> {
    let mut errs = Vec::new();
    for (name, s) in &p.steps {
        let at = |field: &str| format!("steps.{name}.{field}");
        let mut push = |field: &str, code: &'static str, message: String| {
            errs.push(SpecError {
                path: at(field),
                code,
                message,
            });
        };
        let caps = match module_of(p, s, env).and_then(|m| env.modules.get(m).map(|ms| (m, ms))) {
            None => {
                match module_of(p, s, env) {
                    Some(m) => push(
                        "module",
                        "UNKNOWN_MODULE",
                        format!("module `{m}` is not available"),
                    ),
                    None => push(
                        "module",
                        "MISSING_MODULE",
                        "no step module, no pipeline module and no default".into(),
                    ),
                }
                None
            }
            Some((m, methods)) => match methods.get(&s.method) {
                None => {
                    push(
                        "method",
                        "UNKNOWN_METHOD",
                        format!("module `{m}` has no method `{}`", s.method),
                    );
                    None
                }
                Some(c) => Some(*c),
            },
        };
        let limits = s.limits.unwrap_or_default();
        if let Some(c) = caps {
            let over = [
                ("fuel", limits.fuel.is_some_and(|v| v > c.fuel)),
                (
                    "memoryMib",
                    limits.memory_mib.is_some_and(|v| v > c.memory_mib),
                ),
                (
                    "wallSecs",
                    limits.wall_secs.is_some_and(|v| v > c.wall_secs),
                ),
            ];
            for (field, _) in over.into_iter().filter(|(_, o)| *o) {
                push(
                    &format!("limits.{field}"),
                    "LIMIT_EXCEEDS_CAP",
                    "limits may only lower the method's caps".into(),
                );
            }
        }
        if env.pools.is_empty() {
            continue;
        }
        let memory = limits
            .memory_mib
            .or(caps.map(|c| c.memory_mib))
            .unwrap_or(0);
        // Without `pool`, the run queue places a run in any pool whose memory fits it.
        match s.pool.as_deref() {
            None if !env.pools.values().any(|&max| max >= memory) => push(
                "pool",
                "POOL_MEMORY",
                format!("needs {memory} MiB and no pool allows that much"),
            ),
            None => {}
            Some(pool) => match env.pools.get(pool) {
                None => push(
                    "pool",
                    "UNKNOWN_POOL",
                    format!("pool `{pool}` does not exist"),
                ),
                Some(&max) if memory > max => push(
                    "pool",
                    "POOL_MEMORY",
                    format!("needs more memory than pool `{pool}` allows ({max} MiB)"),
                ),
                Some(_) => {}
            },
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// Runs a step makes at most: one, or its `forEach` items over its batch size. A `forEach` over
/// `inputs.x` with a known file count in `counts` uses that count, capped at `maxItems`.
fn max_runs(s: &Step, counts: &BTreeMap<String, u32>) -> u128 {
    let Some(source) = &s.for_each else { return 1 };
    let max = s.max_items.unwrap_or(0);
    let items = source
        .strip_prefix("inputs.")
        .and_then(|name| counts.get(name))
        .map_or(max, |&n| n.min(max));
    u128::from(items).div_ceil(u128::from(s.batch.unwrap_or(1).max(1)))
}

/// What one step is expected to make: its runs, their fuel caps summed and their priced cost.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StepEstimate {
    pub runs: u128,
    pub fuel: u128,
    pub cost: Option<u128>,
}

/// A pipeline's fuel and cost at its caps, for known input file counts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Estimate {
    pub steps: BTreeMap<String, StepEstimate>,
    pub fuel: u128,
    pub cost: Option<u128>,
}

/// Fuel and cost of a run with `counts` files per `forEach` input (a missing count takes
/// `maxItems`); cost is `None` without a table. A step whose module or method is unknown adds
/// nothing; `check` rejects it.
pub fn estimate(
    p: &Pipeline,
    env: &Env,
    counts: &BTreeMap<String, u32>,
    table: Option<&PriceTable>,
) -> Estimate {
    let mut out = Estimate {
        steps: BTreeMap::new(),
        fuel: 0,
        cost: table.map(|_| 0),
    };
    for (name, s) in &p.steps {
        let Some(cap) = caps_of(p, s, env) else {
            continue;
        };
        let runs = max_runs(s, counts);
        let l = s.limits.unwrap_or_default();
        let fuel_cap = l.fuel.unwrap_or(cap.fuel);
        let fuel = u128::from(fuel_cap).saturating_mul(runs);
        let cost = table.map(|t| {
            run_cost(
                fuel_cap,
                l.memory_mib.unwrap_or(cap.memory_mib),
                l.wall_secs.unwrap_or(cap.wall_secs),
                t,
            )
            .saturating_mul(runs)
        });
        out.fuel = out.fuel.saturating_add(fuel);
        out.cost = out.cost.zip(cost).map(|(a, b)| a.saturating_add(b));
        let _prev = out
            .steps
            .insert(name.clone(), StepEstimate { runs, fuel, cost });
    }
    out
}

/// Most fuel the pipeline can burn: each step's fuel cap times its maximum runs.
/// A step whose module or method is unknown adds nothing; `check` rejects it.
pub fn worst_case_fuel(p: &Pipeline, env: &Env) -> u128 {
    estimate(p, env, &BTreeMap::new(), None).fuel
}

/// Upper bound for one run: vCPU-seconds from its fuel cap plus GiB-seconds of its memory and
/// wall limits, each rounded up; saturates at `u128::MAX`. A zero fuel rate is read as 1; `PriceTable::validate` refuses it.
pub fn run_cost(fuel: u64, memory_mib: u32, wall_secs: u32, t: &PriceTable) -> u128 {
    let vcpu_secs = u128::from(fuel).div_ceil(u128::from(t.fuel_per_vcpu_second.max(1)));
    let gib_secs = (u128::from(memory_mib) * u128::from(wall_secs)).div_ceil(1024);
    (vcpu_secs * u128::from(t.vcpu_second)).saturating_add(gib_secs * u128::from(t.gib_second))
}

/// What a run actually used, priced: the larger of its fuel's and its CPU time's vCPU-seconds
/// plus its GiB-seconds, each rounded up, and never above `run_cost` of its caps.
pub fn priced_cost(
    fuel: u64,
    gib_micros: u64,
    cpu_micros: u64,
    caps: Caps,
    t: &PriceTable,
) -> u128 {
    let by_fuel = u128::from(fuel).div_ceil(u128::from(t.fuel_per_vcpu_second.max(1)));
    let by_cpu = u128::from(cpu_micros).div_ceil(1_000_000);
    let gib_secs = u128::from(gib_micros).div_ceil(1_000_000);
    let used = (by_fuel.max(by_cpu) * u128::from(t.vcpu_second))
        .saturating_add(gib_secs * u128::from(t.gib_second));
    used.min(run_cost(caps.fuel, caps.memory_mib, caps.wall_secs, t))
}

/// Most the pipeline can cost: each step's run cost at its limits (or the method's caps) times
/// its maximum runs. A step whose module or method is unknown adds nothing.
pub fn worst_case_cost(p: &Pipeline, env: &Env, t: &PriceTable) -> u128 {
    estimate(p, env, &BTreeMap::new(), Some(t))
        .cost
        .unwrap_or(0)
}
