//! The Kotlin client over kotlin-sdk's mero-core: one `suspend fun` per method, sent through
//! `CalimeroClient.apps.method` or a `ContextContractCalling`, the app's events and errors as
//! sealed interfaces, every value through a kotlinx.serialization serializer that writes the
//! ABI's JSON form (u64/i64 decimal strings, bytes base64url).
//!
//! An app type with a shape in the ABI's `types` is nested in the client: a record a
//! `data class`, an enum a `sealed interface` of its variants, a generic type a generic one, each
//! with a hand-written serializer (`Item.serializer()`, `Page.serializer(Item.serializer())`:
//! no compiler plugin) that follows its serde tagging (external, internal, adjacent, untagged).
//! Each is annotated `@Serializable(with = ..)`, naming that serializer, so kotlinx's
//! `serializer<T>()` finds it: by reflection on the companion's `serializer()`, which kotlinx's
//! bundled R8 rules keep for `@Serializable` classes, or, for a module that applies the
//! kotlinx.serialization compiler plugin (`serialization_plugin`), by the plugin's intrinsic,
//! with no reflection. The plugin writes `serializer()` itself for a `@Serializable` class, so
//! with it the file declares none (a hand-written one would clash with the plugin's).
//! A type the ABI does not describe stays its JSON: `JsonElement`, under no name of its own (a
//! top-level alias would clash with an app type of that name in the package).
//!
//! An optional argument left `null` is not sent: the SDK reads a missing argument as `None`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::abi::{AppAbi, EnumRepr, Field, Fields, Shape, TypeDef, VariantData};
use crate::openapi::{all_events, pascal, Names};
use crate::ty::{Int, Type};

use super::ident::{lower_first, sanitize, Scope};

/// Kotlin's hard keywords: an identifier spelled like one is written in backticks.
const KEYWORDS: &[&str] = &[
    "as",
    "break",
    "class",
    "continue",
    "do",
    "else",
    "false",
    "for",
    "fun",
    "if",
    "in",
    "interface",
    "is",
    "null",
    "object",
    "package",
    "return",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "typeof",
    "val",
    "var",
    "when",
    "while",
];

/// The simple names the file imports, declares or uses from `kotlin.*`: an app type named like
/// one is renamed, since its nested declaration would shadow it in the client.
const RESERVED_TYPES: &[&str] = &[
    "Any",
    "AppError",
    "AppMethodRequest",
    "Base64",
    "Base64Url",
    "Boolean",
    "Byte",
    "ByteArray",
    "CalimeroClient",
    "Char",
    "CloudContextContract",
    "ContextContractCalling",
    "Decoder",
    "Double",
    "Encoder",
    "Event",
    "EventFrame",
    "ExperimentalEncodingApi",
    "Float",
    "Flow",
    "I64AsString",
    "IllegalArgumentException",
    "Int",
    "Json",
    "JsonArray",
    "JsonDecoder",
    "JsonElement",
    "JsonEncoder",
    "JsonNull",
    "JsonObject",
    "JsonPrimitive",
    "KSerializer",
    "List",
    "ListSerializer",
    "Long",
    "Map",
    "MapSerializer",
    "MeroAppBackend",
    "Nothing",
    "Pair",
    "PasskeyProvider",
    "PrimitiveKind",
    "PrimitiveSerialDescriptor",
    "Refusal",
    "SerialDescriptor",
    "Serializable",
    "SerializationException",
    "Short",
    "String",
    "U64AsString",
    "UByte",
    "UInt",
    "ULong",
    "UShort",
    "Unit",
    "UnsupportedOperationException",
];

/// Members of the client class besides the app's methods.
const MEMBERS: &[&str] = &[
    "invokeCall",
    "invokeView",
    "invokeConfirmed",
    "contract",
    "client",
    "app",
    "json",
    "encode",
    "field",
    "newKey",
    "serializer",
    "toString",
    "hashCode",
    "equals",
];

const IMPORTS: &str = "import com.calimero.mero.AppError
import com.calimero.mero.CalimeroClient
import com.calimero.mero.PasskeyProvider
import com.calimero.mero.api.models.AppMethodRequest
import com.calimero.mero.app.CloudContextContract
import com.calimero.mero.app.ContextContractCalling
import com.calimero.mero.app.MeroAppBackend
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.MapSerializer
import kotlinx.serialization.builtins.nullable
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonDecoder
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonEncoder
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.serializer
import kotlin.io.encoding.Base64
import kotlin.io.encoding.ExperimentalEncodingApi
";

/// The wire helpers every client nests: the ABI's string forms as serializers.
const WIRE: &str = r#"
    /** A `u64` as the wire carries it: a decimal string (a number is read too). */
    object U64AsString : KSerializer<ULong> {
        override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("calimero.u64", PrimitiveKind.STRING)

        override fun serialize(
            encoder: Encoder,
            value: ULong,
        ) = encoder.encodeString(value.toString())

        override fun deserialize(decoder: Decoder): ULong {
            val text = (decoder as? JsonDecoder)?.decodeJsonElement()?.jsonPrimitive?.content ?: decoder.decodeString()
            return text.toULongOrNull() ?: throw SerializationException("not a u64: $text")
        }
    }

    /** An `i64` as the wire carries it: a decimal string (a number is read too). */
    object I64AsString : KSerializer<Long> {
        override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("calimero.i64", PrimitiveKind.STRING)

        override fun serialize(
            encoder: Encoder,
            value: Long,
        ) = encoder.encodeString(value.toString())

        override fun deserialize(decoder: Decoder): Long {
            val text = (decoder as? JsonDecoder)?.decodeJsonElement()?.jsonPrimitive?.content ?: decoder.decodeString()
            return text.toLongOrNull() ?: throw SerializationException("not an i64: $text")
        }
    }

    /** `bytes` as the wire carries them: base64url without padding (padding is read too). */
    @OptIn(ExperimentalEncodingApi::class)
    object Base64Url : KSerializer<ByteArray> {
        override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("calimero.bytes", PrimitiveKind.STRING)

        override fun serialize(
            encoder: Encoder,
            value: ByteArray,
        ) = encoder.encodeString(Base64.UrlSafe.encode(value).trimEnd('='))

        override fun deserialize(decoder: Decoder): ByteArray {
            val text = decoder.decodeString().trimEnd('=')
            return try {
                Base64.UrlSafe.decode(text.padEnd((text.length + 3) / 4 * 4, '='))
            } catch (e: IllegalArgumentException) {
                throw SerializationException("not base64url: ${e.message}")
            }
        }
    }
"#;

struct Kt<'a> {
    abi: &'a AppAbi,
    names: Names,
    /// Whether the module applies the kotlinx.serialization plugin, which writes `serializer()`.
    plugin: bool,
    /// App types the ABI describes, met so far: Kotlin name to entry name and entry.
    described: BTreeMap<String, (String, &'a TypeDef)>,
    /// The generic parameters in scope (inside a generic entry's declaration).
    params: Vec<String>,
    /// The tuple arities used.
    tuples: BTreeSet<usize>,
    /// The companion's helpers the declarations use.
    helpers: BTreeSet<&'static str>,
}

fn int_type(int: Int) -> &'static str {
    match int {
        Int::U8 => "UByte",
        Int::U16 => "UShort",
        Int::U32 | Int::Usize => "UInt",
        Int::U64 | Int::U128 | Int::U64Number => "ULong",
        Int::I8 => "Byte",
        Int::I16 => "Short",
        Int::I32 | Int::Isize => "Int",
        Int::I64 | Int::I128 | Int::I64Number => "Long",
    }
}

impl Kt<'_> {
    /// The Kotlin type of an ABI type.
    fn ty(&mut self, ty: &Type) -> String {
        match ty {
            // `null` on the wire.
            Type::Unit => "JsonElement".into(),
            Type::Bool => "Boolean".into(),
            Type::Int(int) => int_type(*int).into(),
            Type::Float(false) => "Float".into(),
            Type::Float(true) => "Double".into(),
            // A Rust `char` is a code point, which a Kotlin `Char` may not hold.
            Type::Char | Type::String => "String".into(),
            Type::Bytes => "ByteArray".into(),
            Type::Option(inner) => {
                let inner = self.ty(inner);
                if inner.ends_with('?') {
                    inner
                } else {
                    format!("{inner}?")
                }
            }
            Type::List(inner) | Type::Set(inner) | Type::Array(inner, _) => {
                format!("List<{}>", self.ty(inner))
            }
            Type::Tuple(items) => {
                let _ = self.tuples.insert(items.len());
                let items: Vec<String> = items.iter().map(|t| self.ty(t)).collect();
                format!("Tuple{}<{}>", items.len(), items.join(", "))
            }
            Type::Map(_, value) => format!("Map<String, {}>", self.ty(value)),
            Type::Named(text) if self.params.contains(text) => text.clone(),
            Type::Named(_) => match self.described(ty) {
                Some((name, args)) if args.is_empty() => name,
                Some((name, args)) => {
                    let args: Vec<String> = args.iter().map(|a| self.ty(a)).collect();
                    format!("{name}<{}>", args.join(", "))
                }
                // A type the ABI names without its shape: its JSON.
                None => "JsonElement".to_owned(),
            },
        }
    }

    /// The serializer that writes an ABI type's JSON form.
    fn ser(&mut self, ty: &Type) -> String {
        match ty {
            Type::Unit => "JsonElement.serializer()".into(),
            Type::Bool => "Boolean.serializer()".into(),
            Type::Int(Int::U64) => "U64AsString".into(),
            Type::Int(Int::I64) => "I64AsString".into(),
            Type::Int(int) => format!("{}.serializer()", int_type(*int)),
            Type::Float(false) => "Float.serializer()".into(),
            Type::Float(true) => "Double.serializer()".into(),
            Type::Char | Type::String => "String.serializer()".into(),
            Type::Bytes => "Base64Url".into(),
            // `T??` is `T?`: one `nullable` serves both.
            Type::Option(inner) if inner.is_optional() => self.ser(inner),
            Type::Option(inner) => format!("{}.nullable", self.ser(inner)),
            Type::List(inner) | Type::Set(inner) | Type::Array(inner, _) => {
                format!("ListSerializer({})", self.ser(inner))
            }
            Type::Tuple(items) => {
                let _ = self.tuples.insert(items.len());
                let items: Vec<String> = items.iter().map(|t| self.ser(t)).collect();
                format!("Tuple{}Serializer({})", items.len(), items.join(", "))
            }
            Type::Map(_, value) => {
                format!("MapSerializer(String.serializer(), {})", self.ser(value))
            }
            Type::Named(text) if self.params.contains(text) => param_serializer(text),
            Type::Named(_) => match self.described(ty) {
                Some((name, args)) => {
                    let args: Vec<String> = args.iter().map(|a| self.ser(a)).collect();
                    format!("{name}.serializer({})", args.join(", "))
                }
                // Its JSON. Never `serializer<T>()`: inside a declaration's companion,
                // `serializer` is the companion's own.
                None => "JsonElement.serializer()".to_owned(),
            },
        }
    }

    /// A described app type's Kotlin name and generic arguments.
    fn described(&mut self, ty: &Type) -> Option<(String, Vec<Type>)> {
        let (entry, def, args) = self.abi.shape_of(ty)?;
        let (name, _) = self.names.named(&entry);
        let _ = self.described.insert(name.clone(), (entry, def));
        Some((name, args))
    }
}

/// The types a shape names.
fn shape_types(shape: &Shape) -> Vec<Type> {
    let data = |d: &VariantData| match d {
        VariantData::Unit => Vec::new(),
        VariantData::Newtype(t) => vec![t.clone()],
        VariantData::Tuple(ts) => ts.clone(),
        VariantData::Record(fs) => fs.iter().map(|f| f.ty.clone()).collect(),
    };
    match shape {
        Shape::Record(fs) => fs.iter().map(|f| f.ty.clone()).collect(),
        Shape::Newtype(t) => vec![t.clone()],
        Shape::Tuple(ts) => ts.clone(),
        Shape::Unit => Vec::new(),
        Shape::Enum { variants, .. } => variants.iter().flat_map(|v| data(&v.data)).collect(),
    }
}

/// The serializer a generic declaration takes for parameter `param`: `T` → `tSerializer`.
fn param_serializer(param: &str) -> String {
    format!("{}Serializer", lower_first(&sanitize(param)))
}

/// An ABI name as a Kotlin identifier, in backticks when it is a keyword.
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
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One case of an event or error: its Kotlin name, ABI name and fields.
struct Variant {
    name: String,
    wire: String,
    fields: Vec<(String, String, Type)>,
}

fn variants(entries: &BTreeMap<String, Fields>, scope: &mut Scope) -> Vec<Variant> {
    entries
        .iter()
        .map(|(wire, fields)| {
            let mut field_scope = Scope::new(&[]);
            Variant {
                name: scope.take(sanitize(&pascal(wire))),
                wire: wire.clone(),
                fields: fields
                    .iter()
                    .map(|(f, t)| (field_scope.take(ident(f)), f.clone(), t.clone()))
                    .collect(),
            }
        })
        .collect()
}

impl Kt<'_> {
    /// A sealed interface's variants, each a data class (or a data object without fields).
    fn variant_classes(
        &mut self,
        out: &mut String,
        parent: &str,
        what: &str,
        variants: &[Variant],
    ) {
        for v in variants {
            let _ = writeln!(out, "        /** The {what} `{}`. */", v.wire);
            if v.fields.is_empty() {
                let _ = writeln!(out, "        data object {} : {parent}\n", v.name);
                continue;
            }
            let fields: Vec<(String, String)> = v
                .fields
                .iter()
                .map(|(name, _, t)| (name.clone(), self.ty(t)))
                .collect();
            if fields.iter().any(|(_, t)| t.starts_with("ByteArray")) {
                let _ = writeln!(out, "        @Suppress(\"ArrayInDataClass\")");
            }
            let _ = writeln!(out, "        data class {}(", v.name);
            for (name, ty) in fields {
                let _ = writeln!(out, "            val {name}: {ty},");
            }
            let _ = writeln!(out, "        ) : {parent}\n");
        }
    }

    /// `Name(a = field(data, "a", ...), ...)` or `Name`.
    fn construct(&mut self, v: &Variant, data: &str) -> String {
        if v.fields.is_empty() {
            return v.name.clone();
        }
        let args: Vec<String> = v
            .fields
            .iter()
            .map(|(name, wire, t)| {
                format!(
                    "{name} = field({data}, {}, {})",
                    string_literal(wire),
                    self.ser(t)
                )
            })
            .collect();
        format!("{}({})", v.name, args.join(", "))
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

/// A shape's stored values: (property, ABI field). A newtype's is `value`, a tuple's `v0`, ...
fn properties(data: &VariantData) -> Vec<(String, Field)> {
    let plain = |name: String, ty: &Type| {
        (
            name.clone(),
            Field {
                name,
                ty: ty.clone(),
                optional: false,
            },
        )
    };
    match data {
        VariantData::Unit => Vec::new(),
        VariantData::Newtype(t) => vec![plain("value".to_owned(), t)],
        VariantData::Tuple(ts) => ts
            .iter()
            .enumerate()
            .map(|(i, t)| plain(format!("v{i}"), t))
            .collect(),
        VariantData::Record(fs) => {
            let mut scope = Scope::new(&[]);
            fs.iter()
                .map(|f| (scope.take(ident(&f.name)), f.clone()))
                .collect()
        }
    }
}

/// `text` indented by `pad` after its first line.
fn indent(text: &str, pad: &str) -> String {
    text.replace('\n', &format!("\n{pad}"))
}

/// The companion's helpers the shapes' serializers call, each declared when used.
const HELPERS: &[(&str, &str)] = &[
    (
        "attempt",
        "
        /** [block]'s value, or `null` when it does not read (an untagged variant that does not fit). */
        private fun <T> attempt(block: () -> T): T? =
            try {
                block()
            } catch (e: IllegalArgumentException) {
                null
            }",
    ),
    (
        "decode",
        "
        private fun <T> decode(
            serializer: KSerializer<T>,
            data: JsonElement,
        ): T = json.decodeFromJsonElement(serializer, data)",
    ),
    (
        "external",
        "
        /** An externally tagged variant's name and data: `\"Name\"`, or `{ \"Name\": data }`. */
        private fun external(
            data: JsonElement,
            what: String,
        ): Pair<String, JsonElement> =
            when {
                data is JsonPrimitive && data.isString -> data.content to JsonNull
                data is JsonObject && data.size == 1 -> data.entries.first().let { it.key to it.value }
                else -> throw SerializationException(\"$what: expected a variant name or an object with one key\")
            }",
    ),
    (
        "items",
        "
        /** [data] as an array of exactly [n] items. */
        private fun items(
            data: JsonElement,
            n: Int,
            what: String,
        ): JsonArray {
            val items = data as? JsonArray ?: throw SerializationException(\"$what: expected an array\")
            if (items.size != n) throw SerializationException(\"$what: expected $n items, got ${items.size}\")
            return items
        }",
    ),
    (
        "obj",
        "
        /** [data] as an object. */
        private fun obj(
            data: JsonElement,
            what: String,
        ): JsonObject = data as? JsonObject ?: throw SerializationException(\"$what: expected an object\")",
    ),
    (
        "tag",
        "
        /** The variant name at [key] of a tagged enum's object. */
        private fun tag(
            data: JsonElement,
            key: String,
            what: String,
        ): String {
            val tag = obj(data, what)[key] as? JsonPrimitive
            if (tag == null || !tag.isString) throw SerializationException(\"$what: no `$key`\")
            return tag.content
        }",
    ),
    (
        "tagged",
        "
        /** An internally tagged variant: [data]'s fields with the tag [key] = [name] first. */
        private fun tagged(
            key: String,
            name: String,
            data: JsonElement,
            what: String,
        ): JsonObject = JsonObject(mapOf(key to JsonPrimitive(name)) + obj(data, what))",
    ),
];

/// One variant of a described enum: its class name, JSON name and data.
struct Case<'v> {
    name: String,
    wire: &'v str,
    data: &'v VariantData,
}

impl Kt<'_> {
    /// A record field's Kotlin type: one the JSON may leave out is nullable.
    fn field_type(&mut self, f: &Field) -> String {
        let ty = self.ty(&f.ty);
        if f.optional && !ty.ends_with('?') {
            format!("{ty}?")
        } else {
            ty
        }
    }

    fn field_ser(&mut self, f: &Field) -> String {
        if f.optional && !f.ty.is_optional() {
            format!("{}.nullable", self.ser(&f.ty))
        } else {
            self.ser(&f.ty)
        }
    }

    fn helper(&mut self, name: &'static str) -> &'static str {
        let _ = self.helpers.insert(name);
        name
    }

    /// The statements and expression that build `ctor` from the JSON element `data`. `same`:
    /// the fields sit beside an internal tag in `data`, already an object. `strict`: a unit
    /// must be `null` (untagged, unit structs).
    fn decode_data(
        &mut self,
        data: &VariantData,
        ctor: &str,
        element: &str,
        owner: &str,
        same: bool,
        strict: bool,
    ) -> (Vec<String>, String) {
        let mut lines = Vec::new();
        if arity(data) == 0 {
            match data {
                VariantData::Unit if strict => lines.push(format!(
                    "if ({element} !is JsonNull) throw SerializationException(\"{owner}: expected null\")"
                )),
                VariantData::Record(_) if !same => {
                    let obj = self.helper("obj");
                    lines.push(format!("{obj}({element}, \"{owner}\")"));
                }
                VariantData::Tuple(_) if !same => {
                    let items = self.helper("items");
                    lines.push(format!("{items}({element}, 0, \"{owner}\")"));
                }
                _ => {}
            }
            return (lines, ctor.to_owned());
        }
        let args: Vec<String> = match data {
            VariantData::Unit => Vec::new(),
            VariantData::Record(_) => {
                let source = if same {
                    element.to_owned()
                } else {
                    let obj = self.helper("obj");
                    lines.push(format!("val fields = {obj}({element}, \"{owner}\")"));
                    "fields".to_owned()
                };
                properties(data)
                    .iter()
                    .map(|(prop, f)| {
                        let ser = self.field_ser(f);
                        format!(
                            "{prop} = field({source}, {}, {ser})",
                            string_literal(&f.name)
                        )
                    })
                    .collect()
            }
            VariantData::Tuple(items) => {
                if same {
                    return (
                        Vec::new(),
                        format!("throw SerializationException(\"{owner}: serde cannot tag a tuple variant internally\")"),
                    );
                }
                let helper = self.helper("items");
                lines.push(format!(
                    "val items = {helper}({element}, {}, \"{owner}\")",
                    items.len()
                ));
                let decode = self.helper("decode");
                items
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("{decode}({}, items[{i}])", self.ser(t)))
                    .collect()
            }
            VariantData::Newtype(t) => {
                let decode = self.helper("decode");
                vec![format!("{decode}({}, {element})", self.ser(t))]
            }
        };
        if args.len() == 1 {
            return (lines, format!("{ctor}({})", args[0]));
        }
        let args: Vec<String> = args.iter().map(|a| format!("    {a},")).collect();
        (lines, format!("{ctor}(\n{}\n)", args.join("\n")))
    }

    /// The JSON element of a shape's data, read from `value`; `tag` puts an internal tag first.
    fn encode_data(&mut self, data: &VariantData, tag: Option<(&str, &str)>, pad: &str) -> String {
        let props = properties(data);
        match data {
            VariantData::Unit if tag.is_none() => "JsonNull".to_owned(),
            VariantData::Unit | VariantData::Record(_) => {
                let mut out = String::from("buildJsonObject {\n");
                if let Some((key, name)) = tag {
                    let _ = writeln!(
                        out,
                        "{pad}    put({}, JsonPrimitive({}))",
                        string_literal(key),
                        string_literal(name)
                    );
                }
                for (prop, f) in &props {
                    let ser = self.field_ser(f);
                    let put = format!(
                        "put({}, encode({ser}, value.{prop}))",
                        string_literal(&f.name)
                    );
                    if f.optional {
                        let _ = writeln!(out, "{pad}    if (value.{prop} != null) {put}");
                    } else {
                        let _ = writeln!(out, "{pad}    {put}");
                    }
                }
                let _ = write!(out, "{pad}}}");
                out
            }
            VariantData::Tuple(_) => {
                let items: Vec<String> = props
                    .iter()
                    .map(|(prop, f)| format!("encode({}, value.{prop})", self.ser(&f.ty)))
                    .collect();
                if items.is_empty() {
                    "JsonArray(emptyList())".to_owned()
                } else {
                    format!("JsonArray(listOf({}))", items.join(", "))
                }
            }
            VariantData::Newtype(t) => format!("encode({}, value.value)", self.ser(t)),
        }
    }

    /// `val a: A,` lines of a data class's constructor.
    fn constructor(&mut self, out: &mut String, data: &VariantData, pad: &str, defaults: bool) {
        for (prop, f) in properties(data) {
            let ty = self.field_type(&f);
            let default = if defaults && ty.ends_with('?') {
                " = null"
            } else {
                ""
            };
            let _ = writeln!(out, "{pad}    val {prop}: {ty}{default},");
        }
    }

    fn has_bytes(&mut self, data: &VariantData) -> bool {
        properties(data)
            .iter()
            .any(|(_, f)| self.field_type(f).starts_with("ByteArray"))
    }

    /// The companion's `serializer()` and its `Wire`, around `serialize` and `deserialize`
    /// bodies (each at 16 spaces).
    fn serializer(
        &mut self,
        out: &mut String,
        name: &str,
        params: &[String],
        serialize: &str,
        deserialize: &str,
        pad: &str,
    ) {
        let ty = if params.is_empty() {
            name.to_owned()
        } else {
            format!("{name}<{}>", params.join(", "))
        };
        let generics = if params.is_empty() {
            String::new()
        } else {
            format!("<{}> ", params.join(", "))
        };
        let args: Vec<String> = params
            .iter()
            .map(|p| format!("{}: KSerializer<{p}>", param_serializer(p)))
            .collect();
        let names: Vec<String> = params.iter().map(|p| param_serializer(p)).collect();
        let _ = writeln!(
            out,
            "{pad}/** `{name}`'s JSON form{}. */",
            if params.is_empty() {
                String::new()
            } else {
                ", each parameter's by its serializer".to_owned()
            }
        );
        // With the plugin, the plugin writes `serializer()` from `@Serializable(with = Wire)`.
        if !self.plugin && params.is_empty() {
            let _ = writeln!(out, "{pad}fun serializer(): KSerializer<{ty}> = Wire\n");
        } else if !self.plugin {
            let _ = writeln!(
                out,
                "{pad}fun {generics}serializer({}): KSerializer<{ty}> = Wire({})\n",
                args.join(", "),
                names.join(", ")
            );
        }
        if params.is_empty() {
            let _ = writeln!(out, "{pad}internal object Wire : KSerializer<{ty}> {{");
        } else {
            let _ = writeln!(out, "{pad}internal class Wire<{}>(", params.join(", "));
            for a in &args {
                let _ = writeln!(out, "{pad}    private val {a},");
            }
            let _ = writeln!(out, "{pad}) : KSerializer<{ty}> {{");
        }
        let _ = writeln!(
            out,
            "{pad}    override val descriptor: SerialDescriptor = JsonElement.serializer().descriptor

{pad}    override fun serialize(
{pad}        encoder: Encoder,
{pad}        value: {ty},
{pad}    ) {{
{pad}        {}
{pad}    }}

{pad}    override fun deserialize(decoder: Decoder): {ty} {{
{pad}        {}
{pad}    }}
{pad}}}",
            indent(serialize, &format!("{pad}        ")),
            indent(deserialize, &format!("{pad}        "))
        );
    }

    /// The declaration of a described app type, nested in the client; `taken` are the names
    /// a variant class must not shadow.
    fn declaration(&mut self, name: &str, entry: &str, def: &TypeDef, taken: &[String]) -> String {
        self.params.clone_from(&def.params);
        let params = def.params.clone();
        let generics = if params.is_empty() {
            String::new()
        } else {
            format!("<{}>", params.join(", "))
        };
        let mut out = format!("\n    /** The app type `{entry}`. */\n");
        match &def.shape {
            Shape::Enum { repr, variants } => {
                self.enumeration(&mut out, name, &params, repr, variants, taken);
            }
            shape => {
                let data = match shape {
                    Shape::Record(fields) => VariantData::Record(fields.clone()),
                    Shape::Newtype(t) => VariantData::Newtype(t.clone()),
                    Shape::Tuple(items) => VariantData::Tuple(items.clone()),
                    _ => VariantData::Unit,
                };
                let encoded = self.encode_data(&data, None, "");
                let serialize = format!(
                    "(encoder as JsonEncoder).encodeJsonElement(\n    {},\n)",
                    indent(&encoded, "    ")
                );
                let ctor = if arity(&data) == 0 || params.is_empty() {
                    name.to_owned()
                } else {
                    format!("{name}<{}>", params.join(", "))
                };
                let (lines, made) = self.decode_data(&data, &ctor, "data", name, false, true);
                let mut deserialize =
                    String::from("val data = (decoder as JsonDecoder).decodeJsonElement()\n");
                for line in lines {
                    let _ = writeln!(deserialize, "{line}");
                }
                let _ = write!(deserialize, "return {made}");
                if arity(&data) == 0 {
                    let _ = writeln!(out, "    @Serializable(with = {name}.Wire::class)");
                    let _ = writeln!(out, "    data object {name} {{");
                    self.serializer(&mut out, name, &[], &serialize, &deserialize, "        ");
                    let _ = writeln!(out, "    }}");
                } else {
                    if self.has_bytes(&data) {
                        let _ = writeln!(out, "    @Suppress(\"ArrayInDataClass\")");
                    }
                    let _ = writeln!(
                        out,
                        "    @Serializable(with = {name}.Companion.Wire::class)"
                    );
                    let _ = writeln!(out, "    data class {name}{generics}(");
                    self.constructor(&mut out, &data, "    ", true);
                    let _ = writeln!(out, "    ) {{\n        companion object {{");
                    self.serializer(
                        &mut out,
                        name,
                        &params,
                        &serialize,
                        &deserialize,
                        "            ",
                    );
                    let _ = writeln!(out, "        }}\n    }}");
                }
            }
        }
        self.params.clear();
        out
    }

    /// A sealed interface of `variants`, with the serializer of serde's tagging `repr`.
    fn enumeration(
        &mut self,
        out: &mut String,
        name: &str,
        params: &[String],
        repr: &EnumRepr,
        variants: &[crate::abi::Variant],
        taken: &[String],
    ) {
        // A variant class shadows, inside the interface, a type its serializer names; `Pair` is
        // only the companion's (`external`).
        let mut reserved: Vec<&str> = taken
            .iter()
            .map(String::as_str)
            .filter(|t| *t != "Pair")
            .collect();
        reserved.extend(["Companion", "Wire", name]);
        reserved.extend(params.iter().map(String::as_str));
        let mut scope = Scope::new(&reserved);
        let cases: Vec<Case<'_>> = variants
            .iter()
            .map(|v| Case {
                name: scope.take(sanitize(&pascal(&v.name))),
                wire: &v.name,
                data: &v.data,
            })
            .collect();
        let generic = !params.is_empty();
        let (out_params, own, nothing) = if generic {
            let outs: Vec<String> = params.iter().map(|p| format!("out {p}")).collect();
            let nothing: Vec<&str> = params.iter().map(|_| "Nothing").collect();
            (
                format!("<{}>", outs.join(", ")),
                format!("<{}>", params.join(", ")),
                format!("{name}<{}>", nothing.join(", ")),
            )
        } else {
            (String::new(), String::new(), name.to_owned())
        };
        let _ = writeln!(
            out,
            "    @Serializable(with = {name}.Companion.Wire::class)"
        );
        let _ = writeln!(out, "    sealed interface {name}{out_params} {{");
        for c in &cases {
            let _ = writeln!(out, "        /** `{}`. */", c.wire);
            if arity(c.data) == 0 {
                let _ = writeln!(out, "        data object {} : {nothing}\n", c.name);
                continue;
            }
            if self.has_bytes(c.data) {
                let _ = writeln!(out, "        @Suppress(\"ArrayInDataClass\")");
            }
            let _ = writeln!(out, "        data class {}{own}(", c.name);
            self.constructor(out, c.data, "        ", false);
            let _ = writeln!(out, "        ) : {name}{own}\n");
        }

        // serialize
        let mut arms = Vec::new();
        for c in &cases {
            let pad = "        ";
            let element = match repr {
                EnumRepr::External if arity(c.data) == 0 && matches!(c.data, VariantData::Unit) => {
                    format!("JsonPrimitive({})", string_literal(c.wire))
                }
                EnumRepr::External => {
                    let data = self.encode_data(c.data, None, pad);
                    format!("JsonObject(mapOf({} to {data}))", string_literal(c.wire))
                }
                EnumRepr::Internal { tag } => match c.data {
                    VariantData::Newtype(t) => {
                        let tagged = self.helper("tagged");
                        format!(
                            "{tagged}({}, {}, encode({}, value.value), \"{name}\")",
                            string_literal(tag),
                            string_literal(c.wire),
                            self.ser(t)
                        )
                    }
                    VariantData::Tuple(_) => format!(
                        "throw SerializationException(\"{name}: serde cannot tag a tuple variant internally\")"
                    ),
                    data => self.encode_data(data, Some((tag, c.wire)), pad),
                },
                EnumRepr::Adjacent { tag, content } => {
                    let mut out = format!(
                        "buildJsonObject {{\n{pad}    put({}, JsonPrimitive({}))\n",
                        string_literal(tag),
                        string_literal(c.wire)
                    );
                    if !matches!(c.data, VariantData::Unit) {
                        let data = self.encode_data(c.data, None, &format!("{pad}    "));
                        let _ = writeln!(out, "{pad}    put({}, {data})", string_literal(content));
                    }
                    let _ = write!(out, "{pad}}}");
                    out
                }
                EnumRepr::Untagged => self.encode_data(c.data, None, pad),
            };
            arms.push(format!("    is {} -> {element}", c.name));
        }
        let serialize = if cases.is_empty() {
            format!("throw SerializationException(\"{name} has no variants\")")
        } else {
            format!(
                "(encoder as JsonEncoder).encodeJsonElement(\n    when (value) {{\n{}\n    }},\n)",
                arms.iter()
                    .map(|a| format!("    {a}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        // deserialize
        let mut d = String::from("val data = (decoder as JsonDecoder).decodeJsonElement()\n");
        let ctor = |c: &Case<'_>| {
            if generic && arity(c.data) > 0 {
                format!("{}{own}", c.name)
            } else {
                c.name.clone()
            }
        };
        let arm = |lines: Vec<String>, made: String| {
            if lines.is_empty() {
                made
            } else {
                let mut block = String::from("{\n");
                for line in lines {
                    let _ = writeln!(block, "    {line}");
                }
                let _ = write!(block, "    {}\n}}", indent(&made, "    "));
                block
            }
        };
        match repr {
            _ if cases.is_empty() => {
                let _ = write!(
                    d,
                    "throw SerializationException(\"{name} has no variants\")"
                );
            }
            EnumRepr::External => {
                let external = self.helper("external");
                let all_units = cases.iter().all(|c| matches!(c.data, VariantData::Unit));
                if all_units {
                    let _ = writeln!(d, "val name = {external}(data, \"{name}\").first");
                } else {
                    let _ = writeln!(d, "val (name, content) = {external}(data, \"{name}\")");
                }
                let _ = writeln!(d, "return when (name) {{");
                for c in &cases {
                    let (lines, made) =
                        self.decode_data(c.data, &ctor(c), "content", name, false, false);
                    let _ = writeln!(
                        d,
                        "    {} -> {}",
                        string_literal(c.wire),
                        indent(&arm(lines, made), "    ")
                    );
                }
                let _ = write!(
                    d,
                    "    else -> throw SerializationException(\"{name}: unknown variant $name\")\n}}"
                );
            }
            EnumRepr::Internal { tag: key } | EnumRepr::Adjacent { tag: key, .. } => {
                let tag = self.helper("tag");
                let _ = writeln!(
                    d,
                    "val name = {tag}(data, {}, \"{name}\")",
                    string_literal(key)
                );
                let element = if let EnumRepr::Adjacent { content, .. } = repr {
                    if cases.iter().any(|c| !matches!(c.data, VariantData::Unit)) {
                        let obj = self.helper("obj");
                        let _ = writeln!(
                            d,
                            "val content = {obj}(data, \"{name}\")[{}] ?: JsonNull",
                            string_literal(content)
                        );
                    }
                    "content"
                } else {
                    "data"
                };
                let _ = writeln!(d, "return when (name) {{");
                for c in &cases {
                    let same = element == "data" && !matches!(c.data, VariantData::Newtype(_));
                    let (lines, made) =
                        self.decode_data(c.data, &ctor(c), element, name, same, false);
                    let _ = writeln!(
                        d,
                        "    {} -> {}",
                        string_literal(c.wire),
                        indent(&arm(lines, made), "    ")
                    );
                }
                let _ = write!(
                    d,
                    "    else -> throw SerializationException(\"{name}: unknown variant $name\")\n}}"
                );
            }
            EnumRepr::Untagged => {
                // serde tries the variants in order; the first that reads is the value.
                let attempt = self.helper("attempt");
                let mut tries = Vec::new();
                for c in &cases {
                    let (lines, made) =
                        self.decode_data(c.data, &ctor(c), "data", name, false, true);
                    let mut block = format!("{attempt} {{\n");
                    for line in lines {
                        let _ = writeln!(block, "    {line}");
                    }
                    let _ = write!(block, "    {}\n}}", indent(&made, "    "));
                    tries.push(block);
                }
                let _ = write!(
                    d,
                    "return {}\n    ?: throw SerializationException(\"{name}: no variant fits\")",
                    tries
                        .iter()
                        .map(|t| indent(t, "    "))
                        .collect::<Vec<_>>()
                        .join("\n    ?: ")
                );
            }
        }

        let _ = writeln!(out, "        companion object {{");
        self.serializer(out, name, params, &serialize, &d, "            ");
        let _ = writeln!(out, "        }}\n    }}");
    }
}

pub(super) fn module(abi: &AppAbi, class: &str, package: &str, plugin: bool) -> String {
    let mut names = Names::new(abi);
    names.reserve(RESERVED_TYPES.iter().copied());
    names.reserve([class]);
    let tuple_names: Vec<String> = (1..=16)
        .flat_map(|n| [format!("Tuple{n}"), format!("Tuple{n}Serializer")])
        .collect();
    names.reserve(tuple_names.iter().map(String::as_str));
    let mut kt = Kt {
        abi,
        names,
        plugin,
        described: BTreeMap::new(),
        params: Vec::new(),
        tuples: BTreeSet::new(),
        helpers: BTreeSet::new(),
    };

    let mut members = Scope::new(MEMBERS);
    let methods: Vec<_> = abi
        .client_methods()
        .map(|(name, m)| (members.take(ident(name)), name, m))
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
        let mut puts = Vec::new();
        for (p, t) in &m.params {
            let arg = params.take(ident(p));
            let ty = kt.ty(t);
            let default = if t.is_optional() { " = null" } else { "" };
            sig.push(format!("{arg}: {ty}{default}"));
            let put = format!("put({}, encode({}, {arg}))", string_literal(p), kt.ser(t));
            // An unset optional argument is left out: the SDK reads it as `None`.
            puts.push(if t.is_optional() {
                format!("if ({arg} != null) {put}")
            } else {
                put
            });
        }
        if view {
            sig.push("stale: Boolean = false".into());
            sig.push("minIndex: Long = 0".into());
        } else if m.confirm {
            sig.push("passkey: PasskeyProvider".into());
            sig.push("idempotencyKey: String? = null".into());
        } else {
            sig.push("idempotencyKey: String? = null".into());
        }
        let args = if puts.is_empty() {
            "JsonObject(emptyMap())".to_owned()
        } else {
            format!(
                "buildJsonObject {{\n{}\n                }}",
                puts.iter()
                    .map(|p| format!("                    {p}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        let (helper, extra) = if view {
            ("invokeView", "stale,\n                minIndex")
        } else if m.confirm {
            (
                "invokeConfirmed",
                "passkey,\n                idempotencyKey",
            )
        } else {
            ("invokeCall", "idempotencyKey")
        };
        let unit = m.returns == Type::Unit;
        let result = kt.ser(&m.returns);
        let mut doc = format!("{} `{name}`", m.kind.name());
        if m.confirm {
            doc.push_str("; [passkey] approves it (`confirm`)");
        }
        if m.public {
            doc.push_str("; public");
        }
        let _ = writeln!(body, "\n    /** {doc}. */");
        let _ = writeln!(body, "    suspend fun {method}(");
        for s in &sig {
            let _ = writeln!(body, "        {s},");
        }
        let invoke = format!(
            "{helper}(\n                {},\n                {args},\n                {extra},\n                {result},\n            )",
            string_literal(name)
        );
        if unit {
            let _ = writeln!(
                body,
                "    ) {{\n        {}\n    }}",
                invoke.replace("\n    ", "\n")
            );
        } else {
            let ret = kt.ty(&m.returns);
            let _ = writeln!(
                body,
                "    ): {ret} =\n        {}",
                invoke.replace("\n    ", "\n")
            );
        }
    }

    // Events and errors, as sealed interfaces nested in the client.
    let events = all_events(abi);
    let mut event_scope = Scope::new(&["Companion", "Event"]);
    let event_variants = variants(&events, &mut event_scope);
    let unknown_event = event_scope.take("Unknown".into());
    let mut refusal_scope = Scope::new(&["Companion", "Refusal"]);
    let refusal_variants = variants(&abi.errors, &mut refusal_scope);
    let unknown_refusal = refusal_scope.take("Unknown".into());

    let mut nested = String::new();
    let _ = writeln!(
        nested,
        "\n    /** An event of the app, as [{stream}] and `MeroAppBackend.events` carry it. */\n    sealed interface Event {{"
    );
    kt.variant_classes(&mut nested, "Event", "event", &event_variants);
    let _ = writeln!(
        nested,
        "        /** An event this client does not know: one a newer release of the app added. */\n        data class {unknown_event}(\n            val kind: String,\n            val data: JsonElement,\n        ) : Event\n"
    );
    let _ = writeln!(
        nested,
        "        companion object {{\n            /** An event's [kind] and [data], typed; a known kind whose data does not fit throws [SerializationException]. */\n            fun decode(\n                kind: String,\n                data: JsonElement,\n            ): Event =\n                when (kind) {{"
    );
    for v in &event_variants {
        let made = kt.construct(v, "data");
        let _ = writeln!(
            nested,
            "                    {} -> {made}",
            string_literal(&v.wire)
        );
    }
    let _ = writeln!(
        nested,
        "                    else -> {unknown_event}(kind, data)\n                }}\n        }}\n    }}"
    );
    let _ = writeln!(
        nested,
        "\n    /** One frame of [{stream}]: the event's log position (`index:seq`) and the event. */\n    data class EventFrame(\n        val id: String,\n        val event: Event,\n    )"
    );

    let _ = writeln!(
        nested,
        "\n    /** The app's refusal: a `422 APP_ERROR`'s code and fields ([AppError.appCode], [AppError.fields]), typed. */\n    sealed interface Refusal {{"
    );
    kt.variant_classes(&mut nested, "Refusal", "error", &refusal_variants);
    let _ = writeln!(
        nested,
        "        /** A code this client does not know, or fields that do not fit it. */\n        data class {unknown_refusal}(\n            val code: String,\n            val fields: JsonObject,\n        ) : Refusal\n"
    );
    let _ = writeln!(
        nested,
        "        companion object {{\n            /** [error]'s code and fields, typed. */\n            fun from(error: AppError): Refusal =\n                try {{\n                    when (error.appCode) {{"
    );
    for v in &refusal_variants {
        let made = kt.construct(v, "error.fields");
        let _ = writeln!(
            nested,
            "                        {} -> {made}",
            string_literal(&v.wire)
        );
    }
    let _ = writeln!(
        nested,
        "                        else -> {unknown_refusal}(error.appCode, error.fields)\n                    }}\n                }} catch (e: IllegalArgumentException) {{\n                    {unknown_refusal}(error.appCode, error.fields)\n                }}\n        }}\n    }}"
    );

    // The described types, and the ones their shapes name, until none is new: first their
    // names (a variant class must not shadow one), then their declarations.
    let mut walked: BTreeSet<String> = BTreeSet::new();
    loop {
        let pending: Vec<(String, &TypeDef)> = kt
            .described
            .iter()
            .filter(|(name, _)| !walked.contains(*name))
            .map(|(n, (_, def))| (n.clone(), *def))
            .collect();
        if pending.is_empty() {
            break;
        }
        for (name, def) in pending {
            kt.params.clone_from(&def.params);
            for ty in shape_types(&def.shape) {
                let _ = kt.ty(&ty);
            }
            kt.params.clear();
            let _ = walked.insert(name);
        }
    }
    let taken: Vec<String> = RESERVED_TYPES
        .iter()
        .map(|t| (*t).to_owned())
        .chain(kt.described.keys().cloned())
        .chain([class.to_owned()])
        .collect();
    let described: Vec<(String, (String, &TypeDef))> = kt
        .described
        .iter()
        .map(|(n, d)| (n.clone(), d.clone()))
        .collect();
    for (name, (entry, def)) in described {
        let decl = kt.declaration(&name, &entry, def, &taken);
        nested.push_str(&decl);
    }

    nested.push_str(WIRE);
    for n in kt.tuples.clone() {
        tuple(&mut nested, n);
    }

    let mut out = String::new();
    let _ = writeln!(out, "// {}", super::provenance(abi));
    let _ = writeln!(
        out,
        "// A thin typed layer over kotlin-sdk's mero-core: each method calls `CalimeroClient.apps.method`\n// (`POST /apps/{{app}}/methods/{{name}}`) or a `ContextContractCalling`, so it travels the sealed\n// channel with the SDK's retries. u64 and i64 are decimal strings and bytes base64url on the wire;\n// here they are `ULong`, `Long` and `ByteArray`.\n"
    );
    let _ = writeln!(out, "package {package}\n");
    out.push_str(IMPORTS);
    out.push('\n');
    let _ = writeln!(
        out,
        "/**
 * The app's methods.
 *
 * Over a signed-in [CalimeroClient] every method runs, with idempotency keys, stale views, passkey
 * step-ups and [{stream}]. Over a [ContextContractCalling] (`MeroAppBackend.contract.bound(app)`, or a
 * double in a test) a view is `query` and a call `execute`: the contract reads strongly and makes its
 * own idempotency key, so `confirm` methods, [{stream}] and an `idempotencyKey` throw
 * [UnsupportedOperationException]. A refusal throws [AppError]; [Refusal.from] types it.
 */
class {class} private constructor(
    private val contract: ContextContractCalling,
    private val client: CalimeroClient?,
    private val app: String,
) {{
    /** Every method, as the account [client] signs in, in the app instance [app]. */
    constructor(client: CalimeroClient, app: String) : this(CloudContextContract(client).bound(app), client, app)

    /** Calls and views through [contract]. */
    constructor(contract: ContextContractCalling) : this(contract, null, \"\")

    /** Calls and views through [backend]'s contract in the app instance [app]. */
    constructor(backend: MeroAppBackend, app: String) : this(backend.contract.bound(app))"
    );
    out.push_str(&body);
    let _ = writeln!(
        out,
        "
    /** The app's events from log index [from], then live, until the collector stops. */
    fun {stream}(from: Long = 0): Flow<EventFrame> {{
        val node = client ?: throw UnsupportedOperationException(\"{stream} needs a CalimeroClient\")
        return node.events(app, from).map {{ EventFrame(it.id, Event.decode(it.kind, it.data)) }}
    }}

    private suspend fun <T> invokeCall(
        method: String,
        args: JsonObject,
        idempotencyKey: String?,
        result: KSerializer<T>,
    ): T {{
        val answer =
            if (client == null) {{
                if (idempotencyKey != null) throw UnsupportedOperationException(\"$method: a contract makes its own idempotency key\")
                json.parseToJsonElement(contract.execute(method, args.toString()))
            }} else {{
                client.apps.method(app, method, args, idempotencyKey = idempotencyKey ?: newKey()).result ?: JsonNull
            }}
        return json.decodeFromJsonElement(result, answer)
    }}

    private suspend fun <T> invokeView(
        method: String,
        args: JsonObject,
        stale: Boolean,
        minIndex: Long,
        result: KSerializer<T>,
    ): T {{
        val answer =
            if (client == null) {{
                json.parseToJsonElement(contract.query(method, args.toString()))
            }} else {{
                val consistency = if (stale) AppMethodRequest.Consistency.STALE else AppMethodRequest.Consistency.STRONG
                client.apps.method(app, method, args, consistency = consistency, minIndex = minIndex).result ?: JsonNull
            }}
        return json.decodeFromJsonElement(result, answer)
    }}

    private suspend fun <T> invokeConfirmed(
        method: String,
        args: JsonObject,
        passkey: PasskeyProvider,
        idempotencyKey: String?,
        result: KSerializer<T>,
    ): T {{
        val node = client ?: throw UnsupportedOperationException(\"$method needs a passkey step-up, so a CalimeroClient\")
        val answer = node.apps.method(app, method, args, idempotencyKey = idempotencyKey ?: newKey(), passkey = passkey).result ?: JsonNull
        return json.decodeFromJsonElement(result, answer)
    }}"
    );
    out.push_str(&nested);
    let _ = writeln!(
        out,
        "
    companion object {{
        private val json = Json {{ ignoreUnknownKeys = true }}

        private fun newKey(): String = java.util.UUID.randomUUID().toString()

        private fun <T> encode(
            serializer: KSerializer<T>,
            value: T,
        ): JsonElement = json.encodeToJsonElement(serializer, value)

        /** Field [name] of an event's or error's [data]; a missing one reads as `null`. */
        private fun <T> field(
            data: JsonElement,
            name: String,
            serializer: KSerializer<T>,
        ): T = json.decodeFromJsonElement(serializer, (data as? JsonObject)?.get(name) ?: JsonNull)"
    );
    for (name, helper) in HELPERS {
        if kt.helpers.contains(name) {
            let _ = writeln!(out, "{helper}");
        }
    }
    let _ = writeln!(out, "    }}\n}}");
    out
}

/// `TupleN` and its serializer: a JSON array of exactly `n` items.
fn tuple(out: &mut String, n: usize) {
    let params: Vec<String> = (0..n).map(|i| format!("T{i}")).collect();
    let generics = params.join(", ");
    let _ = writeln!(
        out,
        "\n    /** `tuple<..>` of {n}: a JSON array `[v0, ..]`. */\n    data class Tuple{n}<{generics}>("
    );
    for (i, p) in params.iter().enumerate() {
        let _ = writeln!(out, "        val v{i}: {p},");
    }
    let _ = writeln!(out, "    )\n");
    let _ = writeln!(out, "    private class Tuple{n}Serializer<{generics}>(");
    for (i, p) in params.iter().enumerate() {
        let _ = writeln!(out, "        private val s{i}: KSerializer<{p}>,");
    }
    let items: Vec<String> = (0..n)
        .map(|i| format!("json.encodeToJsonElement(s{i}, value.v{i})"))
        .collect();
    let decoded: Vec<String> = (0..n)
        .map(|i| format!("json.decodeFromJsonElement(s{i}, items[{i}])"))
        .collect();
    let _ = writeln!(
        out,
        "    ) : KSerializer<Tuple{n}<{generics}>> {{
        override val descriptor: SerialDescriptor = JsonArray.serializer().descriptor

        override fun serialize(
            encoder: Encoder,
            value: Tuple{n}<{generics}>,
        ) = (encoder as JsonEncoder).encodeJsonElement(JsonArray(listOf({})))

        override fun deserialize(decoder: Decoder): Tuple{n}<{generics}> {{
            val items = (decoder as JsonDecoder).decodeJsonElement().jsonArray
            if (items.size != {n}) throw SerializationException(\"expected {n} items, got ${{items.size}}\")
            return Tuple{n}({})
        }}
    }}",
        items.join(", "),
        decoded.join(", ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kt() -> Kt<'static> {
        let abi: &'static AppAbi = Box::leak(Box::new(
            AppAbi::from_json(&serde_json::json!({"abiVersion": 1, "methods": {}})).unwrap(),
        ));
        Kt {
            abi,
            names: Names::new(abi),
            plugin: false,
            described: BTreeMap::new(),
            params: Vec::new(),
            tuples: BTreeSet::new(),
            helpers: BTreeSet::new(),
        }
    }

    #[test]
    fn types_and_their_serializers() {
        let mut kt = kt();
        let cases = [
            ("u64", "ULong", "U64AsString"),
            ("i64", "Long", "I64AsString"),
            ("u32", "UInt", "UInt.serializer()"),
            ("bytes", "ByteArray", "Base64Url"),
            (
                "[u8;32]",
                "List<UByte>",
                "ListSerializer(UByte.serializer())",
            ),
            ("option<u64>", "ULong?", "U64AsString.nullable"),
            (
                "option<option<string>>",
                "String?",
                "String.serializer().nullable",
            ),
            (
                "list<string>",
                "List<String>",
                "ListSerializer(String.serializer())",
            ),
            (
                "BTreeSet<u8>",
                "List<UByte>",
                "ListSerializer(UByte.serializer())",
            ),
            (
                "BTreeMap<String,i64>",
                "Map<String, Long>",
                "MapSerializer(String.serializer(), I64AsString)",
            ),
            (
                "tuple<u32, string>",
                "Tuple2<UInt, String>",
                "Tuple2Serializer(UInt.serializer(), String.serializer())",
            ),
            ("Item", "JsonElement", "JsonElement.serializer()"),
            ("Page<Item>", "JsonElement", "JsonElement.serializer()"),
            ("u64_number", "ULong", "ULong.serializer()"),
            ("option<i64_number>", "Long?", "Long.serializer().nullable"),
            (
                "byte_list",
                "List<UByte>",
                "ListSerializer(UByte.serializer())",
            ),
            ("unit", "JsonElement", "JsonElement.serializer()"),
        ];
        for (abi, ty, ser) in cases {
            let t = Type::parse(abi);
            assert_eq!(kt.ty(&t), ty, "{abi}");
            assert_eq!(kt.ser(&t), ser, "{abi}");
        }
        assert_eq!(kt.tuples, BTreeSet::from([2]));
    }

    #[test]
    fn identifiers_and_literals() {
        assert_eq!(ident("in"), "`in`");
        assert_eq!(ident("0"), "_0");
        assert_eq!(string_literal("a\"$b"), "\"a\\\"\\$b\"");
    }

    #[test]
    fn an_app_type_named_like_a_kotlin_type_is_renamed() {
        let mut kt = kt();
        kt.names.reserve(RESERVED_TYPES.iter().copied());
        let abi = AppAbi::from_json(&serde_json::json!({
            "abiVersion": 1,
            "methods": {},
            "types": { "Flow": { "kind": "unit" }, "String2": { "kind": "unit" } },
        }))
        .unwrap();
        kt.abi = Box::leak(Box::new(abi));
        assert_eq!(kt.ty(&Type::parse("String2")), "String2");
        assert_eq!(kt.ty(&Type::Named("Flow".into())), "FlowType");
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
        let out = module(&abi, "AppClient", "calimero.generated.app", false);
        for expected in [
            r#"        data class NotFound(\n            val id: ULong,\n        ) : Refusal"#,
            r#"        data class Rejected(\n            val _0: String,\n        ) : Refusal"#,
            r#"        data object Unknown : Refusal"#,
            r#"        data class Unknown_("#,
            r#""Default" -> Default(blob = field(data, "blob", Base64Url.nullable))"#,
            r#"fun appEvents(from: Long = 0)"#,
            r#"    suspend fun events(\n        `in`: String,"#,
        ] {
            let expected = expected.replace("\\n", "\n");
            assert!(out.contains(&expected), "{expected} not in\n{out}");
        }
    }

    /// A described type is a nested data class or sealed interface with its own serializer
    /// (generic where the Rust type is); a variant class does not shadow a type it names; a type
    /// without a shape stays `JsonElement`.
    #[test]
    fn described_types_are_nested_types() {
        let abi = AppAbi::from_json(&serde_json::json!({
            "abiVersion": 1,
            "methods": { "get": { "kind": "view", "params": [["page", "Page<Item>"], ["raw", "Raw"]], "returns": "Either" } },
            "types": {
                "Item": { "kind": "record", "fields": [["id", "u64"], ["raw", "Raw"]] },
                "Page": { "kind": "record", "params": ["T"], "fields": [["items", "list<T>"]] },
                "Either": { "kind": "enum", "untagged": true, "variants": [{ "name": "Item", "type": "Item" }, { "name": "String", "type": "string" }] },
            },
        }))
        .unwrap();
        let out = module(&abi, "AppClient", "calimero.generated.app", false);
        for expected in [
            "    @Serializable(with = Item.Companion.Wire::class)\n    data class Item(\n        val id: ULong,\n        val raw: JsonElement,\n    ) {",
            "    @Serializable(with = Page.Companion.Wire::class)\n    data class Page<T>(",
            "    @Serializable(with = Either.Companion.Wire::class)\n    sealed interface Either {",
            "internal object Wire : KSerializer<Item>",
            "internal class Wire<T>(",
            r#"put("raw", encode(JsonElement.serializer(), value.raw))"#,
            "    data class Page<T>(",
            "fun <T> serializer(tSerializer: KSerializer<T>): KSerializer<Page<T>> = Wire(tSerializer)",
            "        data class Item_(\n            val value: Item,\n        ) : Either",
            "        data class String_(\n            val value: String,\n        ) : Either",
            r#"put("page", encode(Page.serializer(Item.serializer()), page))"#,
            "private fun <T> attempt(",
        ] {
            assert!(out.contains(expected), "{expected} not in\n{out}");
        }
        assert!(!out.contains("private fun external("), "{out}");
        // No top-level alias that an app type of the same name in the package would clash with.
        assert!(!out.contains("typealias"), "{out}");
    }

    /// An optional argument left `null` is not sent; a required one always is.
    #[test]
    fn unset_optional_arguments_are_left_out() {
        let abi = AppAbi::from_json(&serde_json::json!({
            "abiVersion": 1,
            "methods": { "set": { "kind": "call", "params": [["id", "u64"], ["note", "option<string>"]] } },
        }))
        .unwrap();
        let out = module(&abi, "AppClient", "calimero.generated.app", false);
        assert!(
            out.contains(r#"put("id", encode(U64AsString, id))"#),
            "{out}"
        );
        assert!(
            out.contains(
                r#"if (note != null) put("note", encode(String.serializer().nullable, note))"#
            ),
            "{out}"
        );
    }
}
