//! The Kotlin client over kotlin-sdk's mero-core: one `suspend fun` per method, sent through
//! `CalimeroClient.apps.method` or a `ContextContractCalling`, the app's events and errors as
//! sealed interfaces, every value through a kotlinx.serialization serializer that writes the
//! ABI's JSON form (u64/i64 decimal strings, bytes base64url).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::abi::{AppAbi, Fields};
use crate::openapi::{all_events, pascal, Names};
use crate::ty::{Int, Type};

use super::ident::{sanitize, Scope};

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
/// one is renamed, since its alias would shadow it in the package.
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
    "KSerializer",
    "List",
    "ListSerializer",
    "Long",
    "Map",
    "MapSerializer",
    "MeroAppBackend",
    "PasskeyProvider",
    "PrimitiveKind",
    "PrimitiveSerialDescriptor",
    "Refusal",
    "SerialDescriptor",
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

struct Kt {
    names: Names,
    /// App types met so far: Kotlin name to ABI text.
    named: BTreeMap<String, String>,
    /// The tuple arities used.
    tuples: BTreeSet<usize>,
}

fn int_type(int: Int) -> &'static str {
    match int {
        Int::U8 => "UByte",
        Int::U16 => "UShort",
        Int::U32 | Int::Usize => "UInt",
        Int::U64 | Int::U128 => "ULong",
        Int::I8 => "Byte",
        Int::I16 => "Short",
        Int::I32 | Int::Isize => "Int",
        Int::I64 | Int::I128 => "Long",
    }
}

impl Kt {
    fn named(&mut self, text: &str) -> String {
        let (name, _) = self.names.named(text);
        let _ = self.named.insert(name.clone(), text.to_owned());
        name
    }

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
            // ABI version 1 names an app type without its fields: its alias is `JsonElement`.
            Type::Named(text) => self.named(text),
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
            Type::Named(text) => format!("serializer<{}>()", self.named(text)),
        }
    }
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

impl Kt {
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

pub(super) fn module(abi: &AppAbi, class: &str, package: &str) -> String {
    let mut names = Names::new(abi);
    names.reserve(RESERVED_TYPES.iter().copied());
    names.reserve([class]);
    let tuple_names: Vec<String> = (1..=16)
        .flat_map(|n| [format!("Tuple{n}"), format!("Tuple{n}Serializer")])
        .collect();
    names.reserve(tuple_names.iter().map(String::as_str));
    let mut kt = Kt {
        names,
        named: BTreeMap::new(),
        tuples: BTreeSet::new(),
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
            puts.push(format!(
                "put({}, encode({}, {arg}))",
                string_literal(p),
                kt.ser(t)
            ));
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
    for (name, text) in &kt.named {
        let _ = writeln!(
            out,
            "/** The app type `{text}`: this client does not read its shape (the ABI's `types`), so it is its JSON. */\ntypealias {name} = JsonElement\n"
        );
    }
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
        ): T = json.decodeFromJsonElement(serializer, (data as? JsonObject)?.get(name) ?: JsonNull)
    }}
}}"
    );
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

    fn kt() -> Kt {
        Kt {
            names: Names::new(
                &AppAbi::from_json(&serde_json::json!({"abiVersion": 1, "methods": {}})).unwrap(),
            ),
            named: BTreeMap::new(),
            tuples: BTreeSet::new(),
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
            ("Item", "Item", "serializer<Item>()"),
            ("Page<Item>", "Page_Item", "serializer<Page_Item>()"),
            ("unit", "JsonElement", "JsonElement.serializer()"),
        ];
        for (abi, ty, ser) in cases {
            let t = Type::parse(abi);
            assert_eq!(kt.ty(&t), ty, "{abi}");
            assert_eq!(kt.ser(&t), ser, "{abi}");
        }
        assert_eq!(kt.tuples, BTreeSet::from([2]));
        assert_eq!(kt.named.keys().collect::<Vec<_>>(), ["Item", "Page_Item"]);
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
        let out = module(&abi, "AppClient", "calimero.generated.app");
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
}
