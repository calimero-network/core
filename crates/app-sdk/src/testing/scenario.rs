//! Shared scenario files: neutral JSON steps both the Rust and TypeScript harnesses run.
//!
//! ```json
//! [{ "as": "bob", "op": "call", "method": "book", "args": {..},
//!    "expect": { "ok": <value> } | { "error": "Taken" } | { "trap": "OutOfFuel" },
//!    "events": { "bob": ["Booked"] } }]
//! ```
//!
//! `as` is an account, `"@anonymous"` or `"@system"`; `"confirmed": true` adds the step-up.
//! `expect` may also be `{ "confirmRequired": true }`, `{ "error": code, "fields": {..} }` (fields compared exactly)
//! or `{ "refused": "<API code>" }`: the platform refused before the app ran, with the node's code
//! (`UNAUTHENTICATED` for an anonymous caller of a non-public method, `NOT_FOUND` for an account the
//! app's org does not hold; Ruling 32).
//! A `tick` always runs as the system: `as` on a tick is refused.
//! `op` is `call`, `view`, `tick` (`method` names the timer), `advance` (`args` is the ms)
//! or `removeMember` (`args` is the account).
//!
//! Variables: `"save": "name"` on a call or view stores its ok result; later, any string
//! that is exactly `"$"` plus a name (`[A-Za-z_][A-Za-z0-9_]*`) inside `args` or `expect` is replaced by the stored value
//! (ids an app mints are not known to the scenario author). Any other string, such as
//! `"$5 lunch"`, is literal; there is no escape, so a literal that looks like `$name` is not expressible.
//! Accounts: a string that is exactly `"@"` plus a name (same pattern) inside `args` or `expect` is that
//! account as the app sees it: here its name (`"@bob"` is `"bob"`); on a cluster, its account id.
//! A substituted value is not substituted again.
//! `save` on `tick`, `advance` or `removeMember` is an error. After a step, `events` lists for each account
//! the names of all events it receives so far, in order.

use std::collections::HashMap;
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
    let mut vars = HashMap::new();
    for (n, step) in steps.iter().enumerate() {
        run_step(t, step, &mut vars).wrap_err_with(|| format!("step {n}: {step}"))?;
    }
    Ok(())
}

type Vars = HashMap<String, Value>;

/// `[A-Za-z_][A-Za-z0-9_]*`.
fn is_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Replaces every string that is exactly `$name` by the saved value of `name`, and every
/// string that is exactly `@name` by the account `name` (in the harness, the name itself).
fn substitute(value: &Value, vars: &Vars) -> Result<Value> {
    Ok(match value {
        Value::String(s) => {
            if let Some(name) = s.strip_prefix('$').filter(|n| is_var_name(n)) {
                vars.get(name)
                    .cloned()
                    .ok_or_else(|| eyre!("unknown variable ${name}"))?
            } else if let Some(account) = s.strip_prefix('@').filter(|n| is_var_name(n)) {
                Value::String(account.to_owned())
            } else {
                value.clone()
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| substitute(v, vars))
                .collect::<Result<_>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), substitute(v, vars)?)))
                .collect::<Result<_>>()?,
        ),
        other => other.clone(),
    })
}

fn run_step(t: &mut TestApp, step: &Value, vars: &mut Vars) -> Result<()> {
    let op = str_of(step, "op")?;
    if step.get("save").is_some() && !matches!(op, "call" | "view") {
        bail!("`save` only applies to a call or view, not {op:?}");
    }
    let args = substitute(step.get("args").unwrap_or(&Value::Null), vars)?;
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
        "tick" => {
            if step.get("as").is_some() {
                bail!("a tick always runs as the system; drop `as`");
            }
            t.tick(str_of(step, "method")?)
        }
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
    let expect = step
        .get("expect")
        .map(|e| substitute(e, vars))
        .transpose()?;
    check_expect(expect.as_ref(), &result)?;
    if let (Some(name), Ok(saved)) = (step.get("save").and_then(Value::as_str), &result) {
        vars.insert(name.to_owned(), saved.clone());
    }
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
    if expect.get("confirmRequired").and_then(Value::as_bool) == Some(true) {
        return match result {
            Err(Failure::ConfirmRequired) => Ok(()),
            other => bail!("expected confirmRequired, got {other:?}"),
        };
    }
    if let Some(code) = expect.get("error").and_then(Value::as_str) {
        return match result {
            Err(Failure::App { code: got, fields }) if got == code => match expect.get("fields") {
                Some(want) if want != fields => {
                    bail!("expected error {code} with fields {want}, got {fields}")
                }
                _ => Ok(()),
            },
            other => bail!("expected error {code}, got {other:?}"),
        };
    }
    if let Some(code) = expect.get("refused").and_then(Value::as_str) {
        return match result {
            Err(Failure::Refused { code: got, .. }) if got == code => Ok(()),
            other => bail!("expected refused {code}, got {other:?}"),
        };
    }
    if let Some(kind) = expect.get("trap").and_then(Value::as_str) {
        return match result {
            Err(Failure::Trap(got)) if format!("{got:?}") == kind => Ok(()),
            other => bail!("expected trap {kind}, got {other:?}"),
        };
    }
    bail!("expect needs ok, error, trap, refused or confirmRequired")
}

fn str_of<'v>(step: &'v Value, key: &str) -> Result<&'v str> {
    step.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| eyre!("step needs a string {key:?}"))
}
