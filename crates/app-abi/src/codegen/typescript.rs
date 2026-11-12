//! The TypeScript client: interfaces for arguments (`init`'s too, as `InitArgs`), errors,
//! events and the app's own types (from the ABI's `types`: an interface per record, a union per
//! enum, generic where the Rust type is), and a class whose methods call mero-js's
//! `client.apps.call` and `client.apps.view`, answering the result, and whose `raw` methods
//! answer it whole (`{ result, index, events }`).

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::abi::{AppAbi, EnumRepr, Field, Fields, Kind, Shape, TypeDef, VariantData};
use crate::openapi::{all_events, args_name, error_name, event_name, Names};
use crate::ty::Type;

/// The names every module declares, kept from app types.
const OWN_NAMES: &[&str] = &[
    "AppCallTransport",
    "AppErrorCode",
    "AppEvent",
    "AppTransport",
    "CallAnswer",
    "CallOptions",
    "ClientOptions",
    "ViewAnswer",
    "ViewOptions",
];

/// The arguments of the app's `init`, declared when it takes any.
const INIT_ARGS: &str = "InitArgs";

/// Who writes the object a type describes: a caller (arguments) may leave an `option` out,
/// the app (errors, events) always writes it, `null` for none.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Input,
    Output,
}

struct Ts<'a> {
    abi: &'a AppAbi,
    names: Names,
    /// App types the ABI does not describe, met so far: component name to ABI text.
    named: BTreeMap<String, String>,
    /// App types it describes, met so far: component name to entry.
    described: BTreeMap<String, (String, &'a TypeDef)>,
    /// The generic parameters in scope (inside a generic entry's declaration).
    params: Vec<String>,
}

impl<'a> Ts<'a> {
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
            Type::Named(text) if self.params.contains(text) => text.clone(),
            Type::Named(text) => {
                if let Some((entry, def, args)) = self.abi.shape_of(ty) {
                    let (name, _) = self.names.named(&entry);
                    let _ = self.described.insert(name.clone(), (entry, def));
                    if args.is_empty() {
                        return name;
                    }
                    let args: Vec<String> = args.iter().map(|a| self.ty(a)).collect();
                    return format!("{name}<{}>", args.join(", "));
                }
                let (name, _) = self.names.named(text);
                let _ = self.named.insert(name.clone(), text.clone());
                name
            }
        }
    }

    /// `{ a: x; b?: y }`, a tag property first when given.
    fn object(&mut self, fields: &[Field], tag: Option<(&str, &str)>) -> String {
        let mut parts = Vec::new();
        if let Some((tag, name)) = tag {
            parts.push(format!("{}: {}", property(tag), string_literal(name)));
        }
        for f in fields {
            let opt = if f.optional { "?" } else { "" };
            parts.push(format!("{}{opt}: {}", property(&f.name), self.ty(&f.ty)));
        }
        if parts.is_empty() {
            return "Record<string, never>".to_owned();
        }
        format!("{{ {} }}", parts.join("; "))
    }

    fn data(&mut self, data: &VariantData) -> String {
        match data {
            VariantData::Unit => "null".to_owned(),
            VariantData::Newtype(t) => self.ty(t),
            VariantData::Tuple(ts) => self.ty(&Type::Tuple(ts.clone())),
            VariantData::Record(fs) => self.object(fs, None),
        }
    }

    /// The declaration of a described app type.
    fn declaration(&mut self, name: &str, entry: &str, def: &TypeDef) -> String {
        self.params.clone_from(&def.params);
        let generics = if def.params.is_empty() {
            String::new()
        } else {
            format!("<{}>", def.params.join(", "))
        };
        let mut out = format!(
            "/** The app type `{entry}`. */
"
        );
        match &def.shape {
            Shape::Record(fields) => {
                let _ = writeln!(out, "export interface {name}{generics} {{");
                for f in fields {
                    let opt = if f.optional { "?" } else { "" };
                    let ty = self.ty(&f.ty);
                    let _ = writeln!(out, "  {}{opt}: {ty};", property(&f.name));
                }
                out.push_str(
                    "}
",
                );
            }
            Shape::Newtype(t) => {
                let ty = self.ty(t);
                let _ = writeln!(out, "export type {name}{generics} = {ty};");
            }
            Shape::Tuple(items) => {
                let ty = self.ty(&Type::Tuple(items.clone()));
                let _ = writeln!(out, "export type {name}{generics} = {ty};");
            }
            Shape::Unit => {
                let _ = writeln!(out, "export type {name}{generics} = null;");
            }
            Shape::Enum { repr, variants } => {
                let arms: Vec<String> = variants
                    .iter()
                    .map(|v| match (repr, &v.data) {
                        (EnumRepr::External, VariantData::Unit) => string_literal(&v.name),
                        (EnumRepr::External, data) => {
                            let data = self.data(data);
                            format!("{{ {}: {data} }}", property(&v.name))
                        }
                        (EnumRepr::Internal { tag }, VariantData::Record(fs)) => {
                            self.object(fs, Some((tag, &v.name)))
                        }
                        (EnumRepr::Internal { tag }, VariantData::Unit)
                        | (EnumRepr::Adjacent { tag, .. }, VariantData::Unit) => {
                            self.object(&[], Some((tag, &v.name)))
                        }
                        (EnumRepr::Internal { tag }, data) => {
                            let data = self.data(data);
                            format!("({} & {data})", self.object(&[], Some((tag, &v.name))))
                        }
                        (EnumRepr::Adjacent { tag, content }, data) => {
                            let data = self.data(data);
                            format!(
                                "{{ {}: {}; {}: {data} }}",
                                property(tag),
                                string_literal(&v.name),
                                property(content)
                            )
                        }
                        (EnumRepr::Untagged, data) => self.data(data),
                    })
                    .collect();
                if arms.is_empty() {
                    let _ = writeln!(out, "export type {name}{generics} = never;");
                } else {
                    let _ = writeln!(
                        out,
                        "export type {name}{generics} =\n  | {};",
                        arms.join("\n  | ")
                    );
                }
            }
        }
        self.params.clear();
        out
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

/** A call's whole answer (`raw`): its result, the log index it committed at, its events. */
export interface CallAnswer<T> {
  result: T;
  /** The log index the call committed at. */
  index: number;
  /** The app's events the call emitted, in order. */
  events: AppEvent[];
}

/** A view's whole answer (`raw`): its result and the log index of the state it read. */
export interface ViewAnswer<T> {
  result: T;
  index: number;
}

/**
 * What calls and views use of mero-js's `CalimeroClient` (`client.apps.call`,
 * `client.apps.view`): `Pick<CalimeroClient, 'apps'>` or a test double of the two. A double
 * that leaves out `index` or `events` answers `raw` with index 0 and no events.
 */
export interface AppCallTransport {
  apps: {
    call(
      args: { app: string; method: string; args?: unknown; idempotencyKey?: string },
      options?: { confirm?: ConfirmCallback; signal?: AbortSignal },
    ): Promise<{ result: unknown; index?: number; events?: { kind: string; data: unknown }[] }>;
    view(
      args: { app: string; method: string; args?: unknown; consistency?: 'strong' | 'stale'; minIndex?: number },
      options?: { signal?: AbortSignal },
    ): Promise<{ result: unknown; index?: number }>;
  };
}

/**
 * What the class uses of mero-js's `CalimeroClient`: calls and views, and the event stream
 * (`client.events`) its `events()` reads. A test double implements this alone.
 */
export interface AppTransport extends AppCallTransport {
  events(app: string, from?: number, signal?: AbortSignal): AsyncIterable<NodeEvent>;
}
";

/// The two private senders every method goes through, after the constructor.
const SENDERS: &str = r"  async #call<R>(method: string, args: object, options: CallOptions): Promise<CallAnswer<R>> {
    const { idempotencyKey, ...rest } = options;
    const request = { app: this.#app, method, args, ...(idempotencyKey === undefined ? {} : { idempotencyKey }) };
    try {
      const { result, index = 0, events = [] } = await this.#client.apps.call(request, rest);
      return { result: result as R, index, events: events as AppEvent[] };
    } finally {
      await this.#after?.();
    }
  }

  async #view<R>(method: string, args: object, options: ViewOptions): Promise<ViewAnswer<R>> {
    const { consistency, minIndex, signal } = options;
    const request = {
      app: this.#app,
      method,
      args,
      ...(consistency === undefined ? {} : { consistency }),
      ...(minIndex === undefined ? {} : { minIndex }),
    };
    try {
      const { result, index = 0 } = await this.#client.apps.view(request, signal === undefined ? {} : { signal });
      return { result: result as R, index };
    } finally {
      await this.#after?.();
    }
  }
";

/// The interface of the class's methods: `KvStoreClient` → `KvStoreApi`.
fn api_name(class: &str) -> String {
    format!("{}Api", class.strip_suffix("Client").unwrap_or(class))
}

/// The interface of the class's `raw` methods: `KvStoreClient` → `KvStoreRawApi`.
fn raw_api_name(class: &str) -> String {
    format!("{}RawApi", class.strip_suffix("Client").unwrap_or(class))
}

pub(super) fn module(abi: &AppAbi, class: &str) -> String {
    let api = api_name(class);
    let raw_api = raw_api_name(class);
    let init = abi
        .methods
        .values()
        .find(|m| m.kind == Kind::Init && !m.params.is_empty());
    let mut names = Names::new(abi);
    names.reserve(
        OWN_NAMES
            .iter()
            .copied()
            .chain([class, api.as_str(), raw_api.as_str()])
            .chain(init.map(|_| INIT_ARGS)),
    );
    let mut ts = Ts {
        abi,
        names,
        named: BTreeMap::new(),
        described: BTreeMap::new(),
        params: Vec::new(),
    };
    let mut body = String::new();

    if let Some(init) = init {
        ts.interface(
            &mut body,
            "The arguments of the app's `init`: `initArgs` of `client.apps.create`.",
            INIT_ARGS,
            &init.params,
            Side::Input,
        );
    }
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
    // The methods alone, so a caller's test double can stand in for the class.
    let mut iface = String::new();
    let mut raw_iface = String::new();
    let mut raw_fields = String::new();
    let mut class_body = String::new();
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
        let (send, options, answer) = if m.runs_as_view() {
            ("view", "ViewOptions", "ViewAnswer")
        } else {
            ("call", "CallOptions", "CallAnswer")
        };
        let _ = writeln!(
            iface,
            "  /** {doc}. */\n  {method}({args_param}options?: {options}): Promise<{ret}>;"
        );
        let _ = writeln!(
            raw_iface,
            "  /** {doc}. */\n  {method}({args_param}options?: {options}): Promise<{answer}<{ret}>>;"
        );
        let raw_args = if m.params.is_empty() { "" } else { "args, " };
        let _ = writeln!(
            raw_fields,
            "      {method}: ({raw_args}options = {{}}) => this.#{send}<{ret}>({lit}, {args_value}, options),"
        );
        let _ = writeln!(
            class_body,
            "\n  /** {doc}. */\n  async {method}({args_param}options: {options} = {{}}): Promise<{ret}> {{\n    return (await this.#{send}<{ret}>({lit}, {args_value}, options)).result;\n  }}"
        );
    }
    let stream = if abi.client_methods().any(|(n, _)| n == "events") {
        "appEvents"
    } else {
        "events"
    };
    let raw = if abi.client_methods().any(|(n, _)| n == "raw") {
        "rawAnswers"
    } else {
        "raw"
    };
    let _ = writeln!(
        iface,
        "  /** The app's events from log index `from`, then live, until `signal` aborts. */\n  {stream}(from?: number, signal?: AbortSignal): AsyncIterable<AppEvent & {{ id: string }}>;"
    );
    let _ = writeln!(
        body,
        "\n/** The app's methods: what `{class}` implements, for a test double to stand in for it. */\nexport interface {api} {{\n{iface}}}"
    );
    let _ = writeln!(
        body,
        "\n/**\n * The app's methods answering whole: `{class}`'s `{raw}`, for code that reads a call's log\n * index or events.\n */\nexport interface {raw_api} {{\n{raw_iface}}}"
    );
    let _ = writeln!(
        body,
        "\n/**\n * The app's methods, over a signed-in `CalimeroClient`, `Pick<CalimeroClient, 'apps'>` or any\n * {{@link AppCallTransport}}; `{stream}()` needs an {{@link AppTransport}}.\n */\nexport class {class}<T extends AppCallTransport = AppTransport> implements {api} {{\n  readonly #client: T;\n  readonly #app: string;\n  readonly #after: ClientOptions['after'];\n  /**\n   * The same methods answering whole: `{{ result, index, events }}` for a call, `{{ result, index }}`\n   * for a view.\n   */\n  readonly {raw}: {raw_api};\n\n  constructor(client: T, app: string, options: ClientOptions = {{}}) {{\n    this.#client = client;\n    this.#app = app;\n    this.#after = options.after;\n    this.{raw} = {{\n{raw_fields}    }};\n  }}\n"
    );
    body.push_str(SENDERS);
    body.push_str(&class_body);
    let _ = writeln!(
        body,
        "\n  /** The app's events from log index `from`, then live, until `signal` aborts. */\n  async *{stream}(this: {class}<AppTransport>, from = 0, signal?: AbortSignal): AsyncGenerator<AppEvent & {{ id: string }}> {{\n    for await (const event of this.#client.events(this.#app, from, signal)) {{\n      yield event as AppEvent & {{ id: string }};\n    }}\n  }}\n}}"
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
    // The described types, and the ones their shapes name, until none is new.
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let pending: Vec<(String, (String, &TypeDef))> = ts
            .described
            .iter()
            .filter(|(name, _)| !declared.contains_key(*name))
            .map(|(n, d)| (n.clone(), d.clone()))
            .collect();
        if pending.is_empty() {
            break;
        }
        for (name, (entry, def)) in pending {
            let decl = ts.declaration(&name, &entry, def);
            let _ = declared.insert(name, decl);
        }
    }
    for decl in declared.values() {
        let _ = writeln!(out, "{decl}");
    }
    for (name, text) in &ts.named {
        let _ = writeln!(
            out,
            "/** The app type `{text}`: the ABI names it without its shape. */\nexport type {name} = unknown;\n"
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

    #[test]
    fn a_method_named_raw_keeps_its_name_and_the_whole_answers_move() {
        let abi = AppAbi::from_json(&json!({
            "abiVersion": 1,
            "methods": { "raw": { "kind": "view", "params": [], "returns": "string" } },
        }))
        .unwrap();
        let ts = module(&abi, "AppClient");
        assert!(
            ts.contains("  async raw(options: ViewOptions = {}): Promise<string> {"),
            "{ts}"
        );
        assert!(ts.contains("  readonly rawAnswers: AppRawApi;"), "{ts}");
        assert!(ts.contains("this.rawAnswers = {"), "{ts}");
    }

    #[test]
    fn init_args_are_declared_only_when_init_takes_some() {
        let none = AppAbi::from_json(&json!({
            "abiVersion": 1,
            "methods": { "init": { "kind": "init", "params": [] } },
        }))
        .unwrap();
        assert!(!module(&none, "AppClient").contains("InitArgs"));
        let some = AppAbi::from_json(&json!({
            "abiVersion": 1,
            "methods": {
                "init": { "kind": "init", "params": [["limit", "u64"], ["note", "option<string>"]] },
                "get": { "kind": "view", "params": [], "returns": "InitArgs" },
            },
        }))
        .unwrap();
        let ts = module(&some, "AppClient");
        assert!(
            ts.contains(
                "export interface InitArgs {\n  limit: string;\n  note?: string | null;\n}"
            ),
            "{ts}"
        );
        assert!(ts.contains("export type InitArgsType = unknown;"), "{ts}");
    }
}
