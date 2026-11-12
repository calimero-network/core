//! The Python client: `TypedDict`s for errors, events and the app's records (from the ABI's
//! `types`; a generic type instantiated per use, an enum a `Union`), and a class whose methods send
//! calimero-client-py's `client.apps.call` and `client.apps.view` (answering JSON:
//! `client.json`), with keyword arguments in snake_case. An optional argument left `None` is not
//! sent: the SDK reads a missing argument as `None`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::abi::{AppAbi, EnumRepr, Field, Fields, Shape, VariantData};
use crate::openapi::{all_events, error_name, event_name, Names};
use crate::ty::Type;

const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield", "self",
];

/// The class's own attributes, kept from the app's method names.
const OWN_ATTRS: &[&str] = &["_app", "_call", "_client", "_view"];

/// The constructor and the two senders every method goes through: the facade's methods
/// answering JSON (`client.json.apps`), an option not given left out rather than sent as
/// `null`.
const CLASS_HEAD: &str = r#"    def __init__(self, client: Client, app: str) -> None:
        self._client = client
        self._app = app

    def _call(
        self,
        method: str,
        args: Dict[str, Any],
        idempotency_key: Optional[str],
        passkey: Optional[SoftPasskey] = None,
    ) -> Any:
        fields: Dict[str, Any] = {}
        if idempotency_key is not None:
            fields["idempotency_key"] = idempotency_key
        answer = self._client.json.apps.call(app=self._app, method=method, args=args, confirm=passkey, **fields)
        return answer["result"]

    def _view(
        self,
        method: str,
        args: Dict[str, Any],
        consistency: Optional[str],
        min_index: Optional[int],
    ) -> Any:
        fields: Dict[str, Any] = {}
        if consistency is not None:
            fields["consistency"] = consistency
        if min_index is not None:
            fields["min_index"] = min_index
        answer = self._client.json.apps.view(app=self._app, method=method, args=args, **fields)
        return answer["result"]
"#;

struct Py<'a> {
    abi: &'a AppAbi,
    names: Names,
    named: BTreeMap<String, String>,
    /// Described app types met so far: name to (ABI text, shape with arguments put in).
    described: BTreeMap<String, (String, Shape)>,
    /// Declarations of helper types an enum's variants need, by name.
    helpers: BTreeMap<String, String>,
    /// Inside a declaration, evaluated when the module loads: app types are written as
    /// strings (forward references), so the order of declarations does not matter.
    quoted: bool,
}

fn is_py_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&s)
}

impl Py<'_> {
    /// The reference to app type `name`, quoted inside declarations.
    fn reference(&self, name: String) -> String {
        if self.quoted {
            format!("{name:?}")
        } else {
            name
        }
    }

    /// A `TypedDict` of `fields` (with a `tag` naming the variant when given), declared under
    /// `name`: the class form when every key is an identifier, else the functional one.
    fn typed_dict_decl(
        &mut self,
        name: &str,
        doc: &str,
        fields: &[Field],
        tag: Option<(&str, &str)>,
    ) -> String {
        let mut entries: Vec<(String, String)> = Vec::new();
        if let Some((tag, variant)) = tag {
            entries.push((tag.to_owned(), format!("Literal[{variant:?}]")));
        }
        let mut optional: Vec<(String, String)> = Vec::new();
        for f in fields {
            let ty = self.ty(&f.ty);
            if f.optional {
                optional.push((f.name.clone(), ty));
            } else {
                entries.push((f.name.clone(), ty));
            }
        }
        if !optional.is_empty() {
            // Keys the JSON may leave out: a `total=False` part (`NotRequired` needs 3.11).
            let required = format!("_{name}Required");
            let rest = format!("_{name}Optional");
            let functional = |n: &str, es: &[(String, String)], total: &str| {
                let es: Vec<String> = es.iter().map(|(k, t)| format!("{k:?}: {t}")).collect();
                format!("{n} = TypedDict({n:?}, {{{}}}{total})\n", es.join(", "))
            };
            return format!(
                "{}{}class {name}({required}, {rest}):\n    \"\"\"{doc}\"\"\"\n",
                functional(&required, &entries, ""),
                functional(&rest, &optional, ", total=False"),
            );
        }
        if !entries.is_empty() && entries.iter().all(|(k, _)| is_py_ident(k)) {
            let mut out = format!("class {name}(TypedDict):\n    \"\"\"{doc}\"\"\"\n\n");
            for (k, t) in &entries {
                let _ = writeln!(out, "    {k}: {t}");
            }
            return out;
        }
        let entries: Vec<String> = entries.iter().map(|(k, t)| format!("{k:?}: {t}")).collect();
        format!(
            "{name} = TypedDict({name:?}, {{{}}})\n\"\"\"{doc}\"\"\"\n",
            entries.join(", ")
        )
    }

    /// A helper `TypedDict` for one variant, declared once; its quoted name.
    fn helper(
        &mut self,
        owner: &str,
        variant: &str,
        fields: &[Field],
        tag: Option<(&str, &str)>,
        inner: bool,
    ) -> String {
        let suffix = if inner { "Fields" } else { "" };
        let (name, _) = self.names.named(&format!("{owner}_{variant}{suffix}"));
        if !self.helpers.contains_key(&name) {
            let doc = if inner {
                format!("The fields of the variant `{variant}` of `{owner}`.")
            } else {
                format!("The variant `{variant}` of `{owner}`.")
            };
            let decl = self.typed_dict_decl(&name, &doc, fields, tag);
            let _ = self.helpers.insert(name.clone(), decl);
        }
        format!("{name:?}")
    }

    fn declaration(&mut self, name: &str, text: &str, shape: &Shape) -> String {
        self.quoted = true;
        let doc = format!("The app type `{text}`.");
        let out = match shape {
            Shape::Record(fields) => self.typed_dict_decl(name, &doc, fields, None),
            Shape::Newtype(t) => {
                let ty = self.ty(t);
                format!("{name} = {ty}\n\"\"\"{doc}\"\"\"\n")
            }
            Shape::Tuple(_) => format!("{name} = List[Any]\n\"\"\"{doc} A JSON array.\"\"\"\n"),
            Shape::Unit => format!("{name} = Literal[None]\n\"\"\"{doc}\"\"\"\n"),
            Shape::Enum { repr, variants } => {
                let mut arms = Vec::new();
                for v in variants {
                    let data = |py: &mut Self| match &v.data {
                        VariantData::Unit => "None".to_owned(),
                        VariantData::Newtype(t) => py.ty(t),
                        VariantData::Tuple(_) => "List[Any]".to_owned(),
                        VariantData::Record(fs) => py.helper(name, &v.name, fs, None, true),
                    };
                    arms.push(match (repr, &v.data) {
                        (EnumRepr::External, VariantData::Unit) => format!("Literal[{:?}]", v.name),
                        (EnumRepr::External, _) => {
                            let t = data(self);
                            self.helper_raw(name, &v.name, &[(v.name.clone(), t)])
                        }
                        (EnumRepr::Internal { tag }, VariantData::Record(fs)) => {
                            self.helper(name, &v.name, fs, Some((tag, &v.name)), false)
                        }
                        (EnumRepr::Internal { tag }, VariantData::Unit)
                        | (EnumRepr::Adjacent { tag, .. }, VariantData::Unit) => {
                            self.helper(name, &v.name, &[], Some((tag, &v.name)), false)
                        }
                        (EnumRepr::Internal { .. }, _) => "Dict[str, Any]".to_owned(),
                        (EnumRepr::Adjacent { tag, content }, _) => {
                            let t = data(self);
                            self.helper_raw(
                                name,
                                &v.name,
                                &[
                                    (tag.clone(), format!("Literal[{:?}]", v.name)),
                                    (content.clone(), t),
                                ],
                            )
                        }
                        (EnumRepr::Untagged, _) => data(self),
                    });
                }
                let ty = match arms.as_slice() {
                    [] => "Any".to_owned(),
                    [one] => one.clone(),
                    _ => format!("Union[{}]", arms.join(", ")),
                };
                format!("{name} = {ty}\n\"\"\"{doc}\"\"\"\n")
            }
        };
        self.quoted = false;
        out
    }

    /// A helper `TypedDict` of ready-made entries (key, Python type), declared once.
    fn helper_raw(&mut self, owner: &str, variant: &str, entries: &[(String, String)]) -> String {
        let (name, _) = self.names.named(&format!("{owner}_{variant}"));
        if !self.helpers.contains_key(&name) {
            let entries: Vec<String> = entries.iter().map(|(k, t)| format!("{k:?}: {t}")).collect();
            let decl = format!(
                "{name} = TypedDict({name:?}, {{{}}})\n\"\"\"The variant `{variant}` of `{owner}`.\"\"\"\n",
                entries.join(", ")
            );
            let _ = self.helpers.insert(name.clone(), decl);
        }
        format!("{name:?}")
    }

    fn ty(&mut self, ty: &Type) -> String {
        match ty {
            Type::Unit => "None".into(),
            Type::Bool => "bool".into(),
            Type::Int(int) if int.is_string() => "str".into(),
            Type::Int(_) => "int".into(),
            Type::Float(_) => "float".into(),
            Type::Char | Type::String | Type::Bytes => "str".into(),
            Type::Option(inner) => format!("Optional[{}]", self.ty(inner)),
            Type::List(inner) | Type::Set(inner) | Type::Array(inner, _) => {
                format!("List[{}]", self.ty(inner))
            }
            // JSON has no tuples: a tuple arrives as a list.
            Type::Tuple(_) => "List[Any]".into(),
            Type::Map(_, value) => format!("Dict[str, {}]", self.ty(value)),
            Type::Named(text) => {
                if let Some((entry, def, args)) = self.abi.shape_of(ty) {
                    // A generic type is declared once per instantiation (`Page_Booking`).
                    let key = if args.is_empty() {
                        entry
                    } else {
                        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
                        format!("{entry}<{}>", args.join(","))
                    };
                    let (name, new) = self.names.named(&key);
                    if new {
                        let _ = self
                            .described
                            .insert(name.clone(), (key, def.instantiate(&args)));
                    }
                    return self.reference(name);
                }
                let (name, _) = self.names.named(text);
                let _ = self.named.insert(name.clone(), text.clone());
                self.reference(name)
            }
        }
    }

    fn typed_dict(&mut self, out: &mut String, name: &str, fields: &Fields) {
        let entries: Vec<String> = fields
            .iter()
            .map(|(k, t)| format!("{k:?}: {}", self.ty(t)))
            .collect();
        let _ = writeln!(
            out,
            "{name} = TypedDict({name:?}, {{{}}})",
            entries.join(", ")
        );
    }
}

/// `roomId` → `room_id`, kept clear of Python keywords.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else if c.is_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if KEYWORDS.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

fn literal(s: &str) -> String {
    format!("{s:?}")
}

pub(super) fn module(abi: &AppAbi, class: &str) -> String {
    let mut py = Py {
        abi,
        names: Names::new(abi),
        named: BTreeMap::new(),
        described: BTreeMap::new(),
        helpers: BTreeMap::new(),
        quoted: false,
    };
    let mut body = String::new();

    for (code, fields) in &abi.errors {
        py.typed_dict(&mut body, &error_name(code), fields);
    }
    let codes: Vec<String> = abi.errors.keys().map(|c| literal(c)).collect();
    if codes.is_empty() {
        let _ = writeln!(body, "AppErrorCode = str");
    } else {
        let _ = writeln!(body, "AppErrorCode = Literal[{}]", codes.join(", "));
    }
    let _ = writeln!(
        body,
        "\"\"\"The app's error codes: `details[\"code\"]` of a 422 APP_ERROR.\"\"\"\n"
    );
    let events = all_events(abi);
    for (event, fields) in &events {
        py.typed_dict(&mut body, &event_name(event), fields);
    }
    if !events.is_empty() {
        body.push('\n');
    }

    let _ = writeln!(body, "\nclass {class}:");
    let _ = writeln!(
        body,
        "    \"\"\"The app's methods, over a signed-in `calimero.Client`.\"\"\"\n"
    );
    body.push_str(CLASS_HEAD);
    let mut used: BTreeSet<String> = OWN_ATTRS.iter().map(|a| (*a).to_owned()).collect();
    for (name, m) in abi.client_methods() {
        let mut method = snake(name);
        while !used.insert(method.clone()) {
            method.push('_');
        }
        let ret = py.ty(&m.returns);
        let view = m.runs_as_view();
        let reserved: &[&str] = if view {
            &["consistency", "min_index"]
        } else if m.confirm {
            &["passkey", "idempotency_key"]
        } else {
            &["idempotency_key"]
        };
        // Required parameters first: Python wants defaults last.
        let mut params: Vec<(&String, &Type)> = m.params.iter().map(|(k, t)| (k, t)).collect();
        params.sort_by_key(|(_, t)| t.is_optional());
        let mut taken: BTreeSet<String> = reserved.iter().map(|r| (*r).to_owned()).collect();
        let mut sig = vec!["self".to_owned()];
        let mut dict = Vec::new();
        let mut unset = Vec::new();
        for (p, t) in params {
            let mut arg = snake(p);
            while !taken.insert(arg.clone()) {
                arg.push('_');
            }
            let ty = py.ty(t);
            if t.is_optional() {
                sig.push(format!("{arg}: {ty} = None"));
                // An unset optional argument is left out: the SDK reads it as `None`.
                unset.push(format!(
                    "\n        if {arg} is not None:\n            args[{}] = {arg}",
                    literal(p)
                ));
            } else {
                sig.push(format!("{arg}: {ty}"));
                dict.push(format!("{}: {arg}", literal(p)));
            }
        }
        sig.push("*".into());
        if view {
            sig.push("consistency: Optional[Literal[\"strong\", \"stale\"]] = None".into());
            sig.push("min_index: Optional[int] = None".into());
        } else {
            if m.confirm {
                sig.push("passkey: Optional[SoftPasskey] = None".into());
            }
            sig.push("idempotency_key: Optional[str] = None".into());
        }
        let lit = literal(name);
        let mut doc = format!("{} `{name}`", m.kind.name());
        if m.confirm {
            doc.push_str("; `passkey` approves it (confirm), else the client's own");
        }
        if m.public {
            doc.push_str("; public");
        }
        let send = if view {
            format!("self._view({lit}, args, consistency, min_index)")
        } else if m.confirm {
            format!("self._call({lit}, args, idempotency_key, passkey)")
        } else {
            format!("self._call({lit}, args, idempotency_key)")
        };
        let _ = writeln!(
            body,
            "\n    def {method}({}) -> {ret}:\n        \"\"\"{doc}.\"\"\"\n        args: Dict[str, Any] = {{{}}}{}\n        return cast({}, {send})",
            sig.join(", "),
            dict.join(", "),
            unset.concat(),
            ret,
        );
    }
    let stream = if used.contains("events") {
        "app_events"
    } else {
        "events"
    };
    let _ = writeln!(
        body,
        "\n    def {stream}(self, start: int = 0, limit: int = 100, timeout: float = 5.0) -> List[Dict[str, Any]]:\n        \"\"\"The app's events from log index `start`: each `{{\"id\", \"kind\", \"data\"}}`.\"\"\"\n        return cast(List[Dict[str, Any]], self._client.events(self._app, start, limit, timeout))"
    );

    let mut out = String::new();
    let _ = writeln!(out, "\"\"\"{}", super::provenance(abi));
    let _ = writeln!(
        out,
        "\nA thin typed layer over calimero-client-py: each method sends `client.apps.call`\n(through the app's log) or `client.apps.view`, so it travels the sealed channel with the\nclient's retries. u64 and i64 are decimal strings and bytes base64url, as on the wire.\n\"\"\"\n"
    );
    let _ = writeln!(out, "from __future__ import annotations\n");
    let _ = writeln!(
        out,
        "from typing import Any, Dict, List, Literal, Optional, TypedDict, Union, cast\n"
    );
    let _ = writeln!(out, "from calimero import Client, SoftPasskey\n");
    // The described types, and the ones their shapes name, until none is new.
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let pending: Vec<(String, (String, Shape))> = py
            .described
            .iter()
            .filter(|(name, _)| !declared.contains_key(*name))
            .map(|(n, d)| (n.clone(), d.clone()))
            .collect();
        if pending.is_empty() {
            break;
        }
        for (name, (text, shape)) in pending {
            let decl = py.declaration(&name, &text, &shape);
            let _ = declared.insert(name, decl);
        }
    }
    declared.append(&mut py.helpers);
    for decl in declared.values() {
        let _ = writeln!(out, "{decl}");
    }
    for (name, text) in &py.named {
        let _ = writeln!(
            out,
            "{name} = Any\n\"\"\"The app type `{text}`: the ABI names it without its shape.\"\"\"\n"
        );
    }
    out.push_str(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_names() {
        assert_eq!(snake("roomId"), "room_id");
        assert_eq!(snake("getOrInsert"), "get_or_insert");
        assert_eq!(snake("from"), "from_");
        assert_eq!(snake("len"), "len");
    }

    #[test]
    fn app_methods_keep_clear_of_the_class_s_own_names() {
        let abi = AppAbi::from_json(&serde_json::json!({
            "abiVersion": 1,
            "methods": {
                "_call": { "kind": "call", "params": [] },
                "getX": { "kind": "view", "params": [] },
                "get_x": { "kind": "view", "params": [] },
            },
        }))
        .unwrap();
        let py = module(&abi, "AppClient");
        assert!(
            py.contains("    def _call_(self, *, idempotency_key"),
            "{py}"
        );
        assert!(py.contains("    def get_x(self, *,"), "{py}");
        assert!(py.contains("    def get_x_(self, *,"), "{py}");
    }
}
