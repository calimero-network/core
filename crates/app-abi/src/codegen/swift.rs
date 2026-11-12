//! The Swift client over swift-sdk's MeroKit: one `async throws` method per app method, sent
//! through `CalimeroClient` (`callJSON`, `callConfirmed`, `viewJSON`) or a
//! `ContextContractCalling`, the app's events and errors as enums, every value `Codable` in the
//! ABI's JSON form (u64/i64 decimal strings, bytes base64url).
//!
//! An app type with a shape in the ABI's `types` is a nested `Codable, Sendable, Equatable` type:
//! a record a `struct`, an enum an `enum` whose `init(from:)` / `encode(to:)` follow its serde
//! tagging (external, internal, adjacent, untagged), a generic type a generic one. A type the ABI
//! does not describe stays its JSON (`JSONValue`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::abi::{AppAbi, EnumRepr, Field, Fields, Shape, TypeDef, VariantData};
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
    "EncodingError",
    "Error",
    "Event",
    "EventFrame",
    "Fields",
    "FixedWidthInteger",
    "Float",
    "Foundation",
    "Indirect",
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

struct Sw<'a> {
    abi: &'a AppAbi,
    names: Names,
    /// App types the ABI does not describe, met so far: Swift name to ABI text.
    named: BTreeMap<String, String>,
    /// App types it describes, met so far: Swift name to entry name and entry.
    described: BTreeMap<String, (String, &'a TypeDef)>,
    /// The generic parameters in scope (inside a generic entry's declaration).
    params: Vec<String>,
    tuples: BTreeSet<usize>,
    /// Whether a declaration boxes a property that holds its own type (`Indirect`).
    indirect: bool,
}

impl Sw<'_> {
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
            Type::Named(text) if self.params.contains(text) => text.clone(),
            Type::Named(text) => {
                if let Some((entry, def, args)) = self.abi.shape_of(ty) {
                    let (name, _) = self.names.named(&entry);
                    let _ = self.described.insert(name.clone(), (entry, def));
                    if args.is_empty() {
                        return name;
                    }
                    // A generic argument is held deeper than a field: its wire type.
                    let args: Vec<String> = args.iter().map(|a| self.ty(a, false)).collect();
                    return format!("{name}<{}>", args.join(", "));
                }
                // A type the ABI names without its shape: its alias is `JSONValue`.
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

impl Sw<'_> {
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

/// Where a shape's data is read from or written to: the decoder's (encoder's) own value, the
/// value at `key` of keyed `container`, or (internal tagging) `container` itself, next to the tag.
enum At<'s> {
    Value,
    Key { container: &'s str, key: &'s str },
    Same(&'s str),
}

/// `Key("name")`.
fn key(name: &str) -> String {
    format!("Key({})", string_literal(name))
}

/// Whether `ty` holds app type `entry` by value (not behind an array or a dictionary): a stored
/// property of it in `entry` itself is boxed (`Indirect`), and an enum carrying it is `indirect`.
fn holds(ty: &Type, entry: &str) -> bool {
    match ty {
        Type::Option(t) => holds(t, entry),
        Type::Tuple(ts) => ts.iter().any(|t| holds(t, entry)),
        Type::Named(_) => ty
            .named_parts()
            .is_some_and(|(name, args)| name == entry || args.iter().any(|a| holds(a, entry))),
        _ => false,
    }
}

fn data_holds(data: &VariantData, entry: &str) -> bool {
    match data {
        VariantData::Unit => false,
        VariantData::Newtype(t) => holds(t, entry),
        VariantData::Tuple(ts) => ts.iter().any(|t| holds(t, entry)),
        VariantData::Record(fs) => fs.iter().any(|f| holds(&f.ty, entry)),
    }
}

/// How many values a variant carries.
fn arity(data: &VariantData) -> usize {
    match data {
        VariantData::Unit => 0,
        VariantData::Newtype(_) => 1,
        VariantData::Tuple(ts) => ts.len(),
        VariantData::Record(fs) => fs.len(),
    }
}

/// `(let v0, let v1)`, or nothing for a variant without values.
fn pattern(n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let binds: Vec<String> = (0..n).map(|i| format!("let v{i}")).collect();
    format!("({})", binds.join(", "))
}

/// `throw DecodingError.dataCorrupted(..)` at `decoder`'s path.
fn corrupted(message: &str) -> String {
    format!(
        "throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: \"{message}\"))"
    )
}

/// An enum's variant: its case name, JSON name, labels (a struct variant's) and data.
struct Case<'v> {
    name: String,
    wire: &'v str,
    labels: Vec<String>,
    data: &'v VariantData,
}

impl Case<'_> {
    /// `(at: v0)`, `(v0, v1)` or nothing.
    fn args(&self) -> String {
        let n = arity(self.data);
        if n == 0 {
            return String::new();
        }
        let args: Vec<String> = (0..n)
            .map(|i| match self.labels.get(i) {
                Some(label) => format!("{label}: v{i}"),
                None => format!("v{i}"),
            })
            .collect();
        format!("({})", args.join(", "))
    }
}

impl Sw<'_> {
    /// A record field's Swift type: an `optional` one that is not an `option` is `T?`.
    fn field_type(&mut self, f: &Field) -> String {
        let ty = self.ty(&f.ty, true);
        if f.optional && !f.ty.is_optional() {
            format!("{ty}?")
        } else {
            ty
        }
    }

    /// Field `f` read from keyed `container`; an `option`, or a field the JSON may leave out,
    /// reads a missing key as `nil`.
    fn field_decode(&mut self, f: &Field, container: &str) -> String {
        let key = key(&f.name);
        if let Type::Option(inner) = &f.ty {
            let wire = self.wire(inner);
            return from_wire(
                &f.ty,
                &format!("try {container}.decodeIfPresent({wire}.self, forKey: {key})"),
            );
        }
        let wire = self.wire(&f.ty);
        if f.optional {
            return from_wire(
                &Type::Option(Box::new(f.ty.clone())),
                &format!("try {container}.decodeIfPresent({wire}.self, forKey: {key})"),
            );
        }
        from_wire(
            &f.ty,
            &format!("try {container}.decode({wire}.self, forKey: {key})"),
        )
    }

    /// The statements that read a shape's data from `at` into `v0`, `v1`, ...; `None` when serde
    /// cannot read it there (the statements then throw).
    fn decode_data(&mut self, data: &VariantData, at: &At<'_>, owner: &str) -> (Vec<String>, bool) {
        let mut lines = Vec::new();
        match data {
            VariantData::Unit => {}
            VariantData::Record(fields) => {
                let container = match at {
                    At::Same(container) => (*container).to_owned(),
                    At::Value | At::Key { .. } => {
                        let open = match at {
                            At::Key { container, key } => {
                                format!("try {container}.nestedContainer(keyedBy: Key.self, forKey: {key})")
                            }
                            _ => "try decoder.container(keyedBy: Key.self)".to_owned(),
                        };
                        if fields.is_empty() {
                            lines.push(format!("_ = {open}"));
                        } else {
                            lines.push(format!("let fields = {open}"));
                        }
                        "fields".to_owned()
                    }
                };
                for (i, f) in fields.iter().enumerate() {
                    let value = self.field_decode(f, &container);
                    lines.push(format!("let v{i} = {value}"));
                }
            }
            VariantData::Tuple(items) => {
                let open = match at {
                    At::Same(_) => {
                        lines.push(corrupted(&format!(
                            "{owner}: serde cannot tag a tuple variant internally"
                        )));
                        return (lines, false);
                    }
                    At::Key { container, key } => {
                        format!("try {container}.nestedUnkeyedContainer(forKey: {key})")
                    }
                    At::Value => "try decoder.unkeyedContainer()".to_owned(),
                };
                let binding = if items.is_empty() { "let" } else { "var" };
                lines.push(format!("{binding} items = {open}"));
                for (i, t) in items.iter().enumerate() {
                    let wire = self.wire(t);
                    let value = from_wire(t, &format!("try items.decode({wire}.self)"));
                    lines.push(format!("let v{i} = {value}"));
                }
                lines.push(format!(
                    "guard items.isAtEnd else {{ throw DecodingError.dataCorruptedError(in: items, debugDescription: \"{owner}: expected {} items\") }}",
                    items.len()
                ));
            }
            VariantData::Newtype(t) => {
                let wire = self.wire(t);
                let read = match at {
                    At::Key { container, key } => {
                        format!("try {container}.decode({wire}.self, forKey: {key})")
                    }
                    At::Value | At::Same(_) => {
                        format!("try decoder.singleValueContainer().decode({wire}.self)")
                    }
                };
                lines.push(format!("let v0 = {}", from_wire(t, &read)));
            }
        }
        (lines, true)
    }

    /// The statements that write a shape's data, `v0`, `v1`, ... (or `values`), to `at`.
    fn encode_data(
        &mut self,
        data: &VariantData,
        at: &At<'_>,
        values: &[String],
        owner: &str,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        match data {
            VariantData::Unit => match at {
                At::Value => {
                    lines.push("var single = encoder.singleValueContainer()".to_owned());
                    lines.push("try single.encodeNil()".to_owned());
                }
                At::Key { container, key } => {
                    lines.push(format!("try {container}.encodeNil(forKey: {key})"));
                }
                At::Same(_) => {}
            },
            VariantData::Record(fields) => {
                let container = match at {
                    At::Same(container) => (*container).to_owned(),
                    At::Value | At::Key { .. } => {
                        let open = match at {
                            At::Key { container, key } => {
                                format!(
                                    "{container}.nestedContainer(keyedBy: Key.self, forKey: {key})"
                                )
                            }
                            _ => "encoder.container(keyedBy: Key.self)".to_owned(),
                        };
                        if fields.is_empty() {
                            lines.push(format!("_ = {open}"));
                        } else {
                            lines.push(format!("var fields = {open}"));
                        }
                        "fields".to_owned()
                    }
                };
                for (f, value) in fields.iter().zip(values) {
                    lines.push(field_encode(f, &container, value));
                }
            }
            VariantData::Tuple(items) => {
                let open = match at {
                    At::Same(_) => {
                        lines.push(format!(
                            "throw EncodingError.invalidValue(self, .init(codingPath: encoder.codingPath, debugDescription: \"{owner}: serde cannot tag a tuple variant internally\"))"
                        ));
                        return lines;
                    }
                    At::Key { container, key } => {
                        format!("{container}.nestedUnkeyedContainer(forKey: {key})")
                    }
                    At::Value => "encoder.unkeyedContainer()".to_owned(),
                };
                if items.is_empty() {
                    lines.push(format!("_ = {open}"));
                } else {
                    lines.push(format!("var items = {open}"));
                }
                for (t, value) in items.iter().zip(values) {
                    lines.push(format!("try items.encode({})", to_wire(t, value)));
                }
            }
            VariantData::Newtype(t) => {
                let value = to_wire(t, &values[0]);
                match at {
                    At::Key { container, key } => {
                        lines.push(format!("try {container}.encode({value}, forKey: {key})"));
                    }
                    At::Value | At::Same(_) => {
                        lines.push("var single = encoder.singleValueContainer()".to_owned());
                        lines.push(format!("try single.encode({value})"));
                    }
                }
            }
        }
        lines
    }

    /// The declaration of a described app type, nested in the client.
    fn declaration(&mut self, name: &str, entry: &str, def: &TypeDef) -> String {
        self.params.clone_from(&def.params);
        let generics = if def.params.is_empty() {
            String::new()
        } else {
            let params: Vec<String> = def
                .params
                .iter()
                .map(|p| format!("{p}: Codable & Sendable & Equatable"))
                .collect();
            format!("<{}>", params.join(", "))
        };
        let mut out = format!("\n    /// The app type `{entry}`.\n");
        match &def.shape {
            Shape::Enum { repr, variants } => {
                self.enumeration(&mut out, name, entry, &generics, repr, variants);
            }
            Shape::Record(fields) => {
                self.structure(&mut out, name, entry, &generics, Stored::Record(fields))
            }
            Shape::Newtype(t) => {
                self.structure(&mut out, name, entry, &generics, Stored::Newtype(t))
            }
            Shape::Tuple(items) => {
                self.structure(&mut out, name, entry, &generics, Stored::Tuple(items))
            }
            Shape::Unit => {
                let _ = writeln!(
                    out,
                    "    public struct {name}{generics}: Codable, Sendable, Equatable {{
        public init() {{}}

        public init(from decoder: Decoder) throws {{
            guard try decoder.singleValueContainer().decodeNil() else {{
                {}
            }}
        }}

        public func encode(to encoder: Encoder) throws {{
            var single = encoder.singleValueContainer()
            try single.encodeNil()
        }}
    }}",
                    corrupted(&format!("{name}: expected null"))
                );
            }
        }
        self.params.clear();
        out
    }

    /// A struct: a record's fields, a newtype's `value` or a tuple's `v0`, `v1`, ...
    fn structure(
        &mut self,
        out: &mut String,
        name: &str,
        entry: &str,
        generics: &str,
        stored: Stored<'_>,
    ) {
        let data = match stored {
            Stored::Record(fields) => VariantData::Record(fields.to_vec()),
            Stored::Newtype(t) => VariantData::Newtype(t.clone()),
            Stored::Tuple(items) => VariantData::Tuple(items.to_vec()),
        };
        // (property, Swift type, whether the JSON may leave it out or it is an option, boxed).
        let mut props: Vec<(String, String, bool, bool)> = Vec::new();
        match &data {
            VariantData::Record(fields) => {
                let mut scope = Scope::new(&[]);
                for f in fields {
                    let ty = self.field_type(f);
                    props.push((
                        ident(&scope.take(sanitize(&f.name))),
                        ty,
                        f.optional || f.ty.is_optional(),
                        holds(&f.ty, entry),
                    ));
                }
            }
            VariantData::Newtype(t) => {
                let ty = self.ty(t, true);
                props.push(("value".to_owned(), ty, false, holds(t, entry)));
            }
            VariantData::Tuple(items) => {
                for (i, t) in items.iter().enumerate() {
                    let ty = self.ty(t, true);
                    props.push((format!("v{i}"), ty, false, holds(t, entry)));
                }
            }
            VariantData::Unit => {}
        }
        let _ = writeln!(
            out,
            "    public struct {name}{generics}: Codable, Sendable, Equatable {{"
        );
        for (prop, ty, _, boxed) in &props {
            let wrapper = if *boxed {
                self.indirect = true;
                "@Indirect "
            } else {
                ""
            };
            let _ = writeln!(out, "        {wrapper}public var {prop}: {ty}");
        }
        // The memberwise initializer: labelled for a record, unlabelled for a newtype or tuple.
        let labelled = matches!(data, VariantData::Record(_));
        let params: Vec<String> = props
            .iter()
            .map(|(prop, ty, optional, _)| {
                let label = if labelled { "" } else { "_ " };
                let default = if *optional { " = nil" } else { "" };
                format!("{label}{prop}: {ty}{default}")
            })
            .collect();
        if params.is_empty() {
            let _ = writeln!(out, "\n        public init() {{}}");
        } else {
            let _ = writeln!(out, "\n        public init(");
            let last = params.len() - 1;
            for (i, p) in params.iter().enumerate() {
                let comma = if i == last { "" } else { "," };
                let _ = writeln!(out, "            {p}{comma}");
            }
            let _ = writeln!(out, "        ) {{");
            for (prop, ..) in &props {
                let _ = writeln!(out, "            self.{prop} = {prop}");
            }
            let _ = writeln!(out, "        }}");
        }
        let (lines, _) = self.decode_data(&data, &At::Value, name);
        let _ = writeln!(
            out,
            "\n        public init(from decoder: Decoder) throws {{"
        );
        for line in &lines {
            let _ = writeln!(out, "            {line}");
        }
        for (i, (prop, ..)) in props.iter().enumerate() {
            let _ = writeln!(out, "            self.{prop} = v{i}");
        }
        let _ = writeln!(out, "        }}");
        let values: Vec<String> = props
            .iter()
            .map(|(prop, ..)| format!("self.{prop}"))
            .collect();
        let lines = self.encode_data(&data, &At::Value, &values, name);
        let _ = writeln!(
            out,
            "\n        public func encode(to encoder: Encoder) throws {{"
        );
        for line in &lines {
            let _ = writeln!(out, "            {line}");
        }
        let _ = writeln!(out, "        }}\n    }}");
    }

    /// An enum, coded as serde codes `repr`.
    fn enumeration(
        &mut self,
        out: &mut String,
        name: &str,
        entry: &str,
        generics: &str,
        repr: &EnumRepr,
        variants: &[crate::abi::Variant],
    ) {
        let indirect = if variants.iter().any(|v| data_holds(&v.data, entry)) {
            "indirect "
        } else {
            ""
        };
        let mut scope = Scope::new(&[]);
        let cases: Vec<Case<'_>> = variants
            .iter()
            .map(|v| {
                let labels = match &v.data {
                    VariantData::Record(fields) => {
                        let mut labels = Scope::new(&[]);
                        fields
                            .iter()
                            .map(|f| ident(&labels.take(sanitize(&f.name))))
                            .collect()
                    }
                    _ => Vec::new(),
                };
                Case {
                    name: ident(&scope.take(lower_first(&sanitize(&pascal(&v.name))))),
                    wire: &v.name,
                    labels,
                    data: &v.data,
                }
            })
            .collect();
        let _ = writeln!(
            out,
            "    public {indirect}enum {name}{generics}: Codable, Sendable, Equatable {{"
        );
        for c in &cases {
            let values: Vec<String> = match c.data {
                VariantData::Unit => Vec::new(),
                VariantData::Newtype(t) => vec![self.ty(t, true)],
                VariantData::Tuple(ts) => ts.iter().map(|t| self.ty(t, true)).collect(),
                VariantData::Record(fields) => fields
                    .iter()
                    .zip(&c.labels)
                    .map(|(f, label)| format!("{label}: {}", self.field_type(f)))
                    .collect(),
            };
            let _ = writeln!(out, "        /// `{}`.", c.wire);
            if values.is_empty() {
                let _ = writeln!(out, "        case {}", c.name);
            } else {
                let _ = writeln!(out, "        case {}({})", c.name, values.join(", "));
            }
        }

        // init(from:)
        let _ = writeln!(
            out,
            "\n        public init(from decoder: Decoder) throws {{"
        );
        let mut body: Vec<String> = Vec::new();
        let unknown = |what: &str| format!("{name}: unknown variant \\({what})");
        match repr {
            _ if cases.is_empty() => body.push(corrupted(&format!("{name} has no variants"))),
            EnumRepr::External => {
                let units: Vec<&Case<'_>> = cases
                    .iter()
                    .filter(|c| matches!(c.data, VariantData::Unit))
                    .collect();
                if !units.is_empty() {
                    body.push(
                        "if let name = try? decoder.singleValueContainer().decode(String.self) {"
                            .to_owned(),
                    );
                    body.push("    switch name {".to_owned());
                    for c in &units {
                        body.push(format!("    case {}:", string_literal(c.wire)));
                        body.push(format!("        self = .{}", c.name));
                    }
                    body.push("    default:".to_owned());
                    body.push(format!("        {}", corrupted(&unknown("name"))));
                    body.push("    }".to_owned());
                    body.push("    return".to_owned());
                    body.push("}".to_owned());
                }
                body.push("let container = try decoder.container(keyedBy: Key.self)".to_owned());
                body.push(
                    "guard container.allKeys.count == 1, let key = container.allKeys.first else {"
                        .to_owned(),
                );
                body.push(format!(
                    "    {}",
                    corrupted(&format!("{name}: expected one variant"))
                ));
                body.push("}".to_owned());
                body.push("switch key.stringValue {".to_owned());
                for c in &cases {
                    let at = At::Key {
                        container: "container",
                        key: "key",
                    };
                    self.decode_case(&mut body, c, &at, name);
                }
                body.push("default:".to_owned());
                body.push(format!(
                    "    throw DecodingError.dataCorruptedError(forKey: key, in: container, debugDescription: \"{}\")",
                    unknown("key.stringValue")
                ));
                body.push("}".to_owned());
            }
            EnumRepr::Internal { tag } | EnumRepr::Adjacent { tag, .. } => {
                let tag_key = key(tag);
                body.push("let container = try decoder.container(keyedBy: Key.self)".to_owned());
                body.push(format!(
                    "let tag = try container.decode(String.self, forKey: {tag_key})"
                ));
                body.push("switch tag {".to_owned());
                let content = match repr {
                    EnumRepr::Adjacent { content, .. } => Some(key(content)),
                    _ => None,
                };
                for c in &cases {
                    let at = match (&content, c.data) {
                        (Some(content), _) => At::Key {
                            container: "container",
                            key: content,
                        },
                        (None, VariantData::Newtype(_)) => At::Value,
                        (None, _) => At::Same("container"),
                    };
                    self.decode_case(&mut body, c, &at, name);
                }
                body.push("default:".to_owned());
                body.push(format!(
                    "    throw DecodingError.dataCorruptedError(forKey: {tag_key}, in: container, debugDescription: \"{}\")",
                    unknown("tag")
                ));
                body.push("}".to_owned());
            }
            EnumRepr::Untagged => {
                // serde tries the variants in order; the first that reads is the value.
                for c in &cases {
                    if let VariantData::Unit = c.data {
                        body.push(
                            "if (try? decoder.singleValueContainer().decodeNil()) == true {"
                                .to_owned(),
                        );
                        body.push(format!("    self = .{}", c.name));
                        body.push("    return".to_owned());
                        body.push("}".to_owned());
                        continue;
                    }
                    let (lines, _) = self.decode_data(c.data, &At::Value, name);
                    body.push("do {".to_owned());
                    for line in lines {
                        body.push(format!("    {line}"));
                    }
                    body.push(format!("    self = .{}{}", c.name, c.args()));
                    body.push("    return".to_owned());
                    body.push("} catch {}".to_owned());
                }
                body.push(corrupted(&format!("{name}: no variant fits")));
            }
        }
        for line in &body {
            let _ = writeln!(out, "            {line}");
        }
        let _ = writeln!(out, "        }}");

        // encode(to:)
        let _ = writeln!(
            out,
            "\n        public func encode(to encoder: Encoder) throws {{"
        );
        let mut body: Vec<String> = vec!["switch self {".to_owned()];
        for c in &cases {
            let n = arity(c.data);
            let values: Vec<String> = (0..n).map(|i| format!("v{i}")).collect();
            body.push(format!("case .{}{}:", c.name, pattern(n)));
            let mut lines = Vec::new();
            let wire = string_literal(c.wire);
            match repr {
                EnumRepr::External => {
                    if let VariantData::Unit = c.data {
                        lines.push("var single = encoder.singleValueContainer()".to_owned());
                        lines.push(format!("try single.encode({wire})"));
                    } else {
                        let variant_key = key(c.wire);
                        lines.push(
                            "var container = encoder.container(keyedBy: Key.self)".to_owned(),
                        );
                        lines.extend(self.encode_data(
                            c.data,
                            &At::Key {
                                container: "container",
                                key: &variant_key,
                            },
                            &values,
                            name,
                        ));
                    }
                }
                EnumRepr::Internal { tag } => {
                    let tag_key = key(tag);
                    if let VariantData::Newtype(t) = c.data {
                        // serde writes the value's own fields beside the tag: an object only.
                        let value = to_wire(t, "v0");
                        lines.push(format!(
                            "guard case .object(_)? = try? JSONDecoder().decode(JSONValue.self, from: JSONEncoder().encode({value})) else {{"
                        ));
                        lines.push(format!(
                            "    throw EncodingError.invalidValue({value}, .init(codingPath: encoder.codingPath, debugDescription: \"{name}: serde tags only an object internally\"))"
                        ));
                        lines.push("}".to_owned());
                        lines.push(format!("try {value}.encode(to: encoder)"));
                        lines.push(
                            "var container = encoder.container(keyedBy: Key.self)".to_owned(),
                        );
                        lines.push(format!("try container.encode({wire}, forKey: {tag_key})"));
                    } else if let VariantData::Tuple(_) = c.data {
                        lines.extend(self.encode_data(
                            c.data,
                            &At::Same("container"),
                            &values,
                            name,
                        ));
                    } else {
                        lines.push(
                            "var container = encoder.container(keyedBy: Key.self)".to_owned(),
                        );
                        lines.push(format!("try container.encode({wire}, forKey: {tag_key})"));
                        lines.extend(self.encode_data(
                            c.data,
                            &At::Same("container"),
                            &values,
                            name,
                        ));
                    }
                }
                EnumRepr::Adjacent { tag, content } => {
                    let content_key = key(content);
                    lines.push("var container = encoder.container(keyedBy: Key.self)".to_owned());
                    lines.push(format!(
                        "try container.encode({wire}, forKey: {})",
                        key(tag)
                    ));
                    if !matches!(c.data, VariantData::Unit) {
                        lines.extend(self.encode_data(
                            c.data,
                            &At::Key {
                                container: "container",
                                key: &content_key,
                            },
                            &values,
                            name,
                        ));
                    }
                }
                EnumRepr::Untagged => {
                    lines.extend(self.encode_data(c.data, &At::Value, &values, name));
                }
            }
            for line in lines {
                body.push(format!("    {line}"));
            }
        }
        body.push("}".to_owned());
        for line in &body {
            let _ = writeln!(out, "            {line}");
        }
        let _ = writeln!(out, "        }}\n    }}");
    }

    /// One `case "<name>":` of a tagged enum's `init(from:)`: its data read from `at`.
    fn decode_case(&mut self, body: &mut Vec<String>, c: &Case<'_>, at: &At<'_>, owner: &str) {
        body.push(format!("case {}:", string_literal(c.wire)));
        let (lines, reads) = self.decode_data(c.data, at, owner);
        for line in lines {
            body.push(format!("    {line}"));
        }
        if reads {
            body.push(format!("    self = .{}{}", c.name, c.args()));
        }
    }
}

/// A struct shape's stored values.
enum Stored<'s> {
    Record(&'s [Field]),
    Newtype(&'s Type),
    Tuple(&'s [Type]),
}

/// Field `f` written to keyed `container`; an `option` is written as `null` when `nil`, a field
/// the JSON may leave out is left out.
fn field_encode(f: &Field, container: &str, value: &str) -> String {
    let key = key(&f.name);
    if f.optional {
        let ty = if f.ty.is_optional() {
            f.ty.clone()
        } else {
            Type::Option(Box::new(f.ty.clone()))
        };
        return format!(
            "try {container}.encodeIfPresent({}, forKey: {key})",
            to_wire(&ty, value)
        );
    }
    format!(
        "try {container}.encode({}, forKey: {key})",
        to_wire(&f.ty, value)
    )
}

/// A property that holds its own type: boxed, so the struct has a size.
const INDIRECT: &str = r#"
    /// A property that holds the type it is in, boxed so that type has a size.
    @propertyWrapper
    public enum Indirect<Value: Sendable & Equatable>: Sendable, Equatable {
        indirect case wrapped(Value)

        public init(wrappedValue: Value) {
            self = .wrapped(wrappedValue)
        }

        public var wrappedValue: Value {
            get {
                switch self {
                case .wrapped(let value):
                    return value
                }
            }
            set {
                self = .wrapped(newValue)
            }
        }
    }
"#;

pub(super) fn module(abi: &AppAbi, class: &str) -> String {
    let mut names = Names::new(abi);
    names.reserve(RESERVED_TYPES.iter().copied());
    names.reserve([class]);
    let tuple_names: Vec<String> = (1..=16).map(|n| format!("Tuple{n}")).collect();
    names.reserve(tuple_names.iter().map(String::as_str));
    let mut sw = Sw {
        abi,
        names,
        named: BTreeMap::new(),
        described: BTreeMap::new(),
        params: Vec::new(),
        tuples: BTreeSet::new(),
        indirect: false,
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
    // The described types, and the ones their shapes name, until none is new.
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let pending: Vec<(String, (String, &TypeDef))> = sw
            .described
            .iter()
            .filter(|(name, _)| !declared.contains_key(*name))
            .map(|(n, d)| (n.clone(), d.clone()))
            .collect();
        if pending.is_empty() {
            break;
        }
        for (name, (entry, def)) in pending {
            let decl = sw.declaration(&name, &entry, def);
            let _ = declared.insert(name, decl);
        }
    }

    nested.push_str(WIRE);
    if sw.indirect {
        nested.push_str(INDIRECT);
    }
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
            "    /// The app type `{text}`: the ABI names it without its shape, so it is its JSON.\n    public typealias {name} = JSONValue\n"
        );
    }
    for decl in declared.values() {
        out.push_str(decl);
    }
    if !declared.is_empty() {
        out.push('\n');
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
        .map(|p| format!("{p}: Codable & Sendable & Equatable"))
        .collect();
    let _ = writeln!(
        out,
        "\n    /// `tuple<..>` of {n}: a JSON array `[v0, ..]`.\n    public struct Tuple{n}<{}>: Codable, Sendable, Equatable {{",
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

    fn sw() -> Sw<'static> {
        let abi: &'static AppAbi = Box::leak(Box::new(
            AppAbi::from_json(&json!({"abiVersion": 1, "methods": {}})).unwrap(),
        ));
        Sw {
            abi,
            names: Names::new(abi),
            named: BTreeMap::new(),
            described: BTreeMap::new(),
            params: Vec::new(),
            tuples: BTreeSet::new(),
            indirect: false,
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

    /// A described type is a nested Codable type (generic where the Rust type is, boxed where it
    /// holds itself); a type without a shape stays `JSONValue`.
    #[test]
    fn described_types_are_nested_types() {
        let abi = AppAbi::from_json(&json!({
            "abiVersion": 1,
            "methods": { "get": { "kind": "view", "params": [["page", "Page<Node>"], ["raw", "Raw"]], "returns": "Shape" } },
            "types": {
                "Node": { "kind": "record", "fields": [["id", "u64"], ["next", "option<Node>", { "optional": true }]] },
                "Page": { "kind": "record", "params": ["T"], "fields": [["items", "list<T>"]] },
                "Shape": { "kind": "enum", "tag": "kind", "variants": [{ "name": "dot" }, { "name": "box", "fields": [["inner", "Shape"]] }] },
            },
        }))
        .unwrap();
        let out = module(&abi, "AppClient");
        for expected in [
            "    public typealias Raw = JSONValue",
            "    public struct Node: Codable, Sendable, Equatable {\n        public var id: UInt64\n        @Indirect public var next: Node?",
            "    public struct Page<T: Codable & Sendable & Equatable>: Codable, Sendable, Equatable {",
            "    public indirect enum Shape: Codable, Sendable, Equatable {",
            "        case box(inner: Shape)",
            r#"let tag = try container.decode(String.self, forKey: Key("kind"))"#,
            "        page: Page<Node>,",
            "    public enum Indirect<Value: Sendable & Equatable>",
        ] {
            assert!(out.contains(expected), "{expected} not in\n{out}");
        }
        assert!(!out.contains("typealias Node"), "{out}");
    }
}
