//! Shared scenario files: neutral JSON steps both the Rust and TypeScript harnesses run.
//!
//! ```json
//! [{ "as": "bob", "op": "call", "method": "book", "args": {..},
//!    "expect": { "ok": <value> } | { "error": "Taken" } | { "trap": "OutOfFuel" },
//!    "events": { "bob": ["Booked"] } }]
//! ```
//!
//! `as` is an account, `"@anonymous"` or `"@system"`; `"confirmed": true` adds the step-up.
//! `op` is `call`, `view`, `tick` (`method` names the timer), `advance` (`args` is the ms)
//! or `removeMember` (`args` is the account). After a step, `events` lists for each account
//! the names of all events it receives so far, in order.

use std::path::Path;

use eyre::{bail, eyre, Result, WrapErr};
use serde_json::Value;

use super::{Failure, TestApp};

/// Runs the scenario file against `t`.
///
/// # Errors
///
/// A malformed scenario, or the first step whose expectation does not hold.
pub fn run_scenario(t: &mut TestApp, path: &Path) -> Result<()> {
    let text =
        std::fs::read_to_string(path).wrap_err_with(|| format!("read {}", path.display()))?;
    let steps: Vec<Value> = serde_json::from_str(&text).wrap_err("scenario is not a JSON array")?;
    for (n, step) in steps.iter().enumerate() {
        run_step(t, step).wrap_err_with(|| format!("step {n}: {step}"))?;
    }
    Ok(())
}

fn run_step(t: &mut TestApp, step: &Value) -> Result<()> {
    let op = str_of(step, "op")?;
    let args = step.get("args").cloned().unwrap_or(Value::Null);
    let result = match op {
        "call" | "view" => {
            let method = str_of(step, "method")?;
            let who = step.get("as").and_then(Value::as_str).unwrap_or("owner");
            let mut caller = match who {
                "@anonymous" => t.anonymous(),
                "@system" => t.system(),
                account => t.as_(account),
            };
            if step.get("confirmed").and_then(Value::as_bool) == Some(true) {
                caller = caller.confirmed();
            }
            if op == "call" {
                caller.call(method, args)
            } else {
                caller.view(method, args)
            }
        }
        "tick" => t.tick(str_of(step, "method")?),
        "advance" => {
            t.advance(
                args.as_u64()
                    .ok_or_else(|| eyre!("advance needs ms in args"))?,
            );
            Ok(Value::Null)
        }
        "removeMember" => {
            t.remove_member(
                args.as_str()
                    .ok_or_else(|| eyre!("removeMember needs an account"))?,
            );
            Ok(Value::Null)
        }
        other => bail!("unknown op {other:?}"),
    };
    check_expect(step.get("expect"), &result)?;
    if let Some(Value::Object(per_account)) = step.get("events") {
        for (account, names) in per_account {
            let want: Vec<&str> = names
                .as_array()
                .ok_or_else(|| eyre!("events.{account} is not a list"))?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let got = t.events_for(account);
            let got: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
            if got != want {
                bail!("events for {account}: expected {want:?}, got {got:?}");
            }
        }
    }
    Ok(())
}

fn check_expect(expect: Option<&Value>, result: &Result<Value, Failure>) -> Result<()> {
    let Some(expect) = expect else {
        return match result {
            Ok(_) => Ok(()),
            Err(f) => bail!("expected success, got {f:?}"),
        };
    };
    if let Some(want) = expect.get("ok") {
        return match result {
            Ok(got) if got == want => Ok(()),
            other => bail!("expected ok {want}, got {other:?}"),
        };
    }
    if let Some(code) = expect.get("error").and_then(Value::as_str) {
        return match result {
            Err(Failure::App { code: got, .. }) if got == code => Ok(()),
            other => bail!("expected error {code}, got {other:?}"),
        };
    }
    if let Some(kind) = expect.get("trap").and_then(Value::as_str) {
        return match result {
            Err(Failure::Trap(got)) if format!("{got:?}") == kind => Ok(()),
            other => bail!("expected trap {kind}, got {other:?}"),
        };
    }
    bail!("expect needs ok, error or trap")
}

fn str_of<'v>(step: &'v Value, key: &str) -> Result<&'v str> {
    step.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| eyre!("step needs a string {key:?}"))
}
