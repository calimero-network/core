//! The Python client: `TypedDict`s for errors and events, and a class whose methods send
//! calimero-client-py's `client.apps.call` and `client.apps.view` (answering JSON:
//! `client.json`), with keyword arguments in snake_case.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::abi::{AppAbi, Fields};
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

struct Py {
    names: Names,
    named: BTreeMap<String, String>,
}

impl Py {
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
                let (name, _) = self.names.named(text);
                let _ = self.named.insert(name.clone(), text.clone());
                name
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
        names: Names::new(abi),
        named: BTreeMap::new(),
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
        for (p, t) in params {
            let mut arg = snake(p);
            while !taken.insert(arg.clone()) {
                arg.push('_');
            }
            let ty = py.ty(t);
            if t.is_optional() {
                sig.push(format!("{arg}: {ty} = None"));
            } else {
                sig.push(format!("{arg}: {ty}"));
            }
            dict.push(format!("{}: {arg}", literal(p)));
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
            "\n    def {method}({}) -> {ret}:\n        \"\"\"{doc}.\"\"\"\n        args: Dict[str, Any] = {{{}}}\n        return cast({}, {send})",
            sig.join(", "),
            dict.join(", "),
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
        "from typing import Any, Dict, List, Literal, Optional, TypedDict, cast\n"
    );
    let _ = writeln!(out, "from calimero import Client, SoftPasskey\n");
    for (name, text) in &py.named {
        let _ = writeln!(
            out,
            "{name} = Any\n\"\"\"The app type `{text}`: ABI version 1 names it without its shape.\"\"\"\n"
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
