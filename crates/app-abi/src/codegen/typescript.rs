//! The TypeScript client: interfaces for arguments, errors and events, and a class whose
//! methods call mero-js's `client.apps.call` and `client.apps.view`.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::abi::{AppAbi, Fields};
use crate::openapi::{all_events, args_name, error_name, event_name, Names};
use crate::ty::Type;

/// The names every module declares, kept from app types.
const OWN_NAMES: &[&str] = &[
    "AppErrorCode",
    "AppEvent",
    "AppTransport",
    "CallOptions",
    "ClientOptions",
    "ViewOptions",
];

/// Who writes the object a type describes: a caller (arguments) may leave an `option` out,
/// the app (errors, events) always writes it, `null` for none.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Input,
    Output,
}

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

    fn interface(
        &mut self,
        out: &mut String,
        doc: &str,
        name: &str,
        fields: &[(String, Type)],
        side: Side,
    ) {
        let _ = writeln!(out, "/** {doc} */");
        if fields.is_empty() {
            let _ = writeln!(out, "export type {name} = Record<string, never>;\n");
            return;
        }
        let _ = writeln!(out, "export interface {name} {{");
        for (field, ty) in fields {
            // The SDK decodes a missing `Option` argument as `None`, the same as `null`.
            let opt = if side == Side::Input && ty.is_optional() {
                "?"
            } else {
                ""
            };
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

/// The options and the transport: the same in every module.
const PRELUDE: &str = r"/** Options of a call. */
export interface CallOptions {
  /** A retry with the same key runs once; the client makes one when none is given. */
  idempotencyKey?: string;
  /**
   * Answers a `confirm` method's `412 CONFIRM_REQUIRED` step-up (`passkeyConfirm(passkey)`),
   * before the `confirm` the `CalimeroClient` was built with.
   */
  confirm?: ConfirmCallback;
  signal?: AbortSignal;
}

/** Options of a view: `stale` reads the answering replica, at least as new as `minIndex`. */
export interface ViewOptions {
  consistency?: 'strong' | 'stale';
  minIndex?: number;
  signal?: AbortSignal;
}

/** Options of the client. */
export interface ClientOptions {
  /**
   * Runs after every call and view, answered or refused: the place to store a session the
   * client refreshed meanwhile (`client.session`), as mero-js has no session-change event.
   */
  after?: () => void | Promise<void>;
}

/**
 * What the class uses of mero-js's `CalimeroClient` (`client.apps.call`, `client.apps.view`,
 * `client.events`): a test double implements this alone.
 */
export interface AppTransport {
  apps: {
    call(
      args: { app: string; method: string; args?: unknown; idempotencyKey?: string },
      options?: { confirm?: ConfirmCallback; signal?: AbortSignal },
    ): Promise<{ result: unknown }>;
    view(
      args: { app: string; method: string; args?: unknown; consistency?: 'strong' | 'stale'; minIndex?: number },
      options?: { signal?: AbortSignal },
    ): Promise<{ result: unknown }>;
  };
  events(app: string, from?: number, signal?: AbortSignal): AsyncIterable<NodeEvent>;
}
";

/// The class's fields, constructor and the two private senders every method goes through.
const CLASS_HEAD: &str = r"  readonly #client: AppTransport;
  readonly #app: string;
  readonly #after: ClientOptions['after'];

  constructor(client: AppTransport, app: string, options: ClientOptions = {}) {
    this.#client = client;
    this.#app = app;
    this.#after = options.after;
  }

  async #call(method: string, args: object, options: CallOptions): Promise<unknown> {
    const { idempotencyKey, ...rest } = options;
    const request = { app: this.#app, method, args, ...(idempotencyKey === undefined ? {} : { idempotencyKey }) };
    try {
      return (await this.#client.apps.call(request, rest)).result;
    } finally {
      await this.#after?.();
    }
  }

  async #view(method: string, args: object, options: ViewOptions): Promise<unknown> {
    const { consistency, minIndex, signal } = options;
    const request = {
      app: this.#app,
      method,
      args,
      ...(consistency === undefined ? {} : { consistency }),
      ...(minIndex === undefined ? {} : { minIndex }),
    };
    try {
      return (await this.#client.apps.view(request, signal === undefined ? {} : { signal })).result;
    } finally {
      await this.#after?.();
    }
  }
";

pub(super) fn module(abi: &AppAbi, class: &str) -> String {
    let mut names = Names::new(abi);
    for name in OWN_NAMES.iter().copied().chain([class]) {
        names.reserve(name);
    }
    let mut ts = Ts {
        names,
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
            Side::Input,
        );
    }
    for (code, fields) in &abi.errors {
        ts.interface(
            &mut body,
            &format!("The fields of the app error `{code}`."),
            &error_name(code),
            &fields_vec(fields),
            Side::Output,
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
            Side::Output,
        );
    }
    let variants: Vec<String> = events
        .keys()
        .map(|e| format!("{{ kind: {}; data: {} }}", string_literal(e), event_name(e)))
        .collect();
    let _ = writeln!(
        body,
        "/** An event of the app, as a call's answer (`events`) and the event stream carry it. */\nexport type AppEvent =\n  | {};\n",
        if variants.is_empty() {
            "never".to_owned()
        } else {
            variants.join("\n  | ")
        }
    );

    body.push_str(PRELUDE);
    let _ = writeln!(
        body,
        "\n/** The app's methods, over a signed-in `CalimeroClient` (or any {{@link AppTransport}}). */\nexport class {class} {{"
    );
    body.push_str(CLASS_HEAD);
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
            doc.push_str("; a passkey approves it (`options.confirm`, else the client's own)");
        }
        if m.public {
            doc.push_str("; public");
        }
        let (send, options) = if m.runs_as_view() {
            ("view", "ViewOptions")
        } else {
            ("call", "CallOptions")
        };
        let _ = writeln!(
            body,
            "\n  /** {doc}. */\n  async {method}({args_param}options: {options} = {{}}): Promise<{ret}> {{\n    return (await this.#{send}({lit}, {args_value}, options)) as {ret};\n  }}"
        );
    }
    let stream = if abi.client_methods().any(|(n, _)| n == "events") {
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
        "// A thin typed layer over mero-js: each method sends `client.apps.call` (through the app's\n// log) or `client.apps.view`, so it travels the sealed channel with the client's retries.\n"
    );
    let _ = writeln!(
        out,
        "import type {{ ConfirmCallback, NodeEvent }} from '@calimero-network/mero-js';\n"
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_app_type_named_like_the_module_s_own_is_renamed() {
        let abi = AppAbi::from_json(&json!({
            "abiVersion": 1,
            "methods": { "get": { "kind": "view", "params": [], "returns": "CallOptions" } },
        }))
        .unwrap();
        let ts = module(&abi, "AppClient");
        assert!(
            ts.contains("export type CallOptionsType = unknown;"),
            "{ts}"
        );
        assert!(ts.contains("Promise<CallOptionsType>"), "{ts}");
        assert_eq!(ts.matches("export interface CallOptions ").count(), 1);
    }
}
