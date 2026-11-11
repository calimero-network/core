//! The `calimero:abi` JSON read as a client reads it: every method with its parameters and
//! result, the state section, errors and events.
//!
//! The node's own reader (`calimero_app_host::Abi`) keeps only what it enforces; this one
//! keeps the free-form keys too (`params`, `returns`, `errors`, `events`), which describe the
//! app to clients. Unknown keys are ignored, like the node does (Ruling 14).

use std::collections::BTreeMap;

use eyre::{bail, eyre, Result as EyreResult};
use serde_json::{Map, Value};

use crate::ty::Type;

/// What a method is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Init,
    Call,
    View,
    Timer,
    Action,
    Compute,
    /// A kind this reader does not know (a newer SDK's).
    Other(String),
}

impl Kind {
    fn parse(text: &str) -> Self {
        match text {
            "init" => Self::Init,
            "call" => Self::Call,
            "view" => Self::View,
            "timer" => Self::Timer,
            "action" => Self::Action,
            "compute" => Self::Compute,
            other => Self::Other(other.to_owned()),
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Init => "init",
            Self::Call => "call",
            Self::View => "view",
            Self::Timer => "timer",
            Self::Action => "action",
            Self::Compute => "compute",
            Self::Other(other) => other,
        }
    }
}

/// One method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Method {
    pub kind: Kind,
    /// `[name, type]` in declaration order; JSON passes them as one object by name.
    pub params: Vec<(String, Type)>,
    /// What it returns; [`Type::Unit`] for nothing (`returns: null`).
    pub returns: Type,
    pub confirm: bool,
    pub public: bool,
    pub allow_system: bool,
    pub allow_apps: Vec<String>,
    /// `allow(action)`: only the app's own actions call it.
    pub allow_action: bool,
    /// Receives a compute run's outcome; only the node calls it.
    pub compute_result: bool,
    /// A public method's rate limit (`"<n>/s"`, `"<n>/min"`, `"<n>/h"`).
    pub rate: Option<String>,
    /// A timer's interval.
    pub every: Option<String>,
    /// A compute method's caps, as the ABI writes them.
    pub compute: Option<Value>,
}

impl Method {
    /// Whether a client calls it over the API: a call or a view that is not reserved for the
    /// node (`computeResult`) or for the app's actions (`allow(action)`).
    #[must_use]
    pub fn client_callable(&self) -> bool {
        matches!(self.kind, Kind::Call | Kind::View) && !self.compute_result && !self.allow_action
    }

    /// Whether the node answers it as a view: a view that needs no passkey. A `confirm` view
    /// runs through the call path, with the step-up, like `/intents` runs it.
    #[must_use]
    pub fn runs_as_view(&self) -> bool {
        self.kind == Kind::View && !self.confirm
    }
}

/// The state section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Each state field's collection kind (`table`, `unorderedMap`, `cell`, ...).
    pub fields: BTreeMap<String, String>,
    /// Each table's full-text search fields.
    pub search: BTreeMap<String, Vec<String>>,
    /// `#[app::state(version = N)]`.
    pub version: Option<u64>,
}

/// An error's or event's fields: camelCase name to type.
pub type Fields = BTreeMap<String, Type>;

/// An app's ABI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppAbi {
    pub abi_version: u64,
    pub world: Option<String>,
    pub methods: BTreeMap<String, Method>,
    pub state: State,
    pub errors: BTreeMap<String, Fields>,
    pub events: BTreeMap<String, Fields>,
}

impl AppAbi {
    /// Reads the ABI JSON, or the node's `GET /apps/{app}/abi` answer (`{ bundle, abi }`).
    pub fn from_json(json: &Value) -> EyreResult<Self> {
        let json = match json.get("abi") {
            Some(Value::Null) if json.get("methods").is_none() => {
                bail!("the app's bundle embeds no ABI")
            }
            Some(inner) if json.get("methods").is_none() => inner,
            _ => json,
        };
        let top = object(json, "the ABI")?;
        let abi_version = top
            .get("abiVersion")
            .and_then(Value::as_u64)
            .ok_or_else(|| eyre!("the ABI has no abiVersion"))?;
        let methods = object(
            top.get("methods")
                .ok_or_else(|| eyre!("the ABI has no methods"))?,
            "methods",
        )?
        .iter()
        .map(|(name, m)| Ok((name.clone(), method(name, m)?)))
        .collect::<EyreResult<_>>()?;
        Ok(Self {
            abi_version,
            world: top.get("world").and_then(Value::as_str).map(str::to_owned),
            methods,
            state: match top.get("state") {
                None | Some(Value::Null) => State::default(),
                Some(s) => state(s)?,
            },
            errors: payloads(top.get("errors"), "errors")?,
            events: payloads(top.get("events"), "events")?,
        })
    }

    /// The methods clients call, by name.
    pub fn client_methods(&self) -> impl Iterator<Item = (&String, &Method)> {
        self.methods.iter().filter(|(_, m)| m.client_callable())
    }

    /// Whether the app has actions, whose failures the node reports as `ActionFailed` events.
    #[must_use]
    pub fn has_actions(&self) -> bool {
        self.methods.values().any(|m| m.kind == Kind::Action)
    }
}

/// The event the node itself emits when an action of the app fails (`ActionFailed`).
#[must_use]
pub fn action_failed_fields() -> Fields {
    ["actionId", "method", "reason"]
        .into_iter()
        .map(|k| (k.to_owned(), Type::String))
        .collect()
}

fn method(name: &str, json: &Value) -> EyreResult<Method> {
    let fields = object(json, name)?;
    let kind = Kind::parse(
        fields
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| eyre!("{name}: no kind"))?,
    );
    let params = match fields.get("params") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(params)) => params
            .iter()
            .map(|p| match p.as_array().map(Vec::as_slice) {
                Some([Value::String(n), Value::String(t)]) => Ok((n.clone(), Type::parse(t))),
                _ => Err(eyre!("{name}: a param is not [name, type]: {p}")),
            })
            .collect::<EyreResult<_>>()?,
        Some(other) => bail!("{name}: params is not a list: {other}"),
    };
    let returns = match fields.get("returns") {
        None | Some(Value::Null) => Type::Unit,
        Some(Value::String(t)) => Type::parse(t),
        Some(other) => bail!("{name}: returns is not a type: {other}"),
    };
    let text = |key: &str| fields.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(Method {
        kind,
        params,
        returns,
        confirm: flag(fields, "confirm", name)?,
        public: flag(fields, "public", name)?,
        allow_system: flag(fields, "allowSystem", name)?,
        allow_apps: fields
            .get("allowApps")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        allow_action: flag(fields, "allowAction", name)?,
        compute_result: flag(fields, "computeResult", name)?,
        rate: text("rate"),
        every: text("every"),
        compute: fields.get("compute").cloned(),
    })
}

fn state(json: &Value) -> EyreResult<State> {
    let top = object(json, "state")?;
    let fields = match top.get("fields") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(f) => object(f, "state.fields")?
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
            .collect(),
    };
    let search = match top.get("search") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(s) => object(s, "state.search")?
            .iter()
            .map(|(k, v)| {
                let fields = v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                (k.clone(), fields)
            })
            .collect(),
    };
    Ok(State {
        fields,
        search,
        version: top.get("version").and_then(Value::as_u64),
    })
}

fn payloads(json: Option<&Value>, what: &str) -> EyreResult<BTreeMap<String, Fields>> {
    let Some(json) = json.filter(|j| !j.is_null()) else {
        return Ok(BTreeMap::new());
    };
    object(json, what)?
        .iter()
        .map(|(name, fields)| {
            let fields = object(fields, name)?
                .iter()
                .map(|(k, t)| match t.as_str() {
                    Some(t) => Ok((k.clone(), Type::parse(t))),
                    None => Err(eyre!("{what}.{name}.{k} is not a type: {t}")),
                })
                .collect::<EyreResult<_>>()?;
            Ok((name.clone(), fields))
        })
        .collect()
}

fn flag(fields: &Map<String, Value>, key: &str, name: &str) -> EyreResult<bool> {
    match fields.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(other) => bail!("{name}: {key} is not a bool: {other}"),
    }
}

fn object<'v>(json: &'v Value, what: &str) -> EyreResult<&'v Map<String, Value>> {
    json.as_object()
        .ok_or_else(|| eyre!("{what} is not an object"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_the_node_answer_and_the_bare_abi() {
        let abi = json!({
            "abiVersion": 1,
            "methods": { "get": { "kind": "view", "params": [["key", "string"]], "returns": "option<string>" } },
        });
        let bare = AppAbi::from_json(&abi).unwrap();
        let wrapped = AppAbi::from_json(&json!({ "bundle": "ab", "abi": abi })).unwrap();
        assert_eq!(bare, wrapped);
        assert!(bare.methods["get"].runs_as_view());
        assert!(AppAbi::from_json(&json!({ "bundle": "ab", "abi": null })).is_err());
    }

    #[test]
    fn node_only_methods_are_not_client_methods() {
        let abi = AppAbi::from_json(&json!({
            "abiVersion": 1,
            "methods": {
                "a": { "kind": "call", "params": [] },
                "b": { "kind": "call", "params": [], "computeResult": true },
                "c": { "kind": "call", "params": [], "allowAction": true },
                "d": { "kind": "timer", "params": [], "every": "30s" },
                "e": { "kind": "init", "params": [] },
                "f": { "kind": "view", "params": [], "confirm": true },
            },
        }))
        .unwrap();
        let names: Vec<_> = abi.client_methods().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["a", "f"]);
        assert!(!abi.methods["f"].runs_as_view());
    }

    #[test]
    fn a_wrong_shape_is_refused() {
        let bad =
            json!({ "abiVersion": 1, "methods": { "a": { "kind": "call", "params": [["x"]] } } });
        assert!(AppAbi::from_json(&bad).is_err());
        let bad =
            json!({ "abiVersion": 1, "methods": { "a": { "kind": "call", "confirm": "yes" } } });
        assert!(AppAbi::from_json(&bad).is_err());
    }
}
