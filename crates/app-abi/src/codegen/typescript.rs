//! The TypeScript client: interfaces for arguments, errors and events, and a class whose
//! methods call mero-js's `CalimeroClient`.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::abi::{AppAbi, Fields};
use crate::openapi::{all_events, args_name, error_name, event_name, Names};
use crate::ty::Type;

struct Ts {
    names: Names,
    /// App types met so far: component name to ABI text.
    named: BTreeMap<String, String>,
}

impl Ts {
    fn ty(&mut self, ty: &Type) -> String {
        match ty {
            Type::Unit => "null".into(),
            Type::Bool => "boolean".into(),
            Type::Int(int) if int.is_string() => "string".into(),
            Type::Int(_) | Type::Float(_) => "number".into(),
            Type::Char | Type::String | Type::Bytes => "string".into(),
            Type::Option(inner) => format!("{} | null", self.ty(inner)),
            Type::List(inner) | Type::Set(inner) | Type::Array(inner, _) => {
                let inner = self.ty(inner);
                if inner.contains(' ') {
                    format!("Array<{inner}>")
                } else {
                    format!("{inner}[]")
                }
            }
            Type::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|t| self.ty(t)).collect();
                format!("[{}]", items.join(", "))
            }
            Type::Map(_, value) => format!("Record<string, {}>", self.ty(value)),
            Type::Named(text) => {
                let (name, _) = self.names.named(text);
                let _ = self.named.insert(name.clone(), text.clone());
                name
            }
        }
    }

    fn interface(&mut self, out: &mut String, doc: &str, name: &str, fields: &[(String, Type)]) {
        let _ = writeln!(out, "/** {doc} */");
        if fields.is_empty() {
            let _ = writeln!(out, "export type {name} = Record<string, never>;\n");
            return;
        }
        let _ = writeln!(out, "export interface {name} {{");
        for (field, ty) in fields {
            let opt = if ty.is_optional() { "?" } else { "" };
            let ty = self.ty(ty);
            let _ = writeln!(out, "  {}{opt}: {ty};", property(field));
        }
        let _ = writeln!(out, "}}\n");
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn property(name: &str) -> String {
    if is_ident(name) {
        return name.to_owned();
    }
    format!("{name:?}")
}

fn string_literal(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn fields_vec(fields: &Fields) -> Vec<(String, Type)> {
    fields.iter().map(|(k, t)| (k.clone(), t.clone())).collect()
}

pub(super) fn module(abi: &AppAbi, class: &str) -> String {
    let mut ts = Ts {
        names: Names::new(abi),
        named: BTreeMap::new(),
    };
    let mut body = String::new();

    for (name, m) in abi.client_methods() {
        if m.params.is_empty() {
            continue;
        }
        ts.interface(
            &mut body,
            &format!("The arguments of `{name}`."),
            &args_name(name),
            &m.params,
        );
    }
    for (code, fields) in &abi.errors {
        ts.interface(
            &mut body,
            &format!("The fields of the app error `{code}`."),
            &error_name(code),
            &fields_vec(fields),
        );
    }
    let codes: Vec<String> = abi.errors.keys().map(|c| string_literal(c)).collect();
    let _ = writeln!(
        body,
        "/** The app's error codes: `details.code` of a `422 APP_ERROR`. */\nexport type AppErrorCode = {};\n",
        if codes.is_empty() {
            "never".to_owned()
        } else {
            codes.join(" | ")
        }
    );
    let events = all_events(abi);
    for (event, fields) in &events {
        ts.interface(
            &mut body,
            &format!("The fields of the event `{event}`."),
            &event_name(event),
            &fields_vec(fields),
        );
    }
    let variants: Vec<String> = events
        .keys()
        .map(|e| format!("{{ kind: {}; data: {} }}", string_literal(e), event_name(e)))
        .collect();
    let _ = writeln!(
        body,
        "/** An event of the app, as a call's answer and the event stream carry it. */\nexport type AppEvent =\n  | {};\n",
        if variants.is_empty() {
            "never".to_owned()
        } else {
            variants.join("\n  | ")
        }
    );

    let has_events_method = abi.client_methods().any(|(n, _)| n == "events");
    let _ = writeln!(
        body,
        "/** Options of a call: a retry with the same key runs once. */"
    );
    let _ = writeln!(
        body,
        "export interface CallOptions {{\n  idempotencyKey?: string;\n}}\n"
    );
    let _ = writeln!(
        body,
        "/** Options of a view: `stale` reads any replica at least as new as `minIndex`. */"
    );
    let _ = writeln!(
        body,
        "export interface ViewOptions {{\n  stale?: boolean;\n  minIndex?: number;\n}}\n"
    );

    let _ = writeln!(
        body,
        "/** The app's methods, over a signed-in `CalimeroClient`. */"
    );
    let _ = writeln!(body, "export class {class} {{");
    let _ = writeln!(body, "  readonly #client: CalimeroClient;");
    let _ = writeln!(body, "  readonly #app: string;\n");
    let _ = writeln!(
        body,
        "  constructor(client: CalimeroClient, app: string) {{\n    this.#client = client;\n    this.#app = app;\n  }}"
    );
    for (name, m) in abi.client_methods() {
        let ret = ts.ty(&m.returns);
        let method = property(name);
        let lit = string_literal(name);
        let (args_param, args_value) = if m.params.is_empty() {
            (String::new(), "{}".to_owned())
        } else {
            (format!("args: {}, ", args_name(name)), "args".to_owned())
        };
        let mut doc = format!("{} `{name}`", m.kind.name());
        if m.confirm {
            doc.push_str("; `passkey` approves it (`confirm`)");
        }
        if m.public {
            doc.push_str("; public");
        }
        let _ = writeln!(body, "\n  /** {doc}. */");
        if m.runs_as_view() {
            let _ = writeln!(
                body,
                "  async {method}({args_param}options: ViewOptions = {{}}): Promise<{ret}> {{\n    const {{ result }} = await this.#client.view(this.#app, {lit}, {args_value}, options);\n    return result as {ret};\n  }}"
            );
        } else if m.confirm {
            let _ = writeln!(
                body,
                "  async {method}({args_param}passkey: PasskeyProvider, options: CallOptions = {{}}): Promise<{ret}> {{\n    const {{ result }} = await this.#client.callConfirmed(\n      this.#app,\n      {lit},\n      {args_value},\n      passkey,\n      options.idempotencyKey,\n    );\n    return result as {ret};\n  }}"
            );
        } else {
            let _ = writeln!(
                body,
                "  async {method}({args_param}options: CallOptions = {{}}): Promise<{ret}> {{\n    const {{ result }} = await this.#client.call(this.#app, {lit}, {args_value}, options.idempotencyKey);\n    return result as {ret};\n  }}"
            );
        }
    }
    let stream = if has_events_method {
        "appEvents"
    } else {
        "events"
    };
    let _ = writeln!(
        body,
        "\n  /** The app's events from log index `from`, then live, until `signal` aborts. */\n  async *{stream}(from = 0, signal?: AbortSignal): AsyncGenerator<AppEvent & {{ id: string }}> {{\n    for await (const event of this.#client.events(this.#app, from, signal)) {{\n      yield event as AppEvent & {{ id: string }};\n    }}\n  }}\n}}"
    );

    let mut out = String::new();
    let _ = writeln!(out, "// {}", super::provenance(abi));
    let _ = writeln!(
        out,
        "// A thin typed layer over mero-js: each method calls `CalimeroClient.call`, `callConfirmed`\n// or `view`, so it travels the sealed channel with the client's retries.\n"
    );
    let confirm = abi
        .client_methods()
        .any(|(_, m)| m.confirm && !m.runs_as_view());
    let imports = if confirm {
        "CalimeroClient, PasskeyProvider"
    } else {
        "CalimeroClient"
    };
    let _ = writeln!(
        out,
        "import type {{ {imports} }} from '@calimero-network/mero-js';\n"
    );
    for (name, text) in &ts.named {
        let _ = writeln!(
            out,
            "/** The app type `{text}`: ABI version 1 names it without its shape. */\nexport type {name} = unknown;\n"
        );
    }
    out.push_str(&body);
    out
}
