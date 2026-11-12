//! The Swift client over swift-sdk's MeroKit: one `async throws` method per app method, sent
//! through `CalimeroClient` (`callJSON`, `callConfirmed`, `viewJSON`) or a
//! `ContextContractCalling`, the app's events and errors as enums, every value `Codable` in the
//! ABI's JSON form (u64/i64 decimal strings, bytes base64url).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::abi::{AppAbi, Fields};
use crate::openapi::{all_events, pascal, Names};
use crate::ty::{Int, Type};

use super::ident::{lower_first, sanitize, Scope};

/// Swift's keywords: an identifier spelled like one is written in backticks.
const KEYWORDS: &[&str] = &[
    "Any",
    "Self",
    "as",
    "associatedtype",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "continue",
    "default",
    "defer",
    "deinit",
    "do",
    "else",
    "enum",
    "extension",
    "fallthrough",
    "false",
    "fileprivate",
    "for",
    "func",
    "guard",
    "if",
    "import",
    "in",
    "init",
    "inout",
    "internal",
    "is",
    "let",
    "nil",
    "open",
    "operator",
    "precedencegroup",
    "private",
    "protocol",
    "public",
    "repeat",
    "rethrows",
    "return",
    "self",
    "static",
    "struct",
    "subscript",
    "super",
    "switch",
    "throw",
    "throws",
    "true",
    "try",
    "typealias",
    "var",
    "where",
    "while",
];

/// The type names the client uses or nests: an app type named like one is renamed, since its
/// alias would shadow it inside the client.
const RESERVED_TYPES: &[&str] = &[
    "Answer",
    "AppError",
    "Args",
    "Array",
    "AsyncThrowingStream",
    "Base64URL",
    "Bool",
    "CalimeroClient",
    "CharacterSet",
    "Codable",
    "CodingKey",
    "ContextContractCalling",
    "Data",
    "DecimalString",
    "Decodable",
    "Decoder",
    "DecodingError",
    "Dictionary",
    "Double",
    "Encodable",
    "Encoder",
    "Error",
    "Event",
    "EventFrame",
    "Fields",
    "FixedWidthInteger",
    "Float",
    "Foundation",
    "Int",
    "Int16",
    "Int32",
    "Int64",
    "Int8",
    "JSONDecoder",
    "JSONEncoder",
    "JSONValue",
    "Key",
    "MeroAppBackend",
    "MeroAppBackendUnsupported",
    "MeroKit",
    "Optional",
    "PasskeyProvider",
    "Refusal",
    "Sendable",
    "String",
    "Task",
    "Transport",
    "UInt16",
    "UInt32",
    "UInt64",
    "UInt8",
    "Void",
];

/// Members of the client besides the app's methods.
const MEMBERS: &[&str] = &[
    "invokeCall",
    "invokeView",
    "invokeConfirmed",
    "transport",
    "json",
    "result",
];

/// The nested helpers every client carries: the ABI's string forms, the argument object and
/// the answer's `result`.
const WIRE: &str = r#"
    /// A `u64` or `i64` as the wire carries it: a decimal string (a number is read too).
    public struct DecimalString<Integer: FixedWidthInteger & Codable & Sendable>: Codable, Sendable, Hashable {
        public var value: Integer

        public init(_ value: Integer) {
            self.value = value
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.singleValueContainer()
            if let text = try? container.decode(String.self) {
                guard let value = Integer(text) else {
                    throw DecodingError.dataCorruptedError(in: container, debugDescription: "not a decimal integer: \(text)")
                }
                self.value = value
            } else {
                value = try container.decode(Integer.self)
            }
        }

        public func encode(to encoder: Encoder) throws {
            var container = encoder.singleValueContainer()
            try container.encode(String(value))
        }
    }

    /// `bytes` as the wire carries them: base64url without padding (padding is read too).
    public struct Base64URL: Codable, Sendable, Hashable {
        public var data: Data

        public init(_ data: Data) {
            self.data = data
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.singleValueContainer()
            let text = try container.decode(String.self)
            var base64 = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
            base64 = base64.trimmingCharacters(in: CharacterSet(charactersIn: "="))
            base64 += String(repeating: "=", count: (4 - base64.count % 4) % 4)
            guard let data = Data(base64Encoded: base64) else {
                throw DecodingError.dataCorruptedError(in: container, debugDescription: "not base64url: \(text)")
            }
            self.data = data
        }

        public func encode(to encoder: Encoder) throws {
            var container = encoder.singleValueContainer()
            let text = data.base64EncodedString()
                .replacingOccurrences(of: "+", with: "-")
                .replacingOccurrences(of: "/", with: "_")
                .trimmingCharacters(in: CharacterSet(charactersIn: "="))
            try container.encode(text)
        }
    }

    /// A method's arguments by name; a `nil` option is sent as `null`.
    fileprivate struct Args: Encodable {
        var entries: [(String, any Encodable)] = []

        mutating func add(_ name: String, _ value: some Encodable) {
            entries.append((name, value))
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: Key.self)
            for (name, value) in entries {
                try container.encode(value, forKey: Key(name))
            }
        }
    }

    fileprivate struct Key: CodingKey {
        var stringValue: String
        var intValue: Int? { nil }

        init(_ name: String) {
            stringValue = name
        }

        init?(stringValue: String) {
            self.stringValue = stringValue
        }

        init?(intValue: Int) {
            return nil
        }
    }

    /// The `result` of a call's or view's answer; a missing one reads as `null`.
    fileprivate struct Answer<Wire: Decodable>: Decodable {
        let result: Wire

        private enum CodingKeys: String, CodingKey {
            case result
        }

        init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            if container.contains(.result) {
                result = try container.decode(Wire.self, forKey: .result)
            } else {
                result = try JSONDecoder().decode(Wire.self, from: Data("null".utf8))
            }
        }
    }

    /// An event's or error's fields; a missing one reads as `null`.
    fileprivate struct Fields {
        let object: [String: JSONValue]

        init(_ data: JSONValue?) {
            if case .object(let object)? = data {
                self.object = object
            } else {
                object = [:]
            }
        }

        func decode<Wire: Decodable>(_ name: String, _ type: Wire.Type) throws -> Wire {
            try JSONDecoder().decode(Wire.self, from: JSONEncoder().encode(object[name] ?? .null))
        }
    }

    private static func json(_ args: Args) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(args)
    }

    private static func result<Wire: Decodable>(_ type: Wire.Type, of answer: Data) throws -> Wire {
        try JSONDecoder().decode(Answer<Wire>.self, from: answer).result
    }
"#;

/// Whether the SDK's macros convert the type itself (D20): `u64`, `i64` and `bytes` have a
/// string form only here or directly inside an `Option`.
fn converted(ty: &Type) -> bool {
    matches!(ty, Type::Int(Int::U64 | Int::I64) | Type::Bytes)
}

struct Sw {
    names: Names,
    /// App types met so far: Swift name to ABI text.
    named: BTreeMap<String, String>,
    tuples: BTreeSet<usize>,
}

impl Sw {
    /// The Swift type of an ABI type. At the top of a parameter, result or field (`top`), a
    /// `u64`, `i64` or `bytes` (or an option of one) is `UInt64`, `Int64` or `Data`; deeper,
    /// where the SDK refuses them (Ruling 27), it keeps its wire type.
    fn ty(&mut self, ty: &Type, top: bool) -> String {
        match ty {
            // `null` on the wire.
            Type::Unit => "JSONValue".into(),
            Type::Bool => "Bool".into(),
            Type::Int(Int::U64) if top => "UInt64".into(),
            Type::Int(Int::I64) if top => "Int64".into(),
            Type::Int(Int::U64) => "DecimalString<UInt64>".into(),
            Type::Int(Int::I64) => "DecimalString<Int64>".into(),
            Type::Int(Int::U8) => "UInt8".into(),
            Type::Int(Int::U16) => "UInt16".into(),
            Type::Int(Int::U32 | Int::Usize) => "UInt32".into(),
            Type::Int(Int::U128) => "UInt64".into(),
            Type::Int(Int::I8) => "Int8".into(),
            Type::Int(Int::I16) => "Int16".into(),
            Type::Int(Int::I32 | Int::Isize) => "Int32".into(),
            Type::Int(Int::I128) => "Int64".into(),
            Type::Float(false) => "Float".into(),
            Type::Float(true) => "Double".into(),
            Type::Char | Type::String => "String".into(),
            Type::Bytes if top => "Data".into(),
            Type::Bytes => "Base64URL".into(),
            Type::Option(inner) => format!("{}?", self.ty(inner, top && converted(inner))),
            Type::List(inner) | Type::Set(inner) | Type::Array(inner, _) => {
                format!("[{}]", self.ty(inner, false))
            }
            Type::Tuple(items) => {
                let _ = self.tuples.insert(items.len());
                let items: Vec<String> = items.iter().map(|t| self.ty(t, false)).collect();
                format!("Tuple{}<{}>", items.len(), items.join(", "))
            }
            Type::Map(_, value) => format!("[String: {}]", self.ty(value, false)),
            // ABI version 1 names an app type without its fields: its alias is `JSONValue`.
            Type::Named(text) => {
                let (name, _) = self.names.named(text);
                let _ = self.named.insert(name.clone(), text.clone());
                name
            }
        }
    }

    /// The type decoded from or encoded to the wire.
    fn wire(&mut self, ty: &Type) -> String {
        self.ty(ty, false)
    }
}

/// `expr` (of `ty`'s Swift type) as its wire type.
fn to_wire(ty: &Type, expr: &str) -> String {
    match ty {
        Type::Int(Int::U64 | Int::I64) => format!("DecimalString({expr})"),
        Type::Bytes => format!("Base64URL({expr})"),
        Type::Option(inner) if converted(inner) => {
            format!("{expr}.map {{ {} }}", to_wire(inner, "$0"))
        }
        _ => expr.to_owned(),
    }
}

/// `expr` (of `ty`'s wire type) as its Swift type.
fn from_wire(ty: &Type, expr: &str) -> String {
    match ty {
        Type::Int(Int::U64 | Int::I64) => format!("{expr}.value"),
        Type::Bytes => format!("{expr}.data"),
        Type::Option(inner) if converted(inner) => {
            let field = if **inner == Type::Bytes {
                "data"
            } else {
                "value"
            };
            format!("{expr}?.{field}")
        }
        _ => expr.to_owned(),
    }
}

/// An ABI name as a Swift identifier, in backticks when it is a keyword.
fn ident(name: &str) -> String {
    let name = sanitize(name);
    if KEYWORDS.contains(&name.as_str()) {
        return format!("`{name}`");
    }
    name
}

fn string_literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One case of an event or error: its Swift name, ABI name and fields.
struct Variant {
    case: String,
    wire: String,
    fields: Vec<(String, String, Type)>,
}

fn variants(entries: &BTreeMap<String, Fields>, scope: &mut Scope) -> Vec<Variant> {
    entries
        .iter()
        .map(|(wire, fields)| {
            let mut field_scope = Scope::new(&[]);
            Variant {
                case: ident(&scope.take(lower_first(&sanitize(&pascal(wire))))),
                wire: wire.clone(),
                fields: fields
                    .iter()
                    .map(|(f, t)| (field_scope.take(ident(f)), f.clone(), t.clone()))
                    .collect(),
            }
        })
        .collect()
}

impl Sw {
    fn cases(&mut self, out: &mut String, what: &str, variants: &[Variant]) {
        for v in variants {
            let _ = writeln!(out, "        /// The {what} `{}`.", v.wire);
            if v.fields.is_empty() {
                let _ = writeln!(out, "        case {}", v.case);
                continue;
            }
            let fields: Vec<String> = v
                .fields
                .iter()
                .map(|(name, _, t)| format!("{name}: {}", self.ty(t, true)))
                .collect();
            let _ = writeln!(out, "        case {}({})", v.case, fields.join(", "));
        }
    }

    /// `try .case(a: fields.decode("a", W.self), ...)` or `.case`.
    fn construct(&mut self, v: &Variant) -> String {
        if v.fields.is_empty() {
            return format!(".{}", v.case);
        }
        let args: Vec<String> = v
            .fields
            .iter()
            .map(|(name, wire, t)| {
                let decoded = format!(
                    "fields.decode({}, {}.self)",
                    string_literal(wire),
                    self.wire(t)
                );
                format!("{name}: {}", from_wire(t, &decoded))
            })
            .collect();
        format!("try .{}({})", v.case, args.join(", "))
    }
}

pub(super) fn module(abi: &AppAbi, class: &str) -> String {
    let mut names = Names::new(abi);
    names.reserve(RESERVED_TYPES.iter().copied());
    names.reserve([class]);
    let tuple_names: Vec<String> = (1..=16).map(|n| format!("Tuple{n}")).collect();
    names.reserve(tuple_names.iter().map(String::as_str));
    let mut sw = Sw {
        names,
        named: BTreeMap::new(),
        tuples: BTreeSet::new(),
    };

    let mut members = Scope::new(MEMBERS);
    let methods: Vec<_> = abi
        .client_methods()
        .map(|(name, m)| (ident(&members.take(sanitize(name))), name, m))
        .collect();
    let stream = members.take(if members.contains("events") {
        "appEvents".into()
    } else {
        "events".into()
    });

    let mut body = String::new();
    for (method, name, m) in &methods {
        let view = m.runs_as_view();
        let reserved: &[&str] = if view {
            &["stale", "minIndex"]
        } else if m.confirm {
            &["passkey", "idempotencyKey"]
        } else {
            &["idempotencyKey"]
        };
        let mut params = Scope::new(reserved);
        let mut sig = Vec::new();
        let mut adds = Vec::new();
        for (p, t) in &m.params {
            let arg = ident(&params.take(sanitize(p)));
            let ty = sw.ty(t, true);
            let default = if t.is_optional() { " = nil" } else { "" };
            sig.push(format!("{arg}: {ty}{default}"));
            adds.push(format!(
                "args.add({}, {})",
                string_literal(p),
                to_wire(t, &arg)
            ));
        }
        let (call, extra) = if view {
            sig.push("stale: Bool = false".into());
            sig.push("minIndex: Int = 0".into());
            ("invokeView", "stale: stale, minIndex: minIndex")
        } else if m.confirm {
            sig.push("passkey: any PasskeyProvider".into());
            sig.push("idempotencyKey: String? = nil".into());
            (
                "invokeConfirmed",
                "passkey: passkey, idempotencyKey: idempotencyKey",
            )
        } else {
            sig.push("idempotencyKey: String? = nil".into());
            ("invokeCall", "idempotencyKey: idempotencyKey")
        };
        let unit = m.returns == Type::Unit;
        let mut doc = format!("{} `{name}`", m.kind.name());
        if m.confirm {
            doc.push_str("; `passkey` approves it (`confirm`)");
        }
        if m.public {
            doc.push_str("; public");
        }
        let _ = writeln!(body, "\n    /// {doc}.");
        if !unit && !view {
            let _ = writeln!(body, "    @discardableResult");
        }
        let _ = writeln!(body, "    public func {method}(");
        let last = sig.len() - 1;
        for (i, s) in sig.iter().enumerate() {
            let comma = if i == last { "" } else { "," };
            let _ = writeln!(body, "        {s}{comma}");
        }
        let ret = if unit {
            String::new()
        } else {
            format!(" -> {}", sw.ty(&m.returns, true))
        };
        let _ = writeln!(body, "    ) async throws{ret} {{");
        if adds.is_empty() {
            let _ = writeln!(body, "        let args = Args()");
        } else {
            let _ = writeln!(body, "        var args = Args()");
            for a in &adds {
                let _ = writeln!(body, "        {a}");
            }
        }
        let lit = string_literal(name);
        if unit {
            let _ = writeln!(
                body,
                "        _ = try await {call}({lit}, args, {extra}, as: JSONValue.self)\n    }}"
            );
        } else {
            let wire = sw.wire(&m.returns);
            let _ = writeln!(
                body,
                "        let result = try await {call}({lit}, args, {extra}, as: {wire}.self)\n        return {}\n    }}",
                from_wire(&m.returns, "result")
            );
        }
    }

    let events = all_events(abi);
    let mut event_scope = Scope::new(&[]);
    let event_variants = variants(&events, &mut event_scope);
    let unknown_event = ident(&event_scope.take("unknown".into()));
    let mut refusal_scope = Scope::new(&[]);
    let refusal_variants = variants(&abi.errors, &mut refusal_scope);
    let unknown_refusal = ident(&refusal_scope.take("unknown".into()));

    let mut nested = String::new();
    let _ = writeln!(
        nested,
        "\n    /// An event of the app, as `{stream}` and `MeroAppBackend.events` carry it.\n    public enum Event: Sendable {{"
    );
    sw.cases(&mut nested, "event", &event_variants);
    let _ = writeln!(
        nested,
        "        /// An event this client does not know: one a newer release of the app added.\n        case {unknown_event}(kind: String, data: JSONValue?)\n"
    );
    let _ = writeln!(
        nested,
        "        /// An event's `kind` and `data`, typed; a known kind whose data does not fit throws.\n        public init(kind: String, data: JSONValue?) throws {{"
    );
    if event_variants.iter().any(|v| !v.fields.is_empty()) {
        let _ = writeln!(nested, "            let fields = Fields(data)");
    }
    let _ = writeln!(nested, "            switch kind {{");
    for v in &event_variants {
        let made = sw.construct(v);
        let _ = writeln!(
            nested,
            "            case {}: self = {made}",
            string_literal(&v.wire)
        );
    }
    let _ = writeln!(
        nested,
        "            default: self = .{unknown_event}(kind: kind, data: data)\n            }}\n        }}\n    }}"
    );
    let _ = writeln!(
        nested,
        "\n    /// One frame of `{stream}`: the event's log position (`index:seq`) and the event.\n    public struct EventFrame: Sendable {{\n        public let id: String\n        public let event: Event\n    }}"
    );
    let _ = writeln!(
        nested,
        "\n    /// The app's refusal: a `422 APP_ERROR`'s code and fields (`AppError.appCode`, `fields`), typed.\n    public enum Refusal: Sendable {{"
    );
    sw.cases(&mut nested, "error", &refusal_variants);
    let _ = writeln!(
        nested,
        "        /// A code this client does not know, or fields that do not fit it.\n        case {unknown_refusal}(code: String, fields: [String: JSONValue])\n"
    );
    let _ = writeln!(
        nested,
        "        /// `error`'s code and fields, typed.\n        public init(_ error: MeroKit.AppError) {{\n            self = (try? Self.decode(error)) ?? .{unknown_refusal}(code: error.appCode, fields: error.fields)\n        }}\n\n        private static func decode(_ error: MeroKit.AppError) throws -> Self {{"
    );
    if refusal_variants.iter().any(|v| !v.fields.is_empty()) {
        let _ = writeln!(
            nested,
            "            let fields = Fields(.object(error.fields))"
        );
    }
    let _ = writeln!(nested, "            switch error.appCode {{");
    for v in &refusal_variants {
        let made = sw.construct(v);
        let _ = writeln!(
            nested,
            "            case {}: return {made}",
            string_literal(&v.wire)
        );
    }
    let _ = writeln!(
        nested,
        "            default: return .{unknown_refusal}(code: error.appCode, fields: error.fields)\n            }}\n        }}\n    }}"
    );
    nested.push_str(WIRE);
    for n in sw.tuples.clone() {
        tuple(&mut nested, n);
    }

    let mut out = String::new();
    let _ = writeln!(out, "// {}", super::provenance(abi));
    let _ = writeln!(
        out,
        "// A thin typed layer over swift-sdk's MeroKit: each method calls `CalimeroClient` (`callJSON`,\n// `callConfirmed`, `viewJSON`) or a `ContextContractCalling`, so it travels the sealed channel with\n// the SDK's retries. u64 and i64 are decimal strings and bytes base64url on the wire; here they are\n// `UInt64`, `Int64` and `Data`.\n"
    );
    let _ = writeln!(out, "import Foundation\nimport MeroKit\n");
    let _ = writeln!(
        out,
        "/// The app's methods.
///
/// Over a signed-in `CalimeroClient` every method runs, with idempotency keys, stale views, passkey
/// step-ups and `{stream}`. Over a `ContextContractCalling` (`MeroAppBackend.contract.bound(to:)`, or
/// a double in a test) a view is `query` and a call `execute`: the contract reads strongly and makes
/// its own idempotency key, so `confirm` methods, `{stream}` and an `idempotencyKey` throw
/// `MeroAppBackendUnsupported`. A refusal throws `AppError`; `Refusal(_:)` types it.
public struct {class}: Sendable {{"
    );
    for (name, text) in &sw.named {
        let _ = writeln!(
            out,
            "    /// The app type `{text}`: this client does not read its shape (the ABI's `types`), so it is its JSON.\n    public typealias {name} = JSONValue\n"
        );
    }
    let _ = writeln!(
        out,
        "    private enum Transport: Sendable {{
        case client(CalimeroClient, String)
        case contract(any ContextContractCalling)
    }}

    private let transport: Transport

    /// Every method, as the account `client` signs in, in the app instance `app`.
    public init(client: CalimeroClient, app: String) {{
        transport = .client(client, app)
    }}

    /// Calls and views through `contract`.
    public init(contract: any ContextContractCalling) {{
        transport = .contract(contract)
    }}

    /// Calls and views through `backend`'s contract in the app instance `app`.
    public init(backend: any MeroAppBackend, app: String) {{
        transport = .contract(backend.contract.bound(to: app))
    }}"
    );
    out.push_str(&body);
    let _ = writeln!(
        out,
        "
    /// The app's events from log index `from`, then live, until the stream is cancelled.
    public func {stream}(from: Int = 0) -> AsyncThrowingStream<EventFrame, Error> {{
        guard case .client(let client, let app) = transport else {{
            return AsyncThrowingStream {{ $0.finish(throwing: MeroAppBackendUnsupported(method: \"{stream}\")) }}
        }}
        let upstream = client.events(app, from: from)
        return AsyncThrowingStream {{ continuation in
            let task = Task {{
                do {{
                    for try await event in upstream {{
                        continuation.yield(EventFrame(id: event.id, event: try Event(kind: event.kind, data: event.data)))
                    }}
                    continuation.finish()
                }} catch {{
                    continuation.finish(throwing: error)
                }}
            }}
            continuation.onTermination = {{ _ in task.cancel() }}
        }}
    }}

    private func invokeCall<Wire: Decodable>(
        _ method: String, _ args: Args, idempotencyKey: String?, as type: Wire.Type
    ) async throws -> Wire {{
        let argsJSON = try Self.json(args)
        switch transport {{
        case .client(let client, let app):
            let answer = try await client.callJSON(app, method, argsJSON: argsJSON, idempotencyKey: idempotencyKey)
            return try Self.result(Wire.self, of: answer)
        case .contract(let contract):
            guard idempotencyKey == nil else {{ throw MeroAppBackendUnsupported(method: method) }}
            return try JSONDecoder().decode(Wire.self, from: try await contract.execute(method: method, argsJSON: argsJSON))
        }}
    }}

    private func invokeView<Wire: Decodable>(
        _ method: String, _ args: Args, stale: Bool, minIndex: Int, as type: Wire.Type
    ) async throws -> Wire {{
        let argsJSON = try Self.json(args)
        switch transport {{
        case .client(let client, let app):
            let answer = try await client.viewJSON(app, method, argsJSON: argsJSON, stale: stale, minIndex: minIndex)
            return try Self.result(Wire.self, of: answer)
        case .contract(let contract):
            return try JSONDecoder().decode(Wire.self, from: try await contract.query(method: method, argsJSON: argsJSON))
        }}
    }}

    private func invokeConfirmed<Wire: Decodable>(
        _ method: String, _ args: Args, passkey: any PasskeyProvider, idempotencyKey: String?, as type: Wire.Type
    ) async throws -> Wire {{
        guard case .client(let client, let app) = transport else {{
            throw MeroAppBackendUnsupported(method: method)
        }}
        let value = try JSONDecoder().decode(JSONValue.self, from: Self.json(args))
        let answer = try await client.callConfirmed(app, method, value, passkey: passkey, idempotencyKey: idempotencyKey)
        return try JSONDecoder().decode(Wire.self, from: JSONEncoder().encode(answer.result ?? .null))
    }}"
    );
    out.push_str(&nested);
    out.push_str("}\n");
    out
}

/// `TupleN`: a JSON array of `n` items.
fn tuple(out: &mut String, n: usize) {
    let params: Vec<String> = (0..n).map(|i| format!("T{i}")).collect();
    let generics: Vec<String> = params
        .iter()
        .map(|p| format!("{p}: Codable & Sendable"))
        .collect();
    let _ = writeln!(
        out,
        "\n    /// `tuple<..>` of {n}: a JSON array `[v0, ..]`.\n    public struct Tuple{n}<{}>: Codable, Sendable {{",
        generics.join(", ")
    );
    for (i, p) in params.iter().enumerate() {
        let _ = writeln!(out, "        public var v{i}: {p}");
    }
    let init: Vec<String> = params
        .iter()
        .enumerate()
        .map(|(i, p)| format!("_ v{i}: {p}"))
        .collect();
    let _ = writeln!(out, "\n        public init({}) {{", init.join(", "));
    for i in 0..n {
        let _ = writeln!(out, "            self.v{i} = v{i}");
    }
    let _ = writeln!(
        out,
        "        }}\n\n        public init(from decoder: Decoder) throws {{\n            var container = try decoder.unkeyedContainer()"
    );
    for (i, p) in params.iter().enumerate() {
        let _ = writeln!(out, "            v{i} = try container.decode({p}.self)");
    }
    let _ = writeln!(
        out,
        "        }}\n\n        public func encode(to encoder: Encoder) throws {{\n            var container = encoder.unkeyedContainer()"
    );
    for i in 0..n {
        let _ = writeln!(out, "            try container.encode(v{i})");
    }
    let _ = writeln!(out, "        }}\n    }}");
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sw() -> Sw {
        Sw {
            names: Names::new(
                &AppAbi::from_json(&json!({"abiVersion": 1, "methods": {}})).unwrap(),
            ),
            named: BTreeMap::new(),
            tuples: BTreeSet::new(),
        }
    }

    #[test]
    fn types_at_the_top_and_on_the_wire() {
        let mut sw = sw();
        // ABI type, Swift type, wire type.
        let cases = [
            ("u64", "UInt64", "DecimalString<UInt64>"),
            ("i64", "Int64", "DecimalString<Int64>"),
            ("bytes", "Data", "Base64URL"),
            ("option<u64>", "UInt64?", "DecimalString<UInt64>?"),
            ("option<bytes>", "Data?", "Base64URL?"),
            ("u32", "UInt32", "UInt32"),
            ("[u8;32]", "[UInt8]", "[UInt8]"),
            (
                "list<u64>",
                "[DecimalString<UInt64>]",
                "[DecimalString<UInt64>]",
            ),
            (
                "option<option<u64>>",
                "DecimalString<UInt64>??",
                "DecimalString<UInt64>??",
            ),
            (
                "BTreeMap<String,u32>",
                "[String: UInt32]",
                "[String: UInt32]",
            ),
            (
                "tuple<u32, string>",
                "Tuple2<UInt32, String>",
                "Tuple2<UInt32, String>",
            ),
            ("Item", "Item", "Item"),
            ("Page<Item>", "Page_Item", "Page_Item"),
            ("unit", "JSONValue", "JSONValue"),
        ];
        for (abi, native, wire) in cases {
            let t = Type::parse(abi);
            assert_eq!(sw.ty(&t, true), native, "{abi}");
            assert_eq!(sw.wire(&t), wire, "{abi}");
        }
        assert_eq!(sw.tuples, BTreeSet::from([2]));
    }

    #[test]
    fn conversions_between_swift_and_wire() {
        let u64 = Type::parse("u64");
        let opt = Type::parse("option<bytes>");
        assert_eq!(to_wire(&u64, "big"), "DecimalString(big)");
        assert_eq!(from_wire(&u64, "result"), "result.value");
        assert_eq!(to_wire(&opt, "blob"), "blob.map { Base64URL($0) }");
        assert_eq!(from_wire(&opt, "result"), "result?.data");
        let list = Type::parse("list<string>");
        assert_eq!(to_wire(&list, "names"), "names");
    }

    #[test]
    fn identifiers() {
        assert_eq!(ident("default"), "`default`");
        assert_eq!(ident("0"), "_0");
        assert_eq!(string_literal("a\"b\\"), "\"a\\\"b\\\\\"");
    }

    /// Errors and events become cases with typed fields, named clear of keywords and of the
    /// fallback case; a method named `events` moves the stream to `appEvents`.
    #[test]
    fn errors_events_and_names() {
        let abi = AppAbi::from_json(&serde_json::json!({
            "abiVersion": 1,
            "methods": { "events": { "kind": "call", "params": [["in", "string"]] } },
            "errors": { "NotFound": { "id": "u64" }, "Rejected": { "0": "string" }, "Unknown": {} },
            "events": { "Default": { "blob": "option<bytes>" } },
        }))
        .unwrap();
        let out = module(&abi, "AppClient");
        for expected in [
            r#"        case notFound(id: UInt64)"#,
            r#"        case rejected(_0: String)"#,
            r#"        case `default`(blob: Data?)"#,
            r#"        case unknown\n"#,
            r#"        case unknown_(code: String, fields: [String: JSONValue])"#,
            r#"case "Default": self = try .`default`(blob: fields.decode("blob", Base64URL?.self)?.data)"#,
            r#"public func appEvents(from: Int = 0)"#,
            r#"    public func events(\n        `in`: String,"#,
        ] {
            let expected = expected.replace("\\n", "\n");
            assert!(out.contains(&expected), "{expected} not in\n{out}");
        }
    }
}
