use crate::model::{Caps, Env, Pipeline, SpecError, Step};

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

/// Most fuel the pipeline can burn: each step's fuel cap times its maximum runs.
/// A step whose module or method is unknown adds nothing; `check` rejects it.
pub fn worst_case_fuel(p: &Pipeline, env: &Env) -> u128 {
    p.steps
        .values()
        .filter_map(|s| {
            let cap = caps_of(p, s, env)?;
            let fuel = s.limits.and_then(|l| l.fuel).unwrap_or(cap.fuel);
            let runs = match s.for_each {
                None => 1,
                Some(_) => {
                    let items = u128::from(s.max_items.unwrap_or(0));
                    items.div_ceil(u128::from(s.batch.unwrap_or(1).max(1)))
                }
            };
            Some(u128::from(fuel) * runs)
        })
        .sum()
}
