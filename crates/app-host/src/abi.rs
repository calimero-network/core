//! Method attributes from the bundle's signed ABI; the host checks them before `handle`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::engine::PrepareError;

/// ABI versions this host reads. A bundle declares its own in `abiVersion`.
pub const SUPPORTED_ABI_VERSIONS: &[u64] = &[1];

/// Most fuel a compute method may declare; about the wall cap of CPU at 1 G fuel per second.
pub const MAX_COMPUTE_FUEL: u64 = 1_000_000_000_000;
/// Most memory a compute method may declare.
pub const MAX_COMPUTE_MEMORY_MIB: u32 = 4_096;
/// Most wall time a compute method may declare.
pub const MAX_COMPUTE_WALL_SECS: u32 = 900;

/// What a method is for; timers are the only methods with an interval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MethodKind {
    Init,
    Call,
    View,
    Timer,
    /// Runs on a job node under these caps; only the node starts it.
    Compute(ComputeCaps),
}

/// What a compute method declares: its budget and the method that receives its outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComputeCaps {
    pub fuel: u64,
    pub memory_mib: u32,
    pub wall_secs: u32,
    /// The ABI name of the `computeResult` method.
    pub result: String,
}

/// One method's attributes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodAbi {
    pub kind: MethodKind,
    /// Needs the owner's passkey confirmation; never runs as a view.
    pub confirm: bool,
    /// Callable without signing in.
    pub public: bool,
    /// A timer's interval (D34); `Some` exactly for timers.
    pub every: Option<Duration>,
    /// Accepts `Caller::System`.
    pub allow_system: bool,
    /// Receives a compute run's outcome; only the node invokes it.
    pub compute_result: bool,
}

/// The methods an app declares, by ABI (camelCase) name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Abi {
    pub methods: BTreeMap<String, MethodAbi>,
}

impl Abi {
    /// Reads the `calimero:abi` JSON. `abiVersion` is required and must be supported.
    /// Unknown keys, top level or per method, are ignored so newer builds stay readable
    /// (Ruling 14); a known key with a value of the wrong shape is refused.
    pub fn from_json(json: &Value) -> Result<Self, PrepareError> {
        let top = object(json, "abi")?;
        let version = match top.get("abiVersion") {
            None => return Err(PrepareError::MissingAbiVersion),
            Some(Value::Number(n)) => n
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or_else(|| bad(format!("abiVersion is not a positive integer: {n}")))?,
            Some(other) => return Err(bad(format!("abiVersion is not a number: {other}"))),
        };
        if !SUPPORTED_ABI_VERSIONS.contains(&version) {
            return Err(PrepareError::UnsupportedAbiVersion(version));
        }
        let Some(methods) = top.get("methods") else {
            return Err(bad("abi has no methods"));
        };
        let methods = object(methods, "methods")?
            .iter()
            .map(|(name, method)| Ok((name.clone(), method_abi(name, method)?)))
            .collect::<Result<BTreeMap<_, _>, PrepareError>>()?;
        check_compute(&methods)?;
        Ok(Self { methods })
    }
}

fn method_abi(name: &str, json: &Value) -> Result<MethodAbi, PrepareError> {
    let fields = object(json, name)?;
    let kind = match fields.get("kind").and_then(Value::as_str) {
        Some("init") => MethodKind::Init,
        Some("call") => MethodKind::Call,
        Some("view") => MethodKind::View,
        Some("timer") => MethodKind::Timer,
        Some("compute") => MethodKind::Compute(compute_caps(fields, name)?),
        other => return Err(bad(format!("{name}: unknown kind {other:?}"))),
    };
    let every = match fields.get("every") {
        None | Some(Value::Null) => None,
        Some(Value::String(every)) => {
            Some(interval(every).ok_or_else(|| bad(format!("{name}: bad every {every:?}")))?)
        }
        Some(other) => return Err(bad(format!("{name}: bad every {other}"))),
    };
    if !matches!(kind, MethodKind::Compute(_)) && fields.contains_key("compute") {
        return Err(bad(format!(
            "{name}: `compute` is only for compute methods"
        )));
    }
    let compute_result = flag(fields, "computeResult", name)?;
    if compute_result && kind != MethodKind::Call {
        return Err(bad(format!(
            "{name}: `computeResult` is only for call methods"
        )));
    }
    if (kind == MethodKind::Timer) != every.is_some() {
        return Err(bad(format!(
            "{name}: `every` is required on timers and refused elsewhere"
        )));
    }
    Ok(MethodAbi {
        kind,
        confirm: flag(fields, "confirm", name)?,
        public: flag(fields, "public", name)?,
        every,
        allow_system: flag(fields, "allowSystem", name)?,
        compute_result,
    })
}

/// The `compute` object of a compute method; fuel is required, memory and wall default to the maxima.
fn compute_caps(fields: &Map<String, Value>, name: &str) -> Result<ComputeCaps, PrepareError> {
    let caps = fields
        .get("compute")
        .ok_or_else(|| bad(format!("{name}: a compute method declares `compute`")))?;
    let caps = object(caps, name)?;
    let number = |key: &str, default: Option<u64>| match caps.get(key) {
        None => default.ok_or_else(|| bad(format!("{name}: compute.{key} is required"))),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| bad(format!("{name}: compute.{key} is not an integer: {value}"))),
    };
    let fuel = number("fuel", None)?;
    let memory_mib = number("memoryMib", Some(MAX_COMPUTE_MEMORY_MIB.into()))?;
    let wall_secs = number("wallSecs", Some(MAX_COMPUTE_WALL_SECS.into()))?;
    let result = caps
        .get("result")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(format!("{name}: compute.result is required")))?
        .to_owned();
    let within = |value: u64, max: u64, key: &str| {
        if value == 0 || value > max {
            return Err(bad(format!("{name}: compute.{key} is not in 1..={max}")));
        }
        Ok(())
    };
    within(fuel, MAX_COMPUTE_FUEL, "fuel")?;
    within(memory_mib, MAX_COMPUTE_MEMORY_MIB.into(), "memoryMib")?;
    within(wall_secs, MAX_COMPUTE_WALL_SECS.into(), "wallSecs")?;
    Ok(ComputeCaps {
        fuel,
        memory_mib: u32::try_from(memory_mib).unwrap_or(u32::MAX),
        wall_secs: u32::try_from(wall_secs).unwrap_or(u32::MAX),
        result,
    })
}

/// A compute method's `result` names a `computeResult` method of the same ABI.
fn check_compute(methods: &BTreeMap<String, MethodAbi>) -> Result<(), PrepareError> {
    for (name, method) in methods {
        let MethodKind::Compute(caps) = &method.kind else {
            continue;
        };
        if !methods.get(&caps.result).is_some_and(|m| m.compute_result) {
            return Err(bad(format!(
                "{name}: compute.result {:?} is not a computeResult method",
                caps.result
            )));
        }
    }
    Ok(())
}

/// `"<n>s"`, `"<n>m"` or `"<n>h"` with `n > 0`.
fn interval(text: &str) -> Option<Duration> {
    let unit = match text.chars().last()? {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        _ => return None,
    };
    let count: u64 = text[..text.len() - 1].parse().ok()?;
    if count == 0 {
        return None;
    }
    Some(Duration::from_secs(count.checked_mul(unit)?))
}

fn flag(fields: &Map<String, Value>, key: &str, name: &str) -> Result<bool, PrepareError> {
    match fields.get(key) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(other) => Err(bad(format!("{name}: {key} is not a bool: {other}"))),
    }
}

fn object<'v>(json: &'v Value, what: &str) -> Result<&'v Map<String, Value>, PrepareError> {
    json.as_object()
        .ok_or_else(|| bad(format!("{what} is not an object")))
}

fn bad(reason: impl Into<String>) -> PrepareError {
    PrepareError::BadAbi(reason.into())
}
