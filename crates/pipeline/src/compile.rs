use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::model::{ParamType, Pipeline, SpecError, Step};

pub const API_VERSION: &str = "compute.calimero.network/v1"; // the only apiVersion v1 accepts
pub const KIND: &str = "Pipeline"; // the only kind v1 accepts
pub const MAX_SOURCE_BYTES: usize = 64 * 1024; // YAML source size, checked before parsing
pub const MAX_STEPS: usize = 64; // steps per pipeline
pub const MAX_INPUTS: usize = 64; // declared inputs per pipeline
pub const MAX_PARAMS: usize = 64; // declared params per pipeline
pub const MAX_STEP_INPUTS: usize = 64; // files bound to one step, the engine's per-run input cap
pub const MAX_ITEMS: u32 = 10_000; // upper bound for any maxItems
pub const MAX_NAME_BYTES: usize = 64; // pipeline, step, input and param names
pub const MAX_ARGS_BYTES: usize = 64 * 1024; // one step's args as JSON
const NOT_IN_V1_FIELDS: [&str; 5] = ["when", "runIf", "uses", "loops", "variables"]; // refused, not ignored
const RESERVED_NAMES: [&str; 4] = ["inputs", "params", "sinks", "item"]; // would make references ambiguous

const TOP_FIELDS: [&str; 13] = [
    "apiVersion",
    "kind",
    "name",
    "module",
    "params",
    "inputs",
    "sources",
    "sinks",
    "steps",
    "publish",
    "notify",
    "outputs",
    "failFast",
];
const PARAM_FIELDS: [&str; 5] = ["type", "default", "min", "max", "enum"];
const INPUT_FIELDS: [&str; 7] = [
    "kind",
    "accept",
    "maxItems",
    "maxFileSize",
    "format",
    "schema",
    "default",
];
const CONNECTOR_FIELDS: [&str; 3] = ["connector", "uri", "connection"];
const STEP_FIELDS: [&str; 14] = [
    "module",
    "method",
    "needs",
    "inputs",
    "args",
    "forEach",
    "maxItems",
    "maxParallel",
    "batch",
    "collect",
    "allowFailures",
    "pool",
    "limits",
    "cache",
];
const LIMIT_FIELDS: [&str; 3] = ["fuel", "memoryMib", "wallSecs"];

/// Parses YAML (or JSON) into a typed pipeline and runs every structural check.
pub fn compile(source: &str) -> Result<Pipeline, Vec<SpecError>> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(vec![err(
            "",
            "TOO_LARGE",
            format!("source exceeds {MAX_SOURCE_BYTES} bytes"),
        )]);
    }
    let raw = parse_yaml(source).map_err(|m| vec![err("", "YAML_PARSE", m)])?;
    let mut errs = Vec::new();
    lint_shape(&raw, &mut errs);
    if !errs.is_empty() {
        return Err(errs);
    }
    let mut p: Pipeline = serde_path_to_error::deserialize(raw).map_err(|e| {
        vec![err(
            e.path().to_string(),
            "INVALID_TYPE",
            e.inner().to_string(),
        )]
    })?;
    validate(&mut p, &mut errs);
    if errs.is_empty() {
        Ok(p)
    } else {
        Err(errs)
    }
}

fn err(path: impl Into<String>, code: &'static str, message: impl Into<String>) -> SpecError {
    SpecError {
        path: path.into(),
        code,
        message: message.into(),
    }
}

fn join(base: &str, key: &str) -> String {
    if base.is_empty() {
        key.to_string()
    } else {
        format!("{base}.{key}")
    }
}

/// The YAML loader rejects alias bombs and duplicate keys; non-string keys fail the JSON conversion.
fn parse_yaml(source: &str) -> Result<Value, String> {
    let v: serde_norway::Value = serde_norway::from_str(source).map_err(|e| e.to_string())?;
    serde_json::to_value(v).map_err(|e| e.to_string())
}

// --- shape: required fields, unknown fields, fields that wait for v1.1 ---

fn lint_shape(root: &Value, errs: &mut Vec<SpecError>) {
    let Some(top) = strict(
        root,
        "",
        &TOP_FIELDS,
        &["apiVersion", "kind", "name", "steps"],
        errs,
    ) else {
        return;
    };
    let sections: [(&str, &[&str], &[&str]); 5] = [
        ("params", &PARAM_FIELDS, &["type"]),
        ("inputs", &INPUT_FIELDS, &["kind"]),
        ("sources", &CONNECTOR_FIELDS, &["connector", "uri"]),
        ("sinks", &CONNECTOR_FIELDS, &["connector", "uri"]),
        ("steps", &STEP_FIELDS, &["method"]),
    ];
    for (key, allowed, required) in sections {
        let Some(v) = top.get(key) else { continue };
        let Some(entries) = as_object(v, key, errs) else {
            continue;
        };
        for (name, entry) in entries {
            let path = join(key, name);
            let m = strict(entry, &path, allowed, required, errs);
            if let Some(limits) = m.and_then(|m| m.get("limits")) {
                strict(limits, &join(&path, "limits"), &LIMIT_FIELDS, &[], errs);
            }
        }
    }
    for (key, fields) in [("publish", ["from", "to"]), ("notify", ["on", "to"])] {
        let Some(Value::Array(items)) = top.get(key) else {
            continue;
        };
        for (i, item) in items.iter().enumerate() {
            strict(item, &format!("{key}[{i}]"), &fields, &fields, errs);
        }
    }
}

fn as_object<'a>(
    v: &'a Value,
    path: &str,
    errs: &mut Vec<SpecError>,
) -> Option<&'a Map<String, Value>> {
    let m = v.as_object();
    if m.is_none() {
        errs.push(err(path, "INVALID_TYPE", "expected a mapping"));
    }
    m
}

fn strict<'a>(
    v: &'a Value,
    path: &str,
    allowed: &[&str],
    required: &[&str],
    errs: &mut Vec<SpecError>,
) -> Option<&'a Map<String, Value>> {
    let m = as_object(v, path, errs)?;
    for key in m.keys() {
        if allowed.contains(&key.as_str()) {
            continue;
        }
        let (code, msg) = if NOT_IN_V1_FIELDS.contains(&key.as_str()) {
            ("NOT_IN_V1", format!("`{key}` is not part of v1"))
        } else {
            ("UNKNOWN_FIELD", format!("unknown field `{key}`"))
        };
        errs.push(err(join(path, key), code, msg));
    }
    for key in required {
        if !m.contains_key(*key) {
            errs.push(err(
                join(path, key),
                "MISSING_FIELD",
                format!("`{key}` is required"),
            ));
        }
    }
    Some(m)
}

// --- semantics ---

fn valid_name(n: &str) -> bool {
    let mut chars = n.chars();
    n.len() <= MAX_NAME_BYTES
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn check_name(n: &str, path: &str, errs: &mut Vec<SpecError>) {
    if !valid_name(n) {
        errs.push(err(
            path,
            "BAD_NAME",
            format!("`{n}` must be 1-{MAX_NAME_BYTES} chars of [a-z0-9-], starting with a letter"),
        ));
    }
}

fn validate(p: &mut Pipeline, errs: &mut Vec<SpecError>) {
    if p.api_version != API_VERSION {
        errs.push(err(
            "apiVersion",
            "BAD_API_VERSION",
            format!("expected `{API_VERSION}`"),
        ));
    }
    if p.kind != KIND {
        errs.push(err("kind", "BAD_KIND", format!("expected `{KIND}`")));
    }
    check_name(&p.name, "name", errs);
    for (section, len, max) in [
        ("steps", p.steps.len(), MAX_STEPS),
        ("inputs", p.inputs.len(), MAX_INPUTS),
        ("params", p.params.len(), MAX_PARAMS),
    ] {
        if len > max {
            errs.push(err(section, "TOO_MANY", format!("at most {max} {section}")));
        }
    }
    p.inputs.iter().for_each(|(n, i)| {
        check_name(n, &join("inputs", n), errs);
        if i.max_items.is_some_and(|m| !(1..=MAX_ITEMS).contains(&m)) {
            errs.push(err(
                format!("inputs.{n}.maxItems"),
                "BAD_MAX_ITEMS",
                range_msg(),
            ));
        }
    });
    for (n, param) in &p.params {
        check_name(n, &join("params", n), errs);
        validate_param(n, param, errs);
    }
    for n in p.sources.keys().filter(|n| !p.inputs.contains_key(*n)) {
        errs.push(err(
            join("sources", n),
            "UNRESOLVED_REF",
            "source names no declared input",
        ));
    }

    let mut deps = BTreeMap::new();
    for (n, step) in &p.steps {
        let path = join("steps", n);
        check_name(n, &path, errs);
        if RESERVED_NAMES.contains(&n.as_str()) {
            errs.push(err(&path, "RESERVED_NAME", format!("`{n}` is reserved")));
        }
        deps.insert(n.clone(), validate_step(p, step, &path, errs));
    }
    if let Some(cycle) = find_cycle(&deps) {
        let msg = format!("cycle, each step needs the next: {}", cycle.join(" -> "));
        errs.push(err(join("steps", &cycle[0]), "CYCLE", msg));
    }
    validate_outputs(p, errs);
    if errs.is_empty() {
        for (n, step) in &mut p.steps {
            step.needs = deps.remove(n).unwrap_or_default().into_iter().collect();
        }
    }
}

fn range_msg() -> String {
    format!("must be in 1..={MAX_ITEMS}")
}

fn validate_param(name: &str, p: &crate::model::Param, errs: &mut Vec<SpecError>) {
    let path = join("params", name);
    let bad = |field: &str, msg: &str, errs: &mut Vec<SpecError>| {
        errs.push(err(join(&path, field), "BAD_PARAM", msg));
    };
    let fits = |v: &Value| match p.ty {
        ParamType::Number => v.is_number(),
        ParamType::String => v.is_string(),
        ParamType::Boolean => v.is_boolean(),
    };
    if p.ty != ParamType::Number && (p.min.is_some() || p.max.is_some()) {
        bad("min", "min and max apply to number params only", errs);
    }
    if p.min.zip(p.max).is_some_and(|(lo, hi)| lo > hi) {
        bad("min", "min exceeds max", errs);
    }
    if p.values.iter().flatten().any(|v| !fits(v)) {
        bad("enum", "enum value does not match the param type", errs);
    }
    let Some(d) = &p.default else { return };
    if !fits(d) {
        bad("default", "default does not match the param type", errs);
    } else if p.values.as_ref().is_some_and(|vs| !vs.contains(d)) {
        bad("default", "default is not in enum", errs);
    } else if d
        .as_f64()
        .is_some_and(|n| p.min.is_some_and(|lo| n < lo) || p.max.is_some_and(|hi| n > hi))
    {
        bad("default", "default is outside min..max", errs);
    }
}

enum Ref<'a> {
    Input(&'a str),
    Param(&'a str),
    Item,
    Step(&'a str, &'a str),
}

fn parse_ref(s: &str) -> Option<Ref<'_>> {
    if s == "item" {
        return Some(Ref::Item);
    }
    match s.split_once('.')? {
        ("inputs", n) => Some(Ref::Input(n)),
        ("params", n) => Some(Ref::Param(n)),
        (step, field @ ("outputs" | "results" | "result")) => Some(Ref::Step(step, field)),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Site {
    ForEach,
    Inputs,
    Collect,
    Args,
    Outputs,
}

/// Resolves one reference, returning the step it points at when it is a step reference.
fn resolve<'a>(
    p: &Pipeline,
    site: Site,
    in_foreach: bool,
    s: &'a str,
    path: &str,
    errs: &mut Vec<SpecError>,
) -> Option<&'a str> {
    let Some(r) = parse_ref(s) else {
        errs.push(err(path, "BAD_REF", format!("`{s}` is not a reference")));
        return None;
    };
    let allowed = matches!(
        (&r, site),
        (Ref::Input(_), Site::ForEach | Site::Inputs)
            | (Ref::Param(_), Site::Args)
            | (Ref::Item, Site::Inputs | Site::Args)
            | (Ref::Step(_, "outputs" | "results"), Site::ForEach)
            | (
                Ref::Step(_, "outputs"),
                Site::Inputs | Site::Collect | Site::Outputs
            )
            | (Ref::Step(_, "results" | "result"), Site::Args)
    );
    if !allowed {
        errs.push(err(path, "BAD_REF", format!("`{s}` is not allowed here")));
        return None;
    }
    let missing = |what: &str, errs: &mut Vec<SpecError>| {
        errs.push(err(
            path,
            "UNRESOLVED_REF",
            format!("`{s}` names no declared {what}"),
        ));
    };
    match r {
        Ref::Input(n) if !p.inputs.contains_key(n) => missing("input", errs),
        Ref::Param(n) if !p.params.contains_key(n) => missing("param", errs),
        Ref::Item if !in_foreach => {
            errs.push(err(
                path,
                "ITEM_OUTSIDE_FOREACH",
                "`item` is only valid in a forEach step",
            ));
        }
        Ref::Step(n, _) if !p.steps.contains_key(n) => missing("step", errs),
        Ref::Step(n, _) => return Some(n),
        _ => {}
    }
    None
}

/// Walks args; any string shaped like a reference must resolve (a literal such as "item" is refused).
fn resolve_args(
    p: &Pipeline,
    in_foreach: bool,
    v: &Value,
    path: &str,
    deps: &mut BTreeSet<String>,
    errs: &mut Vec<SpecError>,
) {
    match v {
        Value::String(s) if parse_ref(s).is_some() => {
            deps.extend(resolve(p, Site::Args, in_foreach, s, path, errs).map(String::from));
        }
        Value::Array(items) => items
            .iter()
            .enumerate()
            .for_each(|(i, x)| resolve_args(p, in_foreach, x, &format!("{path}[{i}]"), deps, errs)),
        Value::Object(m) => m
            .iter()
            .for_each(|(k, x)| resolve_args(p, in_foreach, x, &join(path, k), deps, errs)),
        _ => {}
    }
}

/// Checks one step and returns the steps it depends on (its `needs` plus every reference).
fn validate_step(
    p: &Pipeline,
    s: &Step,
    path: &str,
    errs: &mut Vec<SpecError>,
) -> BTreeSet<String> {
    let mut deps = BTreeSet::new();
    let at = |field: &str| join(path, field);
    let fan_out = s.for_each.is_some();

    if s.module.is_none() && p.module.is_none() {
        errs.push(err(
            at("module"),
            "MISSING_MODULE",
            "no step module and no pipeline module",
        ));
    }
    for n in &s.needs {
        if p.steps.contains_key(n) {
            deps.insert(n.clone());
        } else {
            errs.push(err(
                at("needs"),
                "UNKNOWN_NEED",
                format!("`{n}` is not a step"),
            ));
        }
    }
    if let Some(r) = &s.for_each {
        deps.extend(resolve(p, Site::ForEach, false, r, &at("forEach"), errs).map(String::from));
        match s.max_items {
            None => errs.push(err(
                at("maxItems"),
                "MISSING_MAX_ITEMS",
                "forEach requires maxItems",
            )),
            Some(m) if !(1..=MAX_ITEMS).contains(&m) => {
                errs.push(err(at("maxItems"), "BAD_MAX_ITEMS", range_msg()));
            }
            Some(_) => {}
        }
        if s.batch == Some(0) {
            errs.push(err(at("batch"), "BAD_BATCH", "batch must be at least 1"));
        }
        if s.max_parallel == Some(0) {
            errs.push(err(
                at("maxParallel"),
                "BAD_MAX_PARALLEL",
                "maxParallel must be at least 1",
            ));
        }
    } else {
        for (field, set) in [
            ("maxItems", s.max_items.is_some()),
            ("maxParallel", s.max_parallel.is_some()),
            ("batch", s.batch.is_some()),
        ] {
            if set {
                errs.push(err(
                    at(field),
                    "FOREACH_REQUIRED",
                    format!("`{field}` needs forEach"),
                ));
            }
        }
        if s.allow_failures.is_some() {
            errs.push(err(
                at("allowFailures"),
                "ALLOW_FAILURES_NOT_FOREACH",
                "allowFailures needs forEach",
            ));
        }
    }
    if s.inputs.len() > MAX_STEP_INPUTS {
        errs.push(err(
            at("inputs"),
            "TOO_MANY",
            format!("at most {MAX_STEP_INPUTS} inputs"),
        ));
    }
    for (i, r) in s.inputs.iter().enumerate() {
        let found = resolve(
            p,
            Site::Inputs,
            fan_out,
            r,
            &format!("{}[{i}]", at("inputs")),
            errs,
        );
        deps.extend(found.map(String::from));
    }
    if let Some(r) = &s.collect {
        if fan_out {
            errs.push(err(
                at("collect"),
                "COLLECT_WITH_FOREACH",
                "a step cannot both fan out and collect",
            ));
        }
        if let Some(target) = resolve(p, Site::Collect, false, r, &at("collect"), errs) {
            if p.steps[target].for_each.is_none() {
                errs.push(err(
                    at("collect"),
                    "COLLECT_TARGET",
                    format!("`{target}` is not a forEach step"),
                ));
            }
            deps.insert(target.to_string());
        }
    }
    for (k, v) in &s.args {
        resolve_args(p, fan_out, v, &join(&at("args"), k), &mut deps, errs);
    }
    if serde_json::to_vec(&s.args).map_or(true, |b| b.len() > MAX_ARGS_BYTES) {
        errs.push(err(
            at("args"),
            "ARGS_TOO_LARGE",
            format!("args exceed {MAX_ARGS_BYTES} bytes"),
        ));
    }
    if let Some(l) = &s.limits {
        let zero = [
            ("fuel", l.fuel == Some(0)),
            ("memoryMib", l.memory_mib == Some(0)),
            ("wallSecs", l.wall_secs == Some(0)),
        ];
        for (field, _) in zero.into_iter().filter(|(_, z)| *z) {
            errs.push(err(
                join(&at("limits"), field),
                "BAD_LIMIT",
                "a limit must be positive",
            ));
        }
    }
    deps
}

fn validate_outputs(p: &Pipeline, errs: &mut Vec<SpecError>) {
    for (i, r) in p.outputs.iter().enumerate() {
        resolve(p, Site::Outputs, false, r, &format!("outputs[{i}]"), errs);
    }
    for (i, publish) in p.publish.iter().enumerate() {
        resolve(
            p,
            Site::Outputs,
            false,
            &publish.from,
            &format!("publish[{i}].from"),
            errs,
        );
        let path = format!("publish[{i}].to");
        match publish.to.strip_prefix("sinks.") {
            Some(sink) if p.sinks.contains_key(sink) => {}
            Some(_) => errs.push(err(
                path,
                "UNRESOLVED_REF",
                format!("`{}` names no declared sink", publish.to),
            )),
            None => errs.push(err(path, "BAD_REF", "expected `sinks.<name>`")),
        }
    }
}

/// First cycle found walking needs edges, as `a -> b -> a`.
fn find_cycle(deps: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    fn visit<'a>(
        n: &'a str,
        deps: &'a BTreeMap<String, BTreeSet<String>>,
        done: &mut BTreeSet<&'a str>,
        stack: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        stack.push(n);
        for d in deps[n].iter().filter(|d| deps.contains_key(*d)) {
            if let Some(at) = stack.iter().position(|s| s == d) {
                let mut c: Vec<String> = stack[at..].iter().map(|s| s.to_string()).collect();
                c.push(d.clone());
                return Some(c);
            }
            if !done.contains(d.as_str()) {
                if let Some(c) = visit(d, deps, done, stack) {
                    return Some(c);
                }
            }
        }
        stack.pop();
        done.insert(n);
        None
    }
    let mut done = BTreeSet::new();
    deps.keys().find_map(|n| {
        if done.contains(n.as_str()) {
            None
        } else {
            visit(n, deps, &mut done, &mut Vec::new())
        }
    })
}
