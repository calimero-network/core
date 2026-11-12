//! An app's ABI as an OpenAPI 3.1 document.
//!
//! Each method a client calls is an operation `POST /apps/{app}/methods/{name}` (operationId
//! the method's name, tagged `call` or `view`), which the node runs exactly as it runs the
//! same method through `/apps/{app}/intents` or `/apps/{app}/views`. Its request body is the
//! envelope those routes take without `method` (`args` the parameters by name), its answer is
//! theirs with `result` typed from `returns`, the app's errors are the `422 APP_ERROR` it can
//! answer, and a `confirm` method documents the `412 CONFIRM_REQUIRED` step-up. The app's
//! events are the frames of `GET /apps/{app}/events`.
//!
//! Errors use the node API's shape, `{ error, message, details }`. Every request travels
//! inside the sealed channel; a client generated from this document needs that transport
//! (mero-js, calimero-client-py), which is why the typed clients in [`crate::codegen`] are
//! written on top of those clients rather than on plain HTTP.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::abi::{
    action_failed_fields, AppAbi, EnumRepr, Field, Fields, Method, Shape, TypeDef, VariantData,
};
use crate::ty::{Int, Type};

/// The document's `info`.
#[derive(Clone, Debug)]
pub struct Info {
    pub title: String,
    pub version: String,
    /// The app instance the document was served for: the `app` parameter's example.
    pub app: Option<String>,
}

/// The OpenAPI version written.
pub const OPENAPI_VERSION: &str = "3.1.0";

const DESCRIPTION: &str = "Generated from the app's ABI (its `calimero:abi` section). \
Every request and answer travels inside the node's sealed channel, after the client has \
verified the node's attestation: use mero-js or calimero-client-py as the transport (the \
typed clients `mero-app-build codegen` writes do), not plain HTTP. \
`POST /apps/{app}/methods/{name}` runs exactly as `POST /apps/{app}/intents` (a call) or \
`POST /apps/{app}/views` (a view) with the same `method`: the same authentication, gating, \
passkey step-up and limits. Errors are `{ error, message, details }`.";

/// The document for `abi`.
#[must_use]
pub fn document(abi: &AppAbi, info: &Info) -> Value {
    let mut ctx = Ctx::new(abi);
    let mut paths = Map::new();
    for (name, method) in abi.client_methods() {
        let op = ctx.operation(abi, name, method);
        let _ = paths.insert(
            format!("/apps/{{app}}/methods/{name}"),
            json!({ "post": op }),
        );
    }
    let _ = paths.insert("/apps/{app}/events".into(), ctx.events_path(abi));
    error_schemas(abi, &mut ctx);

    let mut app_param = json!({
        "name": "app",
        "in": "path",
        "required": true,
        "description": "The app instance's id.",
        "schema": { "type": "string" },
    });
    if let Some(app) = &info.app {
        app_param["example"] = Value::String(app.clone());
    }
    let mut x_calimero = json!({ "abiVersion": abi.abi_version });
    if let Some(world) = &abi.world {
        x_calimero["world"] = Value::String(world.clone());
    }
    if let Some(version) = abi.state.version {
        x_calimero["stateVersion"] = version.into();
    }

    let mut schemas = common_schemas();
    schemas.append(&mut ctx.schemas);
    sorted(&json!({
        "openapi": OPENAPI_VERSION,
        "info": {
            "title": info.title,
            "version": info.version,
            "description": DESCRIPTION,
        },
        "x-calimero": x_calimero,
        "tags": [
            { "name": "call", "description": "Writes: each runs once, in order, through the app's log." },
            { "name": "view", "description": "Reads: answered from the app's state without writing." },
            { "name": "events", "description": "The app's committed events, then live." },
        ],
        "security": [{ "session": [] }],
        "paths": paths,
        "components": {
            "parameters": { "app": app_param },
            "schemas": schemas,
            "responses": responses(abi),
            "securitySchemes": {
                "session": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "A passkey session's token or an API key, as `Authorization: Bearer`, inside the sealed channel.",
                },
            },
        },
    }))
}

/// `value` with every object's keys in sorted order, whether or not the build unifies
/// serde_json's `preserve_order` in, so the same ABI always gives the same bytes.
#[must_use]
pub fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.clone(), sorted(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// `getOrInsert` → `GetOrInsert`; `not_found` → `NotFound`.
#[must_use]
pub fn pascal(name: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            upper = true;
            continue;
        }
        if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// The component name of an app type: its last path segments, anything else an underscore
/// (`Page<Booking>` → `Page_Booking`).
#[must_use]
pub fn type_name(abi_text: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            let last = word.rsplit("::").next().unwrap_or(word);
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            out.push_str(last);
            word.clear();
        }
    };
    for c in abi_text.chars() {
        if c.is_alphanumeric() || c == '_' || c == ':' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
        }
    }
    flush(&mut word, &mut out);
    if out.is_empty() {
        out.push_str("AppType");
    }
    out
}

pub(crate) fn args_name(method: &str) -> String {
    format!("{}Args", pascal(method))
}

pub(crate) fn error_name(code: &str) -> String {
    format!("{}Error", pascal(code))
}

pub(crate) fn event_name(event: &str) -> String {
    format!("{}Event", pascal(event))
}

/// The events a subscriber can receive: the app's own, and the node's `ActionFailed` when the
/// app has actions and does not declare that name itself.
pub(crate) fn all_events(abi: &AppAbi) -> BTreeMap<String, Fields> {
    let mut events = abi.events.clone();
    if abi.has_actions() {
        let _ = events
            .entry("ActionFailed".to_owned())
            .or_insert_with(action_failed_fields);
    }
    events
}

/// Component names: the generated ones are reserved first, so an app type that happens to
/// share one is renamed rather than replacing it.
pub(crate) struct Names {
    taken: BTreeSet<String>,
    named: BTreeMap<String, String>,
}

impl Names {
    pub(crate) fn new(abi: &AppAbi) -> Self {
        let mut taken: BTreeSet<String> = [
            "ApiError",
            "AppError",
            "Trap",
            "Confirm",
            "ConfirmRequired",
            "Receipt",
            "CallEvent",
            "EventFrame",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        for (name, _) in abi.client_methods() {
            for suffix in ["Args", "Request", "Response"] {
                let _ = taken.insert(format!("{}{suffix}", pascal(name)));
            }
        }
        taken.extend(abi.errors.keys().map(|c| error_name(c)));
        taken.extend(all_events(abi).keys().map(|e| event_name(e)));
        Self {
            taken,
            named: BTreeMap::new(),
        }
    }

    /// Keeps `name` from an app type: a client generator's own names (`CallOptions`, its
    /// class), which an app type that shares one must not replace.
    pub(crate) fn reserve(&mut self, name: &str) {
        let _ = self.taken.insert(name.to_owned());
    }

    /// The component name of app type `abi_text`, the same on every call.
    pub(crate) fn named(&mut self, abi_text: &str) -> (String, bool) {
        if let Some(name) = self.named.get(abi_text) {
            return (name.clone(), false);
        }
        let base = type_name(abi_text);
        let mut name = base.clone();
        let mut n = 1;
        while self.taken.contains(&name) {
            name = if n == 1 {
                format!("{base}Type")
            } else {
                format!("{base}Type{n}")
            };
            n += 1;
        }
        let _ = self.taken.insert(name.clone());
        let _ = self.named.insert(abi_text.to_owned(), name.clone());
        (name, true)
    }
}

struct Ctx {
    names: Names,
    schemas: Map<String, Value>,
    types: BTreeMap<String, TypeDef>,
}

fn reference(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

fn response_ref(name: &str) -> Value {
    json!({ "$ref": format!("#/components/responses/{name}") })
}

impl Ctx {
    fn new(abi: &AppAbi) -> Self {
        Self {
            names: Names::new(abi),
            schemas: Map::new(),
            types: abi.types.clone(),
        }
    }

    /// The JSON Schema of an app type's shape, as serde writes it.
    fn shape_schema(&mut self, shape: &Shape) -> Value {
        match shape {
            Shape::Record(fields) => self.object(fields, None),
            Shape::Newtype(t) => self.schema(t),
            Shape::Tuple(items) => self.schema(&Type::Tuple(items.clone())),
            Shape::Unit => json!({ "type": "null" }),
            Shape::Enum { repr, variants } => {
                let one_of: Vec<Value> = variants
                    .iter()
                    .map(|v| {
                        let name = json!({ "const": v.name });
                        let data = |ctx: &mut Self| match &v.data {
                            VariantData::Unit => json!({ "type": "null" }),
                            VariantData::Newtype(t) => ctx.schema(t),
                            VariantData::Tuple(ts) => ctx.schema(&Type::Tuple(ts.clone())),
                            VariantData::Record(fs) => ctx.object(fs, None),
                        };
                        match (repr, &v.data) {
                            (EnumRepr::External, VariantData::Unit) => name,
                            (EnumRepr::External, _) => json!({
                                "type": "object",
                                "properties": { v.name.clone(): data(self) },
                                "required": [v.name],
                                "additionalProperties": false,
                            }),
                            (EnumRepr::Internal { tag }, VariantData::Record(fs)) => {
                                self.object(fs, Some((tag, &v.name)))
                            }
                            (EnumRepr::Internal { tag }, VariantData::Unit) => {
                                self.object(&[], Some((tag, &v.name)))
                            }
                            (EnumRepr::Internal { tag }, _) => json!({
                                "allOf": [self.object(&[], Some((tag, &v.name))), data(self)],
                            }),
                            (EnumRepr::Adjacent { tag, .. }, VariantData::Unit) => {
                                self.object(&[], Some((tag, &v.name)))
                            }
                            (EnumRepr::Adjacent { tag, content }, _) => json!({
                                "type": "object",
                                "properties": { tag.clone(): name, content.clone(): data(self) },
                                "required": [tag, content],
                            }),
                            (EnumRepr::Untagged, _) => data(self),
                        }
                    })
                    .collect();
                json!({ "oneOf": one_of })
            }
        }
    }

    /// An object of `fields`, with a `tag` property naming the variant when given.
    fn object(&mut self, fields: &[Field], tag: Option<(&String, &String)>) -> Value {
        let mut props = Map::new();
        let mut required = Vec::new();
        if let Some((tag, name)) = tag {
            let _ = props.insert(tag.clone(), json!({ "const": name }));
            required.push(tag.clone());
        }
        for f in fields {
            let _ = props.insert(f.name.clone(), self.schema(&f.ty));
            if !f.optional {
                required.push(f.name.clone());
            }
        }
        json!({ "type": "object", "properties": props, "required": required })
    }

    /// The JSON Schema of `ty` as it travels in JSON (D20).
    fn schema(&mut self, ty: &Type) -> Value {
        match ty {
            Type::Unit => json!({ "type": "null" }),
            Type::Bool => json!({ "type": "boolean" }),
            Type::Int(int) => int_schema(*int),
            Type::Float(double) => {
                json!({ "type": "number", "format": if *double { "double" } else { "float" } })
            }
            Type::Char => json!({ "type": "string", "minLength": 1, "maxLength": 1 }),
            Type::String => json!({ "type": "string" }),
            Type::Bytes => json!({
                "type": "string",
                "contentEncoding": "base64url",
                "pattern": "^[A-Za-z0-9_-]*$",
                "description": "Bytes, base64url without padding.",
            }),
            Type::Option(inner) => json!({ "anyOf": [self.schema(inner), { "type": "null" }] }),
            Type::List(inner) => json!({ "type": "array", "items": self.schema(inner) }),
            Type::Set(inner) => {
                json!({ "type": "array", "items": self.schema(inner), "uniqueItems": true })
            }
            Type::Tuple(items) => {
                let prefix: Vec<Value> = items.iter().map(|t| self.schema(t)).collect();
                json!({
                    "type": "array",
                    "prefixItems": prefix,
                    "items": false,
                    "minItems": items.len(),
                    "maxItems": items.len(),
                })
            }
            Type::Array(inner, n) => json!({
                "type": "array",
                "items": self.schema(inner),
                "minItems": n,
                "maxItems": n,
            }),
            Type::Map(key, value) => {
                let mut s = json!({ "type": "object", "additionalProperties": self.schema(value) });
                match &**key {
                    Type::String | Type::Char => {}
                    Type::Int(int) => {
                        s["propertyNames"] = json!({ "pattern": int_pattern(*int) });
                    }
                    other => s["x-calimero-key-type"] = Value::String(other.to_string()),
                }
                s
            }
            Type::Named(text) => {
                let (name, new) = self.names.named(text);
                let described = ty.named_parts().and_then(|(entry, args)| {
                    let def = self.types.get(&entry)?;
                    (def.params.len() == args.len()).then(|| def.instantiate(&args))
                });
                if let (true, Some(shape)) = (new, described) {
                    let mut schema = self.shape_schema(&shape);
                    schema["x-calimero-abi-type"] = Value::String(text.clone());
                    let _ = self.schemas.insert(name.clone(), schema);
                } else if new {
                    let _ = self.schemas.insert(
                        name.clone(),
                        json!({
                            "description": format!("The app type `{text}`. The ABI names it without describing its shape, so any JSON value is accepted here."),
                            "x-calimero-abi-type": text,
                        }),
                    );
                }
                reference(&name)
            }
        }
    }

    fn fields(&mut self, fields: &Fields) -> Value {
        let props: Map<String, Value> = fields
            .iter()
            .map(|(k, t)| (k.clone(), self.schema(t)))
            .collect();
        let required: Vec<&String> = fields
            .iter()
            .filter(|(_, t)| !t.is_optional())
            .map(|(k, _)| k)
            .collect();
        json!({ "type": "object", "properties": props, "required": required })
    }

    fn operation(&mut self, abi: &AppAbi, name: &str, m: &Method) -> Value {
        let pascal_name = pascal(name);
        let args_schema = {
            let props: Map<String, Value> = m
                .params
                .iter()
                .map(|(p, t)| (p.clone(), self.schema(t)))
                .collect();
            let required: Vec<&String> = m
                .params
                .iter()
                .filter(|(_, t)| !t.is_optional())
                .map(|(p, _)| p)
                .collect();
            json!({
                "type": "object",
                "description": format!("The arguments of `{name}`, by name."),
                "properties": props,
                "required": required,
            })
        };
        let args_required = m.params.iter().any(|(_, t)| !t.is_optional());
        let _ = self.schemas.insert(args_name(name), args_schema);

        let view = m.runs_as_view();
        let mut request = json!({
            "type": "object",
            "properties": { "args": reference(&args_name(name)) },
            "required": if args_required { json!(["args"]) } else { json!([]) },
        });
        if view {
            request["properties"]["consistency"] = json!({
                "type": "string",
                "enum": ["strong", "stale"],
                "default": "strong",
                "description": "`strong` reads the leader's latest state; `stale` any replica at least as new as `minIndex`.",
            });
            request["properties"]["minIndex"] =
                json!({ "type": "integer", "minimum": 0, "default": 0 });
        } else {
            request["properties"]["idempotencyKey"] = json!({
                "type": "string",
                "description": "A retry with the same key is answered once, from the first run.",
            });
            if m.confirm {
                request["properties"]["confirm"] = reference("Confirm");
            }
        }
        let _ = self
            .schemas
            .insert(format!("{pascal_name}Request"), request);

        let result = self.schema(&m.returns);
        let mut response = json!({
            "type": "object",
            "required": ["index", "result", "receipt"],
            "properties": {
                "index": { "type": "integer", "minimum": 0, "description": "The log index the answer reflects." },
                "result": result,
                "receipt": reference("Receipt"),
            },
        });
        if !view {
            response["required"] = json!(["index", "result", "events", "receipt"]);
            response["properties"]["events"] = json!({
                "type": "array",
                "description": "The events the call emitted that the caller is in the audience of.",
                "items": reference("CallEvent"),
            });
        }
        let _ = self
            .schemas
            .insert(format!("{pascal_name}Response"), response);

        let mut flags = json!({ "kind": m.kind.name(), "confirm": m.confirm, "public": m.public });
        if m.allow_system {
            flags["allowSystem"] = true.into();
        }
        if !m.allow_apps.is_empty() {
            flags["allowApps"] = json!(m.allow_apps);
        }
        if let Some(rate) = &m.rate {
            flags["rate"] = Value::String(rate.clone());
        }

        let mut responses = Map::new();
        let _ = responses.insert(
            "200".into(),
            json!({
                "description": "The method ran.",
                "content": { "application/json": { "schema": reference(&format!("{pascal_name}Response")) } },
            }),
        );
        for (status, name) in [
            ("400", "BadRequest"),
            ("401", "Unauthorized"),
            ("403", "Forbidden"),
            ("404", "NotFound"),
            ("410", "Gone"),
        ] {
            let _ = responses.insert(status.into(), response_ref(name));
        }
        if m.confirm {
            let _ = responses.insert("412".into(), response_ref("ConfirmRequired"));
        }
        let _ = responses.insert("422".into(), response_ref("AppError"));
        let _ = responses.insert("429".into(), response_ref("RateLimited"));
        let _ = responses.insert("503".into(), response_ref("Unavailable"));

        let mut description = vec![if view {
            format!("A view: runs as `POST /apps/{{app}}/views` with `method: \"{name}\"`.")
        } else {
            format!("A call: runs as `POST /apps/{{app}}/intents` with `method: \"{name}\"`.")
        }];
        if m.confirm {
            description.push(
                "Needs the account holder's passkey: the first request is answered `412 CONFIRM_REQUIRED` with a challenge; send the same request again with `confirm` (the challenge id and the passkey's assertion over it).".into(),
            );
        }
        if m.public {
            description.push("Public: callers who are not signed in may call it.".into());
        }
        if let Some(rate) = &m.rate {
            description.push(format!(
                "Rate-limited to {rate} per caller (`429 RATE_LIMITED`)."
            ));
        }
        if !abi.errors.is_empty() {
            description.push("The ABI lists the app's errors for the whole app, not per method: any of them may answer `422 APP_ERROR`.".into());
        }

        let mut op = json!({
            "operationId": name,
            "tags": [m.kind.name()],
            "summary": format!("{} `{name}`", m.kind.name()),
            "description": description.join("\n\n"),
            "x-calimero-method": flags,
            "parameters": [{ "$ref": "#/components/parameters/app" }],
            "requestBody": {
                "required": true,
                "content": { "application/json": { "schema": reference(&format!("{pascal_name}Request")) } },
            },
            "responses": responses,
        });
        if m.public {
            op["security"] = json!([{}, { "session": [] }]);
        }
        op
    }

    fn events_path(&mut self, abi: &AppAbi) -> Value {
        let events = all_events(abi);
        let mut frames = Vec::new();
        let mut call_events = Vec::new();
        for (event, fields) in &events {
            let schema_name = event_name(event);
            let data = self.fields(fields);
            let _ = self.schemas.insert(schema_name.clone(), data);
            frames.push(json!({
                "type": "object",
                "required": ["id", "event", "data"],
                "properties": {
                    "id": { "type": "string", "description": "`<index>:<seq>`." },
                    "event": { "const": event },
                    "data": reference(&schema_name),
                },
            }));
            call_events.push(json!({
                "type": "object",
                "required": ["kind", "data"],
                "properties": { "kind": { "const": event }, "data": reference(&schema_name) },
            }));
        }
        let (frame, call_event) = if frames.is_empty() {
            let none = json!({ "not": {}, "description": "This app declares no events." });
            (none.clone(), none)
        } else {
            (json!({ "oneOf": frames }), json!({ "oneOf": call_events }))
        };
        let _ = self.schemas.insert("EventFrame".into(), frame);
        let _ = self.schemas.insert("CallEvent".into(), call_event);
        json!({
            "get": {
                "operationId": "events",
                "tags": ["events"],
                "summary": "The app's events",
                "description": "Server-sent events: the committed events from log index `from`, then live ones. Each frame's `event` is the event's name, `id` is `<index>:<seq>` and `data` its fields as JSON. A subscriber receives only the events it is in the audience of.",
                "parameters": [
                    { "$ref": "#/components/parameters/app" },
                    { "name": "from", "in": "query", "required": false, "schema": { "type": "integer", "minimum": 0, "default": 0 } },
                ],
                "responses": {
                    "200": {
                        "description": "An event stream.",
                        "content": { "text/event-stream": { "schema": reference("EventFrame") } },
                    },
                    "401": response_ref("Unauthorized"),
                    "403": response_ref("Forbidden"),
                    "404": response_ref("NotFound"),
                    "429": response_ref("RateLimited"),
                },
            },
        })
    }
}

fn int_pattern(int: Int) -> &'static str {
    if int.signed() {
        "^(0|-?[1-9][0-9]*)$"
    } else {
        "^(0|[1-9][0-9]*)$"
    }
}

fn int_schema(int: Int) -> Value {
    let (lo, hi) = int.range();
    // Every range fits i64 below and u64 above.
    let lo = i64::try_from(lo).unwrap_or(i64::MIN);
    let hi = u64::try_from(hi).unwrap_or(u64::MAX);
    if int.is_string() {
        return json!({
            "type": "string",
            "pattern": int_pattern(int),
            "description": format!("A {} as a decimal string ({lo}..={hi}).", int.name()),
            "x-calimero-type": int.name(),
        });
    }
    let mut s = json!({ "type": "integer", "minimum": lo, "maximum": hi });
    let format = match int {
        Int::I32 | Int::Isize => Some("int32"),
        _ => None,
    };
    if let Some(format) = format {
        s["format"] = format.into();
    }
    s["x-calimero-type"] = int.name().into();
    if matches!(int, Int::U128 | Int::I128) {
        s["description"] = format!(
            "A {} as a JSON number; it encodes only within 64 bits, and JavaScript loses precision past 2^53.",
            int.name()
        )
        .into();
    }
    s
}

fn common_schemas() -> Map<String, Value> {
    let mut schemas = Map::new();
    let _ = schemas.insert(
        "ApiError".into(),
        json!({
            "type": "object",
            "required": ["error", "message"],
            "properties": {
                "error": { "type": "string", "description": "A stable code, e.g. `NOT_FOUND`." },
                "message": { "type": "string" },
                "details": {},
            },
        }),
    );
    let _ = schemas.insert(
        "ConfirmRequired".into(),
        json!({
            "type": "object",
            "required": ["error", "message", "details"],
            "properties": {
                "error": { "const": "CONFIRM_REQUIRED" },
                "message": { "type": "string" },
                "details": {
                    "type": "object",
                    "required": ["challengeId", "challenge", "rpId", "allowCredentials"],
                    "properties": {
                        "challengeId": { "type": "string" },
                        "challenge": { "type": "string", "contentEncoding": "base64url" },
                        "rpId": { "type": "string" },
                        "allowCredentials": { "type": "array", "items": { "type": "string" } },
                    },
                },
            },
        }),
    );
    let _ = schemas.insert(
        "Confirm".into(),
        json!({
            "type": "object",
            "description": "The passkey's approval of this exact call: the 412's challenge id and the WebAuthn assertion over its challenge.",
            "required": ["challengeId", "assertion"],
            "properties": {
                "challengeId": { "type": "string" },
                "assertion": {
                    "type": "object",
                    "required": ["credentialId", "authenticatorData", "clientDataJson", "signature"],
                    "properties": {
                        "credentialId": { "type": "string" },
                        "authenticatorData": { "type": "string" },
                        "clientDataJson": { "type": "string" },
                        "signature": { "type": "string" },
                    },
                },
            },
        }),
    );
    let _ = schemas.insert(
        "Receipt".into(),
        json!({
            "type": "object",
            "description": "The answering node's signature over the app, the index and the result's hash.",
            "required": ["app", "index", "resultHash", "node", "key", "signature"],
            "properties": {
                "app": { "type": "string" },
                "index": { "type": "integer", "minimum": 0 },
                "resultHash": { "type": "string" },
                "node": { "type": "integer", "minimum": 0 },
                "key": { "type": "string" },
                "signature": { "type": "string" },
            },
        }),
    );
    let _ = schemas.insert(
        "Trap".into(),
        json!({
            "type": "object",
            "required": ["error", "message", "details"],
            "properties": {
                "error": { "const": "TRAP" },
                "message": { "type": "string" },
                "details": {
                    "type": "object",
                    "properties": {
                        "index": { "type": "integer" },
                        "kind": { "type": "string" },
                        "detail": {},
                    },
                },
            },
        }),
    );
    schemas
}

fn responses(abi: &AppAbi) -> Value {
    let api_error = |description: &str| {
        json!({
            "description": description,
            "content": { "application/json": { "schema": reference("ApiError") } },
        })
    };
    let mut app_errors: Vec<Value> = abi
        .errors
        .keys()
        .map(|code| {
            json!({
                "type": "object",
                "required": ["error", "message", "details"],
                "properties": {
                    "error": { "const": "APP_ERROR" },
                    "message": { "type": "string" },
                    "details": {
                        "type": "object",
                        "required": ["code", "fields"],
                        "properties": {
                            "index": { "type": "integer" },
                            "code": { "const": code },
                            "fields": reference(&error_name(code)),
                        },
                    },
                },
            })
        })
        .collect();
    app_errors.push(reference("Trap"));
    json!({
        "BadRequest": api_error("The request is malformed (`BAD_REQUEST`), names a reserved method (`RESERVED_METHOD`), or carries an approval for another call (`WRONG_CHALLENGE`)."),
        "Unauthorized": api_error("No valid session or API key, and the method is not public."),
        "Forbidden": api_error("The caller may not call this method of this app (`FORBIDDEN`), or the app is read-only."),
        "NotFound": api_error("No such app here."),
        "Gone": api_error("`UNSUPPORTED_ENGINE`: the app is an SDK 0.12 app, which this node no longer runs."),
        "RateLimited": api_error("`RATE_LIMITED`: a public method's rate, or the node's limits."),
        "Unavailable": api_error("`UNAVAILABLE` or `TIMEOUT`: retry with the same idempotency key."),
        "ConfirmRequired": {
            "description": "`CONFIRM_REQUIRED`: approve the call with the passkey and send it again with `confirm`.",
            "content": { "application/json": { "schema": reference("ConfirmRequired") } },
        },
        "AppError": {
            "description": "The app refused the call (`APP_ERROR`, one of its declared errors) or the run trapped (`TRAP`).",
            "content": { "application/json": { "schema": { "oneOf": app_errors } } },
        },
    })
}

/// Each error's fields, which the `AppError` response refers to.
fn error_schemas(abi: &AppAbi, ctx: &mut Ctx) {
    for (code, fields) in &abi.errors {
        let schema = ctx.fields(fields);
        let _ = ctx.schemas.insert(error_name(code), schema);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(pascal("getOrInsert"), "GetOrInsert");
        assert_eq!(pascal("not_found"), "NotFound");
        assert_eq!(type_name("Page<Booking>"), "Page_Booking");
        assert_eq!(type_name("crate::types::Room"), "Room");
        assert_eq!(type_name("Either<u8,String>"), "Either_u8_String");
    }
}
