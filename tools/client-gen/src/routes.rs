//! The document's operations as methods: `routes.rs` of the generated client.
//!
//! One async method per operation, on the group its `x-calimero-group` names
//! (`client.orgs().invite_create(..)`), named as mero-js, calimero-client-py
//! and the Kotlin SDK name it: the operationId in snake case without a first
//! word that only repeats the group (`org_get` in `orgs` is `get`), unless the
//! rest would be a keyword or another method's name; `intents` is `call`,
//! `views` is `view`. Server-sent event streams are not methods here.
//!
//! Arguments are the path's parameters, then the JSON body's fields and the
//! query's (required ones first), flat; a body that would make more than six
//! arguments, that is not one plain object, that the document leaves open, or
//! whose fields share a parameter's name, is taken whole as its type. A raw
//! body is `bytes` (with `content_type` where the document takes any).
//! `confirm` and `operatorProof` are not arguments: a step-up route
//! (`x-calimero-step-up`) takes `passkey`, which answers its `412
//! CONFIRM_REQUIRED`, and an operator route (`x-calimero-operator`) takes
//! `operator`, which signs what its `428 OPERATOR_SIGNATURE_REQUIRED` names. A
//! retry-safe route (`x-calimero-retry-safe`) is sent again while the node is
//! unavailable, with the key `x-calimero-retry-requires` names (one is made
//! when none is given). A route `x-calimero-sealed: false` goes in the clear.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use eyre::{bail, eyre, Result as EyreResult};
use serde_json::Value;

use crate::naming::{ident, is_keyword, pascal};
use crate::types::{doc_comment, doc_text, Ty, Types};

/// Operation ids whose snake case names the route rather than what it does.
const RENAMES: &[(&str, &str)] = &[("intents", "call"), ("views", "view")];

/// At most this many arguments besides `self` (clippy's `too_many_arguments`
/// is seven, `self` included).
const MAX_ARGS: usize = 6;

const VERBS: &[&str] = &["get", "post", "put", "delete", "patch"];

/// How an argument reaches the request.
#[derive(Debug)]
enum Kind {
    Path,
    Query(Ty),
    Field(Ty, bool),
    Body(bool),
    ContentType,
    Bytes,
    Passkey,
    Operator,
}

#[derive(Debug)]
struct Arg {
    wire: String,
    name: String,
    kind: Kind,
}

impl Arg {
    fn rust(&self) -> String {
        match &self.kind {
            Kind::Path | Kind::ContentType => "&str".into(),
            Kind::Query(t) => arg_type(t, true),
            Kind::Field(t, required) => arg_type(t, *required),
            Kind::Body(_) => String::new(),
            Kind::Bytes => "Vec<u8>".into(),
            Kind::Passkey => "Option<&mut dyn Passkey>".into(),
            Kind::Operator => "Option<&OperatorSigner<'_>>".into(),
        }
    }
}

/// An argument's type: strings borrowed, the rest owned; `Option` for one
/// that may be left out, `Option<Option<T>>` for one that may also be `null`.
fn arg_type(t: &Ty, required: bool) -> String {
    let base = |t: &Ty| match t {
        Ty::String => "&str".to_owned(),
        Ty::Nullable(inner) if **inner == Ty::String => "Option<&str>".to_owned(),
        t => t.rust(),
    };
    match (t, required) {
        (t, true) => base(t),
        (t, false) => format!("Option<{}>", base(t)),
    }
}

struct Method {
    name: String,
    text: String,
}

/// The method name of `id` in `group` (the rules above), before collisions.
fn method_name(group: &str, id: &str) -> String {
    if let Some((_, n)) = RENAMES.iter().find(|(i, _)| *i == id) {
        return (*n).to_owned();
    }
    let mut prefixes = vec![group.to_owned()];
    if let Some(one) = group.strip_suffix('s') {
        prefixes.push(one.to_owned());
    }
    for p in prefixes {
        if let Some(rest) = id.strip_prefix(&format!("{p}_")) {
            if !rest.is_empty() && !is_keyword(rest) {
                return rest.to_owned();
            }
        }
    }
    id.to_owned()
}

/// The answer's media type and schema.
fn answer_of(op: &Value) -> Option<(&str, &Value)> {
    let ok = op["responses"]["200"]["content"].as_object()?;
    let (media, c) = ok.iter().next()?;
    Some((media.as_str(), &c["schema"]))
}

/// The operations, by group, as `routes.rs`.
pub fn emit(doc: &Value, types: &mut Types<'_>) -> EyreResult<(String, usize, Vec<String>)> {
    let mut groups: BTreeMap<String, Vec<(String, &Value, &str, &str)>> = BTreeMap::new();
    let mut streams = Vec::new();
    for (path, item) in doc["paths"].as_object().into_iter().flatten() {
        for (verb, op) in item.as_object().into_iter().flatten() {
            if !VERBS.contains(&verb.as_str()) {
                continue;
            }
            if answer_of(op).is_some_and(|(m, _)| m == "text/event-stream") {
                streams.push(format!("{} {path}", verb.to_uppercase()));
                continue;
            }
            let group = op["x-calimero-group"]
                .as_str()
                .ok_or_else(|| eyre!("{path}: no x-calimero-group"))?;
            let id = op["operationId"]
                .as_str()
                .ok_or_else(|| eyre!("{path}: no operationId"))?;
            groups.entry(group.to_owned()).or_default().push((
                method_name(group, id),
                op,
                path.as_str(),
                verb.as_str(),
            ));
        }
    }
    let mut out = String::new();
    let mut count = 0;
    for (group, ops) in &mut groups {
        let mut taken: BTreeMap<String, usize> = BTreeMap::new();
        for (name, ..) in ops.iter() {
            *taken.entry(name.clone()).or_default() += 1;
        }
        for (name, op, ..) in ops.iter_mut() {
            if taken[name.as_str()] > 1 {
                name.clone_from(&op["operationId"].as_str().unwrap_or_default().to_owned());
            }
        }
        let mut methods = Vec::new();
        for (name, op, path, verb) in ops.iter() {
            methods.push(method(types, name, op, path, verb)?);
        }
        methods.sort_by(|a, b| a.name.cmp(&b.name));
        let names: BTreeSet<&str> = methods.iter().map(|m| m.name.as_str()).collect();
        if names.len() != methods.len() {
            bail!("{group}: two methods of one name");
        }
        count += methods.len();
        let ty = pascal(group);
        let _ = writeln!(
            out,
            "/// The operations the document groups as `{group}`: `client.{group}()`.\n\
             #[derive(Clone, Copy)]\npub struct {ty}<'c> {{\n    client: &'c Client,\n}}\n\n\
             impl Client {{\n    /// The `{group}` operations.\n    #[must_use]\n    \
             pub fn {group}(&self) -> {ty}<'_> {{\n        {ty} {{ client: self }}\n    }}\n}}\n\n\
             impl {ty}<'_> {{"
        );
        for (i, m) in methods.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&m.text);
        }
        out.push_str("}\n\n");
    }
    Ok((out, count, streams))
}

fn method(
    types: &mut Types<'_>,
    name: &str,
    op: &Value,
    path: &str,
    verb: &str,
) -> EyreResult<Method> {
    let id = op["operationId"].as_str().unwrap_or_default();
    let op_type = pascal(id);
    let step_up = op["x-calimero-step-up"] == true;
    let operator = op["x-calimero-operator"] == true;
    let retry = op["x-calimero-retry-safe"] == true;
    let retry_key = op["x-calimero-retry-requires"].as_str();
    let sealed = op["x-calimero-sealed"] != false;
    let params = op["parameters"].as_array().cloned().unwrap_or_default();

    // Path parameters, in the order the path names them.
    let mut args: Vec<Arg> = Vec::new();
    let mut template = String::new();
    let mut rest = path.trim_start_matches('/');
    let mut path_args = Vec::new();
    while let Some(open) = rest.find('{') {
        template.push_str(&rest[..open]);
        let close = rest[open..]
            .find('}')
            .ok_or_else(|| eyre!("{path}: an unclosed parameter"))?
            + open;
        let wire = &rest[open + 1..close];
        template.push_str("{}");
        path_args.push(ident(wire));
        args.push(Arg {
            wire: wire.to_owned(),
            name: ident(wire),
            kind: Kind::Path,
        });
        rest = &rest[close + 1..];
    }
    template.push_str(rest);
    let path_names: BTreeSet<String> = args.iter().map(|a| a.wire.clone()).collect();

    let mut query: Vec<(Arg, bool)> = Vec::new();
    for p in params.iter().filter(|p| p["in"] == "query") {
        let wire = p["name"].as_str().unwrap_or_default();
        let required = p["required"] == true;
        let ty = types.resolve(&p["schema"], &format!("{op_type}{}", pascal(wire)))?;
        let name = if path_names.contains(wire) {
            ident(&format!("{wire}_query"))
        } else {
            ident(wire)
        };
        query.push((
            Arg {
                wire: wire.to_owned(),
                name,
                kind: Kind::Query(if required {
                    ty
                } else {
                    Ty::Nullable(Box::new(ty))
                }),
            },
            required,
        ));
    }

    // The body.
    let mut body_args: Vec<Arg> = Vec::new();
    let mut json_body = false;
    let mut body_struct: Option<(String, bool)> = None;
    if let Some(rb) = op["requestBody"].as_object() {
        let optional = rb.get("required") != Some(&Value::Bool(true));
        let (media, content) = rb["content"]
            .as_object()
            .and_then(|c| c.iter().next())
            .ok_or_else(|| eyre!("{id}: a body without content"))?;
        let schema = &content["schema"];
        if media != "application/json" || types.resolved(schema)["format"] == "binary" {
            if media.contains('*') {
                body_args.push(Arg {
                    wire: String::new(),
                    name: "content_type".into(),
                    kind: Kind::ContentType,
                });
            }
            body_args.push(Arg {
                wire: media.clone(),
                name: "bytes".into(),
                kind: Kind::Bytes,
            });
        } else {
            json_body = true;
            let mut resolved = types.resolved(schema).clone();
            if let Some(props) = resolved["properties"].as_object_mut() {
                if step_up {
                    let _confirm = props.remove("confirm");
                }
                if operator {
                    let _proof = props.remove("operatorProof");
                }
            }
            let plain = resolved.get("properties").is_some()
                && resolved.get("oneOf").is_none()
                && resolved.get("anyOf").is_none();
            let open = resolved
                .get("additionalProperties")
                .is_some_and(|a| a != false)
                || resolved["description"].as_str().is_some_and(|d| {
                    d.contains("Other properties than those listed may be present")
                });
            let props: Vec<(String, Value)> = resolved["properties"]
                .as_object()
                .map(|p| p.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            let collides = props
                .iter()
                .any(|(k, _)| path_names.contains(k) || query.iter().any(|(q, _)| q.wire == *k));
            let extras = usize::from(step_up) + usize::from(operator);
            let flat = plain
                && !open
                && !collides
                && args.len() + query.len() + props.len() + extras <= MAX_ARGS;
            if flat {
                let required: BTreeSet<&str> = resolved["required"]
                    .as_array()
                    .map(|r| r.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let mut req_fields = Vec::new();
                let mut opt_fields = Vec::new();
                for (wire, prop) in &props {
                    let ty = types.resolve(prop, &format!("{op_type}Request{}", pascal(wire)))?;
                    let is_req = required.contains(wire.as_str());
                    let a = Arg {
                        wire: wire.clone(),
                        name: ident(wire),
                        kind: Kind::Field(ty, is_req),
                    };
                    if is_req {
                        req_fields.push(a);
                    } else {
                        opt_fields.push(a);
                    }
                }
                body_args.extend(req_fields);
                body_args.extend(opt_fields);
            } else if !props.is_empty()
                || resolved.get("oneOf").is_some()
                || resolved.get("anyOf").is_some()
            {
                let ty = if schema.get("$ref").is_some() && !step_up && !operator {
                    types.resolve(schema, &format!("{op_type}Request"))?
                } else {
                    types.resolve(&resolved, &format!("{op_type}Request"))?
                };
                let Ty::Named(tname) = ty else {
                    bail!("{id}: a body that is not a named type");
                };
                body_struct = Some((tname, optional));
                body_args.push(Arg {
                    wire: String::new(),
                    name: "body".into(),
                    kind: Kind::Body(optional),
                });
            }
        }
    }

    // Arguments, in order: path, required query, body, optional query, approvals.
    let (req_q, opt_q): (Vec<_>, Vec<_>) = query.into_iter().partition(|(_, r)| *r);
    args.extend(req_q.into_iter().map(|(a, _)| a));
    args.extend(body_args);
    args.extend(opt_q.into_iter().map(|(a, _)| a));
    if step_up {
        args.push(Arg {
            wire: String::new(),
            name: "passkey".into(),
            kind: Kind::Passkey,
        });
    }
    if operator {
        args.push(Arg {
            wire: String::new(),
            name: "operator".into(),
            kind: Kind::Operator,
        });
    }
    if args.len() > MAX_ARGS {
        bail!("{id}: {} arguments", args.len());
    }

    // The answer.
    let (media, schema) = answer_of(op).unwrap_or(("application/json", &Value::Null));
    let (answer, reads) = if media == "text/plain" {
        ("String".to_owned(), "text")
    } else if media != "application/json" || types.resolved(schema)["format"] == "binary" {
        ("Vec<u8>".to_owned(), "bytes")
    } else {
        let ty = if schema.is_null() {
            Ty::Value
        } else {
            types.resolve(schema, &format!("{op_type}Response"))?
        };
        types.answer(&ty);
        (ty.rust(), "json")
    };

    // The doc comment.
    let mut text = String::new();
    let summary = op["summary"].as_str().unwrap_or(id).trim_end_matches('.');
    let mut doc = format!(
        "{summary}.\n\n`{} {path}` (`{id}`). Who: {}.",
        verb.to_uppercase(),
        op["x-calimero-who"].as_str().unwrap_or("see the document")
    );
    if step_up {
        let when = op["x-calimero-step-up-when"]
            .as_str()
            .map(|w| format!(" when {w}"))
            .unwrap_or_default();
        let _ = write!(
            doc,
            "\n\nStep-up: answers `412 CONFIRM_REQUIRED` first{when}; `passkey` answers it (user verification `{}`), without one the refusal is returned.",
            op["x-calimero-user-verification"].as_str().unwrap_or("required")
        );
    }
    if operator {
        doc.push_str(
            "\n\nOperator: answers `428 OPERATOR_SIGNATURE_REQUIRED` first where governance gives the decision to the operator; `operator` signs the statement it names, without it the refusal is returned.",
        );
    }
    if retry {
        let _ = write!(
            doc,
            "\n\nRetried while the node answers `503` or cannot be reached (D70){}.",
            retry_key
                .map(|k| format!(", with its `{k}` (one is made when none is given)"))
                .unwrap_or_default()
        );
    }
    if !sealed {
        doc.push_str("\n\nSent in the clear, as the document says (`x-calimero-sealed: false`), without a credential.");
    }
    doc_comment(&mut text, "    ", &doc_text(&doc));

    // The signature.
    let sig: Vec<String> = args
        .iter()
        .map(|a| match &a.kind {
            Kind::Body(optional) => {
                let (t, _) = body_struct
                    .as_ref()
                    .map_or(("Value".to_owned(), false), |(t, _)| (t.clone(), *optional));
                if *optional {
                    format!("body: Option<{t}>")
                } else {
                    format!("body: {t}")
                }
            }
            _ => format!("{}: {}", a.name, a.rust()),
        })
        .collect();
    let _ = writeln!(
        text,
        "    pub async fn {name}(self{}) -> Result<{answer}> {{",
        sig.iter().map(|s| format!(", {s}")).collect::<String>()
    );

    // The body of the method.
    let mut lines: Vec<String> = Vec::new();
    let path_expr = if path_args.is_empty() {
        format!("{template:?}")
    } else {
        format!(
            "format!({template:?}, {})",
            path_args
                .iter()
                .map(|p| format!("route::segment({p})"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let fields: Vec<&Arg> = args
        .iter()
        .filter(|a| matches!(a.kind, Kind::Field(..)))
        .collect();
    if let Some(key) = retry_key {
        let key_ident = ident(key);
        if fields.iter().any(|a| a.wire == key) {
            lines.push(format!(
                "let {key_ident} = Some({key_ident}.map_or_else(route::new_key, str::to_owned));"
            ));
        } else if body_struct.is_some() {
            lines.push("let mut body = body;".into());
            lines.push(format!(
                "if body.{key_ident}.is_none() {{ body.{key_ident} = Some(route::new_key()); }}"
            ));
        } else {
            bail!("{id}: retry needs `{key}`, which it does not take");
        }
    }
    let mut builder = format!("Request::new(Method::{}, {path_expr})", verb.to_uppercase());
    if json_body {
        if let Some((_, optional)) = &body_struct {
            if *optional {
                lines.push("let body = match body { Some(b) => route::value(&b)?, None => Value::Object(Map::new()) };".into());
            } else {
                lines.push("let body = route::value(&body)?;".into());
            }
        } else if fields.is_empty() {
            lines.push("let body = Value::Object(Map::new());".into());
        } else {
            lines.push("let mut fields = Map::new();".into());
            for a in &fields {
                let Kind::Field(_, required) = &a.kind else {
                    continue;
                };
                if *required {
                    lines.push(format!(
                        "route::put(&mut fields, {:?}, &{})?;",
                        a.wire, a.name
                    ));
                } else {
                    lines.push(format!(
                        "route::put_some(&mut fields, {:?}, {})?;",
                        a.wire, a.name
                    ));
                }
            }
            lines.push("let body = Value::Object(fields);".into());
        }
        builder.push_str(".json(body)");
    } else if let Some(bytes) = args.iter().find(|a| matches!(a.kind, Kind::Bytes)) {
        if args.iter().any(|a| matches!(a.kind, Kind::ContentType)) {
            builder.push_str(".bytes(content_type.to_owned(), bytes)");
        } else {
            let _ = write!(builder, ".bytes({:?}, bytes)", bytes.wire);
        }
    }
    let queries: Vec<&Arg> = args
        .iter()
        .filter(|a| matches!(a.kind, Kind::Query(_)))
        .collect();
    if queries.is_empty() {
        lines.push(format!("let req = {builder};"));
    } else {
        lines.push(format!("let mut req = {builder};"));
        for q in queries {
            let Kind::Query(t) = &q.kind else { continue };
            match t {
                Ty::Nullable(inner) => lines.push(format!(
                    "if let Some(v) = {} {{ req = req.query({:?}, {}); }}",
                    q.name,
                    q.wire,
                    query_value(inner, "v")
                )),
                t => lines.push(format!(
                    "req = req.query({:?}, {});",
                    q.wire,
                    query_value(t, &q.name)
                )),
            }
        }
    }
    let passkey = if step_up { "passkey" } else { "None" };
    let operator_arg = if operator { "operator" } else { "None" };
    let call = match (reads, sealed) {
        ("json", false) => "route::clear(self.client.transport(), req).await".to_owned(),
        ("json", true) => format!(
            "route::json(self.client.transport(), req, {passkey}, {operator_arg}, {retry}).await"
        ),
        ("text", _) => "route::text(self.client.transport(), req).await".to_owned(),
        _ => "route::bytes(self.client.transport(), req).await".to_owned(),
    };
    lines.push(call);
    for l in lines {
        let _ = writeln!(text, "        {l}");
    }
    text.push_str("    }\n");
    Ok(Method {
        name: name.to_owned(),
        text,
    })
}

/// A query value as the string it is sent as.
fn query_value(t: &Ty, v: &str) -> String {
    match t {
        Ty::String => v.to_owned(),
        Ty::Named(_) => format!("{v}.as_str()"),
        _ => format!("{v}.to_string()"),
    }
}
