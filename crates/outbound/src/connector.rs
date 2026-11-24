//! First-party connectors: for a service that matters (payments, an LLM API), the
//! node, not the app, builds the HTTP request.
//!
//! A [`Connector`] is node code: a name, a pinned upstream API version, one host,
//! the credential it attaches and a set of [`Operation`]s. Each operation fixes
//! the method, the path and where each of its typed input [`Field`]s goes, and
//! which response fields come back ([`Output`]). The app names a connector, an
//! operation and a JSON input; anything else (another path, another field, a
//! field of the wrong type, a value the grant does not allow) is refused before
//! anything leaves the node. The request then goes through every gate a raw
//! request goes through ([`OutboundService::prepare`]): the operator's hosts, the
//! approval, the credential's binding and the bounds, and the response is
//! redacted of the credential before it is mapped.
//!
//! An app whose release declares a connector may not reach the connector's host
//! with a raw request ([`RunClients`]), so a key bound to that host is used only
//! for the operations the release declares.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use calimero_primitives::outbound::{
    OutboundMethod, OutboundRequest, MAX_REQUEST_BODY, SECRET_PREFIX, SECRET_SUFFIX,
};
use eyre::{bail, eyre, Result as EyreResult};
use serde_json::{Map, Value};
use tracing::warn;

use crate::{
    egress_gated, refusal_kind, AppIdentity, Approval, EgressCheck, OutboundHost, OutboundService,
    EGRESS_OFF,
};

mod openai;
mod stripe;

pub use openai::OPENAI;
pub use stripe::STRIPE;

/// Every connector this node knows, by name.
pub const CONNECTORS: &[&Connector] = &[&STRIPE, &OPENAI];

/// The largest input one connector call takes, before it is parsed.
pub const MAX_CONNECTOR_INPUT: usize = MAX_REQUEST_BODY;

/// The most values one grant's limit holds (metadata keys, models).
pub const MAX_LIMIT_VALUES: usize = 64;

/// The longest response error code a refusal carries back to the app.
const MAX_ERROR_CODE: usize = 64;

/// The connector named `name`, if this node has one.
#[must_use]
pub fn connector(name: &str) -> Option<&'static Connector> {
    CONNECTORS.iter().copied().find(|c| c.name == name)
}

/// A first-party connector: one service, reached only as its operations say.
#[derive(Debug)]
pub struct Connector {
    /// What a release's manifest names under `connectors`, e.g. `stripe`.
    pub name: &'static str,
    /// The upstream API version the request shapes are pinned to.
    pub version: &'static str,
    /// The one host every operation goes to; raw outbound to it is refused for
    /// an app that declares the connector.
    pub host: &'static str,
    /// The credential the node attaches.
    pub credential: Credential,
    /// Headers every request carries, set by the node (e.g. `Stripe-Version`).
    pub headers: &'static [(&'static str, &'static str)],
    pub operations: &'static [Operation],
}

/// Where a connector's credential goes: the secret binding `secret` (the
/// operator's, or one the instance's owner set), in header `header` written as
/// `format` with `{}` for the value. The app never names or places it.
#[derive(Debug)]
pub struct Credential {
    pub secret: &'static str,
    pub header: &'static str,
    pub format: &'static str,
}

/// One request a connector makes.
#[derive(Debug)]
pub struct Operation {
    /// What the app calls, e.g. `createPaymentIntent`.
    pub name: &'static str,
    pub method: OutboundMethod,
    /// The path, with `{field}` for each [`Place::Path`] field.
    pub path: &'static str,
    pub body: Body,
    /// The input, in the order the body writes it. Nothing else is accepted.
    pub fields: &'static [Field],
    /// What the response returns; nothing else reaches the app.
    pub output: &'static [Output],
}

/// How an operation's body is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Body {
    /// No body (a `GET`).
    None,
    /// `application/x-www-form-urlencoded`; a map field `m` writes `m[key]=value`.
    Form,
    /// `application/json`, one object.
    Json,
}

/// One input field.
#[derive(Debug)]
pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    pub place: Place,
    pub required: bool,
}

/// Where a field goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Place {
    /// Into the path, at `{name}`; only a [`Kind::Id`].
    Path,
    Body,
}

/// What a field holds. Every kind is bounded.
#[derive(Debug)]
pub enum Kind {
    /// A JSON integer in `min..=max`.
    Integer { min: u64, max: u64 },
    /// An id: `prefix` then ASCII letters, digits and `_`, at most `max` bytes.
    Id { prefix: &'static str, max: usize },
    /// Exactly `len` lowercase ASCII letters (an ISO 4217 currency code).
    Code { len: usize },
    /// A string the grant's limit `limit` lists (a model the owner allows).
    Choice { limit: &'static str },
    /// An object of strings, each key one the grant's limit `limit` lists, each
    /// value at most `max_value` bytes.
    Map {
        limit: &'static str,
        max_value: usize,
    },
    /// A chat: at most `max` `{role, content}` objects, `role` one of `roles`,
    /// `content` a string of at most `max_content` bytes.
    Messages {
        max: usize,
        roles: &'static [&'static str],
        max_content: usize,
    },
}

/// One response field the app gets: the value at `from` in the upstream's JSON
/// (an index for an array), written at `to`. A string, number, boolean or null
/// is copied; an object only with `limit`, keeping that limit's keys with a
/// string value; anything else is left out.
#[derive(Debug)]
pub struct Output {
    pub to: &'static [&'static str],
    pub from: &'static [&'static str],
    pub limit: Option<&'static str>,
}

/// What a release may do with one connector: its signed manifest's
/// `connectors.<name>`, plus the values its owner allows.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConnectorGrant {
    pub name: String,
    /// The operations the release declares; any other is refused.
    pub operations: BTreeSet<String>,
    /// The values each limit allows, by the limit's name: the metadata keys a
    /// release declares, the models an owner allows.
    pub limits: BTreeMap<String, BTreeSet<String>>,
}

impl Connector {
    fn operation(&self, name: &str) -> Option<&Operation> {
        self.operations.iter().find(|o| o.name == name)
    }

    /// The URL prefixes `grant`'s operations reach: what an approval of the
    /// connector must cover.
    #[must_use]
    pub fn prefixes(&self, grant: &ConnectorGrant) -> Vec<String> {
        let mut out: Vec<String> = self
            .operations
            .iter()
            .filter(|o| grant.operations.contains(o.name))
            .map(|o| {
                let fixed = o.path.split('{').next().unwrap_or_default();
                format!("https://{}{fixed}", self.host)
            })
            .collect();
        out.dedup();
        out
    }

    /// The request `operation` makes from `input`, with the credential named,
    /// never filled in: [`OutboundService::prepare`] fills it.
    ///
    /// # Errors
    /// The operation is not one the grant declares, or the input is not exactly
    /// what it takes.
    pub fn build(
        &self,
        grant: &ConnectorGrant,
        operation: &str,
        input: &[u8],
    ) -> EyreResult<OutboundRequest> {
        let op = self
            .operation(operation)
            .filter(|o| grant.operations.contains(o.name))
            .ok_or_else(|| {
                eyre!(
                    "{operation} is not an operation of the {} connector this release declares",
                    self.name
                )
            })?;
        if input.len() > MAX_CONNECTOR_INPUT {
            bail!("the input is longer than {MAX_CONNECTOR_INPUT} bytes");
        }
        let Value::Object(input) = serde_json::from_slice(input)? else {
            bail!("the input is a JSON object");
        };
        if let Some(extra) = input
            .keys()
            .find(|k| !op.fields.iter().any(|f| f.name == k.as_str()))
        {
            bail!("{}.{} takes no field {extra:?}", self.name, op.name);
        }
        if names_a_secret(&Value::Object(input.clone())) {
            bail!("an input may not name a secret ({SECRET_PREFIX}…{SECRET_SUFFIX})");
        }

        let mut path = op.path.to_owned();
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        let mut json = Map::new();
        for field in op.fields {
            let Some(value) = input.get(field.name) else {
                if field.required {
                    bail!("{}.{} needs {}", self.name, op.name, field.name);
                }
                continue;
            };
            let value = field.check(grant, value)?;
            match (field.place, op.body) {
                (Place::Path, _) => {
                    let Value::String(id) = &value else {
                        bail!("{} is not an id", field.name);
                    };
                    path = path.replace(&format!("{{{}}}", field.name), id);
                }
                (Place::Body, Body::Form) => match &value {
                    Value::Object(map) => {
                        for (key, v) in map {
                            let _ = form.append_pair(
                                &format!("{}[{key}]", field.name),
                                v.as_str().unwrap_or_default(),
                            );
                        }
                    }
                    Value::String(s) => {
                        let _ = form.append_pair(field.name, s);
                    }
                    Value::Number(n) => {
                        let _ = form.append_pair(field.name, &n.to_string());
                    }
                    _ => bail!("{} cannot be written to a form", field.name),
                },
                (Place::Body, Body::Json) => {
                    let _ = json.insert(field.name.to_owned(), value);
                }
                (Place::Body, Body::None) => bail!("{} has no body to go in", op.name),
            }
        }

        let mut headers: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect();
        headers.push((
            self.credential.header.to_owned(),
            self.credential.format.replace(
                "{}",
                &format!("{SECRET_PREFIX}{}{SECRET_SUFFIX}", self.credential.secret),
            ),
        ));
        let body = match op.body {
            Body::None => Vec::new(),
            Body::Form => {
                headers.push((
                    "content-type".to_owned(),
                    "application/x-www-form-urlencoded".to_owned(),
                ));
                form.finish().into_bytes()
            }
            Body::Json => {
                headers.push(("content-type".to_owned(), "application/json".to_owned()));
                serde_json::to_vec(&json)?
            }
        };
        Ok(OutboundRequest {
            method: op.method,
            url: format!("https://{}{path}", self.host),
            headers,
            body,
        })
    }

    /// The app's view of a `2xx` answer to `operation`: only its declared
    /// outputs.
    ///
    /// # Errors
    /// The answer is not a JSON object.
    pub fn map(&self, grant: &ConnectorGrant, operation: &str, body: &[u8]) -> EyreResult<Vec<u8>> {
        let op = self
            .operation(operation)
            .ok_or_else(|| eyre!("no operation {operation}"))?;
        let answer: Value = serde_json::from_slice(body)
            .map_err(|_| eyre!("{} answered something other than JSON", self.name))?;
        let mut out = Value::Object(Map::new());
        for output in op.output {
            let Some(value) = output.from.iter().try_fold(&answer, |v, step| match v {
                Value::Object(map) => map.get(*step),
                Value::Array(list) => list.get(step.parse::<usize>().ok()?),
                _ => None,
            }) else {
                continue;
            };
            let value = match (value, output.limit) {
                (Value::Object(map), Some(limit)) => {
                    let allowed = grant.limits.get(limit);
                    Value::Object(
                        map.iter()
                            .filter(|(k, v)| {
                                v.is_string() && allowed.is_some_and(|a| a.contains(k.as_str()))
                            })
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    )
                }
                (Value::Object(_) | Value::Array(_), _) => continue,
                (scalar, _) => scalar.clone(),
            };
            put(&mut out, output.to, value);
        }
        Ok(serde_json::to_vec(&out)?)
    }
}

/// Writes `value` at `to` in `out`, making the objects on the way.
fn put(out: &mut Value, to: &[&str], value: Value) {
    let Some((last, path)) = to.split_last() else {
        return;
    };
    let mut at = out;
    for step in path {
        let Value::Object(map) = at else {
            return;
        };
        at = map
            .entry((*step).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if let Value::Object(map) = at {
        let _ = map.insert((*last).to_owned(), value);
    }
}

/// Whether any string or key in `value` holds a secret placeholder, which
/// `prepare` would fill in.
fn names_a_secret(value: &Value) -> bool {
    match value {
        Value::String(s) => s.contains(SECRET_PREFIX),
        Value::Array(list) => list.iter().any(names_a_secret),
        Value::Object(map) => map
            .iter()
            .any(|(k, v)| k.contains(SECRET_PREFIX) || names_a_secret(v)),
        _ => false,
    }
}

impl Field {
    /// `value` as this field takes it, or why not.
    fn check(&self, grant: &ConnectorGrant, value: &Value) -> EyreResult<Value> {
        let name = self.name;
        let allowed = |limit: &str| -> EyreResult<&BTreeSet<String>> {
            grant
                .limits
                .get(limit)
                .filter(|l| !l.is_empty() && l.len() <= MAX_LIMIT_VALUES)
                .ok_or_else(|| eyre!("no {limit} is allowed for {}", grant.name))
        };
        match &self.kind {
            Kind::Integer { min, max } => match value.as_u64() {
                Some(n) if (*min..=*max).contains(&n) => Ok(value.clone()),
                _ => bail!("{name} is an integer from {min} to {max}"),
            },
            Kind::Id { prefix, max } => match value.as_str() {
                Some(s)
                    if s.len() <= *max
                        && s.strip_prefix(prefix).is_some_and(|rest| {
                            !rest.is_empty()
                                && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        }) =>
                {
                    Ok(value.clone())
                }
                _ => bail!("{name} is an id starting {prefix:?}"),
            },
            Kind::Code { len } => match value.as_str() {
                Some(s) if s.len() == *len && s.bytes().all(|b| b.is_ascii_lowercase()) => {
                    Ok(value.clone())
                }
                _ => bail!("{name} is {len} lowercase letters"),
            },
            Kind::Choice { limit } => match value.as_str() {
                Some(s) if allowed(limit)?.contains(s) => Ok(value.clone()),
                _ => bail!("{name} is not a {limit} allowed for {}", grant.name),
            },
            Kind::Map { limit, max_value } => {
                let Value::Object(map) = value else {
                    bail!("{name} is an object of strings");
                };
                let keys = allowed(limit)?;
                for (key, v) in map {
                    if !keys.contains(key) {
                        bail!("{name} key {key:?} is not one this release declares");
                    }
                    if !v.as_str().is_some_and(|s| s.len() <= *max_value) {
                        bail!("{name}.{key} is a string of at most {max_value} bytes");
                    }
                }
                Ok(value.clone())
            }
            Kind::Messages {
                max,
                roles,
                max_content,
            } => {
                let Value::Array(list) = value else {
                    bail!("{name} is a list");
                };
                if list.is_empty() || list.len() > *max {
                    bail!("{name} holds 1 to {max} messages");
                }
                for message in list {
                    let ok = message.as_object().is_some_and(|m| {
                        m.len() == 2
                            && m.get("role")
                                .and_then(Value::as_str)
                                .is_some_and(|r| roles.contains(&r))
                            && m.get("content")
                                .and_then(Value::as_str)
                                .is_some_and(|c| c.len() <= *max_content)
                    });
                    if !ok {
                        bail!(
                            "each of {name} is exactly {{role, content}}, role one of {roles:?}, \
                             content at most {max_content} bytes"
                        );
                    }
                }
                Ok(value.clone())
            }
        }
    }
}

/// Calls a connector for a run: `input` and the answer are JSON.
pub trait ConnectorHost: Send + Sync + fmt::Debug {
    /// Call `operation` of `connector` with `input`.
    ///
    /// # Errors
    /// A message for the guest: a refused input or request, a failed request,
    /// or the service's refusal (its status and error code, nothing more).
    fn call(&self, connector: &str, operation: &str, input: &[u8]) -> Result<Vec<u8>, String>;
}

/// What a run of an app may send: raw requests, connector calls, or both.
#[derive(Debug, Default)]
pub struct RunClients {
    /// Raw requests under the declared prefixes; refused to a declared
    /// connector's host.
    pub http: Option<Arc<dyn OutboundHost>>,
    /// The declared connectors.
    pub connectors: Option<Arc<dyn ConnectorHost>>,
}

impl OutboundService {
    /// [`Self::for_run_counted`] for an app that also declares `connectors`:
    /// its raw requests may not reach a declared connector's host, and its
    /// connector calls go through the same gates and the same count. Both
    /// clients ask `egress` before every request or call and refuse it with
    /// [`EGRESS_OFF`] while egress is off ([`egress_gated`]): a connector is
    /// no way around the switch.
    #[must_use]
    #[expect(clippy::too_many_arguments, reason = "one run's whole outbound scope")]
    pub fn for_run_with_connectors(
        self: &Arc<Self>,
        app: AppIdentity,
        declared: Vec<String>,
        connectors: Vec<ConnectorGrant>,
        approval: Option<Approval>,
        context: String,
        sent: Arc<AtomicU64>,
        egress: EgressCheck,
    ) -> RunClients {
        let grants: BTreeMap<String, ConnectorGrant> = connectors
            .into_iter()
            .filter(|g| connector(&g.name).is_some())
            .map(|g| (g.name.clone(), g))
            .collect();
        let reserved: BTreeMap<String, String> = grants
            .keys()
            .filter_map(|name| connector(name))
            .map(|c| (c.host.to_owned(), c.name.to_owned()))
            .collect();
        let http = self
            .run_outbound(
                app.clone(),
                declared,
                reserved,
                approval.clone(),
                context.clone(),
                Arc::clone(&sent),
            )
            .map(|http| egress_gated(http, Arc::clone(&egress)));
        let connectors = (!grants.is_empty()).then(|| {
            egress_gated_connectors(
                Arc::new(RunConnectors {
                    service: Arc::clone(self),
                    app,
                    grants,
                    approval,
                    context,
                    sent,
                }),
                egress,
            )
        });
        RunClients { http, connectors }
    }
}

/// `inner`, asking `off` before each call and refusing it with [`EGRESS_OFF`]
/// while egress is off, before anything is built, resolved or sent: the
/// connectors' side of [`egress_gated`].
#[must_use]
pub fn egress_gated_connectors(
    inner: Arc<dyn ConnectorHost>,
    off: EgressCheck,
) -> Arc<dyn ConnectorHost> {
    Arc::new(EgressGatedConnectors { inner, off })
}

/// [`egress_gated_connectors`]' host.
struct EgressGatedConnectors {
    inner: Arc<dyn ConnectorHost>,
    off: EgressCheck,
}

impl fmt::Debug for EgressGatedConnectors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EgressGatedConnectors")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl ConnectorHost for EgressGatedConnectors {
    fn call(&self, connector: &str, operation: &str, input: &[u8]) -> Result<Vec<u8>, String> {
        if let Some(who) = (self.off)() {
            return Err(format!(
                "{EGRESS_OFF}: {who} turned egress off, so no request leaves the cluster \
                 for this app, whatever was approved"
            ));
        }
        self.inner.call(connector, operation, input)
    }
}

/// One run's connector client.
#[derive(Debug)]
struct RunConnectors {
    service: Arc<OutboundService>,
    app: AppIdentity,
    grants: BTreeMap<String, ConnectorGrant>,
    approval: Option<Approval>,
    context: String,
    sent: Arc<AtomicU64>,
}

impl RunConnectors {
    /// Logs a refused call by the node's name for the refusal, never its words:
    /// they quote the input (a field, a URL, a name the guest wrote) and this
    /// line ships to the operator's log. The guest gets the words.
    fn refused(&self, name: &str, reason: &eyre::Report) -> String {
        let reason = reason.to_string();
        warn!(
            context = %self.context,
            package = %self.app.package,
            connector = name,
            refusal = refusal_kind(&reason),
            "connector call refused"
        );
        reason
    }
}

impl ConnectorHost for RunConnectors {
    fn call(&self, name: &str, operation: &str, input: &[u8]) -> Result<Vec<u8>, String> {
        let started = Instant::now();
        let (Some(connector), Some(grant)) = (connector(name), self.grants.get(name)) else {
            return Err(format!(
                "{name} is not a connector this release declares (its manifest's `connectors`)"
            ));
        };
        let request = connector
            .build(grant, operation, input)
            .map_err(|e| self.refused(name, &e))?;
        // The credential is the connector's, bound to its host alone, whatever
        // the release declares for its raw requests.
        let mut app = self.app.clone();
        if let Some(secrets) = &mut app.secrets {
            let _ = secrets.insert(
                connector.credential.secret.to_owned(),
                vec![connector.host.to_owned()],
            );
        }
        let declared: BTreeSet<String> = connector.prefixes(grant).into_iter().collect();
        let prepared = self
            .service
            .prepare(&app, &declared, self.approval.as_ref(), request)
            .map_err(|e| self.refused(name, &e))?;
        let (status, body) = self.service.transmit(
            &self.app,
            &self.context,
            &self.sent,
            &declared,
            &prepared,
            started,
        )?;
        if !(200..300).contains(&status) {
            return Err(format!(
                "{name} answered {status}{}",
                error_code(&body).map_or_else(String::new, |c| format!(": {c}"))
            ));
        }
        connector
            .map(grant, operation, &body)
            .map_err(|e| e.to_string())
    }
}

/// The service's error code (`error.code`, else `error.type`) when it is a
/// short token: never its message, which may quote the request.
fn error_code(body: &[u8]) -> Option<String> {
    let answer: Value = serde_json::from_slice(body).ok()?;
    let error = answer.get("error")?;
    ["code", "type"]
        .iter()
        .filter_map(|k| error.get(*k)?.as_str())
        .find(|c| {
            !c.is_empty()
                && c.len() <= MAX_ERROR_CODE
                && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests;
