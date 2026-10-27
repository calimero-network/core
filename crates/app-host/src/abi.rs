//! Method attributes from the bundle's signed ABI; the host checks them before `handle`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::engine::PrepareError;

const TOP_LEVEL_KEYS: &[&str] = &["world", "state", "methods", "errors", "events"];
const METHOD_KEYS: &[&str] = &[
    "kind",
    "params",
    "returns",
    "confirm",
    "public",
    "every",
    "allowSystem",
];

/// What a method is for; timers are the only methods with an interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    Init,
    Call,
    View,
    Timer,
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
}

/// The methods an app declares, by ABI (camelCase) name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Abi {
    pub methods: BTreeMap<String, MethodAbi>,
}

impl Abi {
    /// Reads the `calimero:abi` JSON. Unknown keys are refused, so an attribute the host
    /// does not understand can't be silently ignored.
    pub fn from_json(json: &Value) -> Result<Self, PrepareError> {
        let top = object(json, "abi")?;
        closed(top, TOP_LEVEL_KEYS, "abi")?;
        let Some(methods) = top.get("methods") else {
            return Err(bad("abi has no methods"));
        };
        let methods = object(methods, "methods")?
            .iter()
            .map(|(name, method)| Ok((name.clone(), method_abi(name, method)?)))
            .collect::<Result<_, PrepareError>>()?;
        Ok(Self { methods })
    }
}

fn method_abi(name: &str, json: &Value) -> Result<MethodAbi, PrepareError> {
    let fields = object(json, name)?;
    closed(fields, METHOD_KEYS, name)?;
    let kind = match fields.get("kind").and_then(Value::as_str) {
        Some("init") => MethodKind::Init,
        Some("call") => MethodKind::Call,
        Some("view") => MethodKind::View,
        Some("timer") => MethodKind::Timer,
        other => return Err(bad(format!("{name}: unknown kind {other:?}"))),
    };
    let every = match fields.get("every") {
        None | Some(Value::Null) => None,
        Some(Value::String(every)) => {
            Some(interval(every).ok_or_else(|| bad(format!("{name}: bad every {every:?}")))?)
        }
        Some(other) => return Err(bad(format!("{name}: bad every {other}"))),
    };
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
    })
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

fn closed(fields: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<(), PrepareError> {
    match fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(bad(format!("{what}: unknown key {key:?}"))),
        None => Ok(()),
    }
}

fn bad(reason: impl Into<String>) -> PrepareError {
    PrepareError::BadAbi(reason.into())
}
