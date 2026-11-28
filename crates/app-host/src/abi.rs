//! Method attributes from the bundle's signed ABI; the host checks them before `handle`.

use std::collections::BTreeMap;
use std::time::Duration;

use calimero_app_wit::{names_compute_world, names_sandbox_world};
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

/// Most vCPUs a sandbox method may declare.
pub const MAX_SANDBOX_VCPUS: u32 = 4;
/// Most memory a sandbox method may declare for everything its commands start.
pub const MAX_SANDBOX_MEMORY_MIB: u32 = 8_192;
/// Most workspace a sandbox method may declare.
pub const MAX_SANDBOX_WORKSPACE_MIB: u32 = 20_480;
/// Most bytes of a sandbox method's image name.
pub const MAX_SANDBOX_IMAGE_BYTES: usize = 128;

/// What a method is for; timers are the only methods with an interval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MethodKind {
    Init,
    Call,
    View,
    Timer,
    /// Runs outside the log on the leader, with outbound HTTP; started, never called.
    Action,
    /// Runs on a job node under these caps; only the node starts it. A sandbox method (ABI
    /// kind `sandbox`) is a compute method whose caps carry `sandbox`.
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
    /// Set for a sandbox method: it runs on a sandbox job node and drives a Linux sandbox.
    pub sandbox: Option<SandboxCaps>,
}

impl ComputeCaps {
    /// Whether this is a sandbox method's.
    #[must_use]
    pub fn is_sandbox(&self) -> bool {
        self.sandbox.is_some()
    }
}

/// What a sandbox method declares for the Linux sandbox its driver runs commands in: caps for
/// everything those commands start, and the image they run in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxCaps {
    pub vcpus: u32,
    pub memory_mib: u32,
    pub workspace_mib: u32,
    /// The image's name in the node's image catalog.
    pub image: String,
}

/// One method's attributes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodAbi {
    pub kind: MethodKind,
    /// Needs the owner's passkey confirmation; never runs as a view.
    pub confirm: bool,
    /// Callable without signing in.
    pub public: bool,
    /// A timer's interval; `Some` exactly for timers.
    pub every: Option<Duration>,
    /// Accepts `Caller::System`.
    pub allow_system: bool,
    /// The apps whose xcalls it accepts (`Caller::App`); `"*"` is any app the
    /// platform delivers from.
    pub allow_apps: Vec<String>,
    /// `allow(action)`: called only by this app's actions, never directly by clients.
    pub action_only: bool,
    /// Receives a compute run's outcome; only the node invokes it.
    pub compute_result: bool,
}

impl MethodAbi {
    /// Whether a delivered xcall from `app` may run it.
    #[must_use]
    pub fn allows_app(&self, app: &str) -> bool {
        self.allow_apps.iter().any(|a| a == "*" || a == app)
    }
}

/// The methods an app declares, by ABI (camelCase) name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Abi {
    pub methods: BTreeMap<String, MethodAbi>,
    /// The tables whose records the host indexes for full-text search:
    /// table (state field, camelCase) to its text fields, from `state.search`. Tables with
    /// no fields are left out.
    pub search: BTreeMap<String, Vec<String>>,
}

impl Abi {
    /// Reads the `calimero:abi` JSON. `abiVersion` is required and must be supported.
    /// Unknown keys, top level or per method, are ignored so newer builds stay readable;
    /// a known key with a value of the wrong shape is refused.
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
        let world = top.get("world").and_then(Value::as_str);
        if world.is_some_and(names_compute_world) {
            check_component_world(&methods, false)?;
        } else if world.is_some_and(names_sandbox_world) {
            check_component_world(&methods, true)?;
        } else {
            check_compute(&methods)?;
        }
        let search = match top.get("state").and_then(|s| s.get("search")) {
            None | Some(Value::Null) => BTreeMap::new(),
            Some(search) => search_fields(search)?,
        };
        Ok(Self { methods, search })
    }
}

fn method_abi(name: &str, json: &Value) -> Result<MethodAbi, PrepareError> {
    let fields = object(json, name)?;
    let kind = match fields.get("kind").and_then(Value::as_str) {
        Some("init") => MethodKind::Init,
        Some("call") => MethodKind::Call,
        Some("view") => MethodKind::View,
        Some("timer") => MethodKind::Timer,
        Some("action") => MethodKind::Action,
        Some("compute") => MethodKind::Compute(compute_caps(fields, name, false)?),
        Some("sandbox") => MethodKind::Compute(compute_caps(fields, name, true)?),
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
    if !matches!(&kind, MethodKind::Compute(c) if c.is_sandbox()) && fields.contains_key("sandbox")
    {
        return Err(bad(format!(
            "{name}: `sandbox` is only for sandbox methods"
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
        action_only: flag(fields, "allowAction", name)?,
        allow_apps: match fields.get("allowApps") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(apps)) => apps
                .iter()
                .map(|a| {
                    a.as_str()
                        .filter(|a| !a.is_empty())
                        .map(str::to_owned)
                        .ok_or_else(|| bad(format!("{name}: allowApps holds {a}")))
                })
                .collect::<Result<_, _>>()?,
            Some(other) => return Err(bad(format!("{name}: allowApps is not a list: {other}"))),
        },
        compute_result,
    })
}

/// `state.search`: `{ table: [field, ..] }`, every field a non-empty string.
fn search_fields(json: &Value) -> Result<BTreeMap<String, Vec<String>>, PrepareError> {
    let mut out = BTreeMap::new();
    for (table, fields) in object(json, "state.search")? {
        let Some(fields) = fields.as_array() else {
            return Err(bad(format!("search of {table} is not a list")));
        };
        let fields = fields
            .iter()
            .map(|f| match f.as_str() {
                Some(f) if !f.is_empty() => Ok(f.to_owned()),
                _ => Err(bad(format!("search of {table}: {f} is not a field name"))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !fields.is_empty() {
            let _prev = out.insert(table.clone(), fields);
        }
    }
    Ok(out)
}

/// The `compute` object of a compute or sandbox method; fuel is required, memory and wall
/// default to the maxima. A sandbox method also declares `sandbox`.
fn compute_caps(
    fields: &Map<String, Value>,
    name: &str,
    sandbox: bool,
) -> Result<ComputeCaps, PrepareError> {
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
    let sandbox = if sandbox {
        Some(sandbox_caps(fields, name)?)
    } else {
        None
    };
    Ok(ComputeCaps {
        fuel,
        memory_mib: u32::try_from(memory_mib).unwrap_or(u32::MAX),
        wall_secs: u32::try_from(wall_secs).unwrap_or(u32::MAX),
        result,
        sandbox,
    })
}

/// The `sandbox` object of a sandbox method: `{ vcpus, memoryMib, workspaceMib, image }`,
/// every number in `1..=` its maximum and the image a non-empty name.
fn sandbox_caps(fields: &Map<String, Value>, name: &str) -> Result<SandboxCaps, PrepareError> {
    let caps = fields
        .get("sandbox")
        .ok_or_else(|| bad(format!("{name}: a sandbox method declares `sandbox`")))?;
    let caps = object(caps, name)?;
    let number = |key: &str, max: u32| -> Result<u32, PrepareError> {
        let value = caps
            .get(key)
            .ok_or_else(|| bad(format!("{name}: sandbox.{key} is required")))?;
        value
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| (1..=max).contains(v))
            .ok_or_else(|| {
                bad(format!(
                    "{name}: sandbox.{key} is not in 1..={max}: {value}"
                ))
            })
    };
    let image = caps
        .get("image")
        .and_then(Value::as_str)
        .filter(|i| !i.is_empty() && i.len() <= MAX_SANDBOX_IMAGE_BYTES)
        .ok_or_else(|| {
            bad(format!(
                "{name}: sandbox.image is required, at most {MAX_SANDBOX_IMAGE_BYTES} bytes"
            ))
        })?
        .to_owned();
    Ok(SandboxCaps {
        vcpus: number("vcpus", MAX_SANDBOX_VCPUS)?,
        memory_mib: number("memoryMib", MAX_SANDBOX_MEMORY_MIB)?,
        workspace_mib: number("workspaceMib", MAX_SANDBOX_WORKSPACE_MIB)?,
        image,
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

/// A compute component's ABI lists compute methods only, and a sandbox component's sandbox
/// methods only; their result methods live in the app's own ABI.
fn check_component_world(
    methods: &BTreeMap<String, MethodAbi>,
    sandbox: bool,
) -> Result<(), PrepareError> {
    let (component, kind) = if sandbox {
        ("sandbox", "sandbox")
    } else {
        ("compute", "compute")
    };
    match methods
        .iter()
        .find(|(_, m)| !matches!(&m.kind, MethodKind::Compute(c) if c.is_sandbox() == sandbox))
    {
        Some((name, _)) => Err(bad(format!(
            "{name}: a {component} component runs {kind} methods only"
        ))),
        None => Ok(()),
    }
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
