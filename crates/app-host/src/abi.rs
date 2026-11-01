//! Method attributes from the bundle's signed ABI; the host checks them before `handle`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::engine::PrepareError;

/// ABI versions this host reads. A bundle declares its own in `abiVersion`.
/// Version 2 adds `captcha`, which nodes must enforce (D51): a node that reads only 1
/// refuses such a bundle instead of running the method unguarded.
pub const SUPPORTED_ABI_VERSIONS: &[u64] = &[1, 2];

/// What a method is for; timers are the only methods with an interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    Init,
    Call,
    View,
    Timer,
    /// Runs outside the log on the leader, with outbound HTTP (D12); started, never called.
    Action,
}

/// The human check a method needs before a client's call runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Captcha {
    /// Cloudflare Turnstile: the node verifies the client's token with Cloudflare.
    Turnstile,
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
    /// The apps whose xcalls it accepts (`Caller::App`, D13); `"*"` is any app the
    /// platform delivers from.
    pub allow_apps: Vec<String>,
    /// `allow(action)`: called only by this app's actions, never directly by clients.
    pub action_only: bool,
    /// A client's call needs a solved captcha, which the node verifies first. Calls and
    /// actions only.
    pub captcha: Option<Captcha>,
}

impl MethodAbi {
    /// Whether a delivered xcall from `app` may run it (D13).
    #[must_use]
    pub fn allows_app(&self, app: &str) -> bool {
        self.allow_apps.iter().any(|a| a == "*" || a == app)
    }
}

/// The methods an app declares, by ABI (camelCase) name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Abi {
    pub methods: BTreeMap<String, MethodAbi>,
    /// The tables whose records the host indexes for full-text search (spec section 7):
    /// table (state field, camelCase) to its text fields, from `state.search`. Tables with
    /// no fields are left out.
    pub search: BTreeMap<String, Vec<String>>,
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
            .collect::<Result<_, PrepareError>>()?;
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
    let captcha = match fields.get("captcha") {
        None | Some(Value::Null) => None,
        Some(Value::String(c)) if c == "turnstile" => Some(Captcha::Turnstile),
        Some(other) => return Err(bad(format!("{name}: unknown captcha {other}"))),
    };
    if captcha.is_some() && !matches!(kind, MethodKind::Call | MethodKind::Action) {
        return Err(bad(format!(
            "{name}: `captcha` is for calls and actions only"
        )));
    }
    Ok(MethodAbi {
        kind,
        captcha,
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
