//! The document's schemas as Rust types: `types.rs` of the generated client.
//!
//! A component is a type of its own name; a schema written inline (a request
//! body, an answer, a nested object or enum) is named after where it is
//! (`{Operation}Request`, `{Operation}Response`, then each property's name).
//! Shapes:
//!
//! - an object with properties is a struct; one that may carry other fields
//!   (an answer, or one the document leaves open) keeps them in `extra`, so
//!   what the node sends survives a round trip;
//! - an object of one value type is a `BTreeMap<String, T>`, any other object
//!   a `Map<String, Value>`, a schema that says nothing a `Value`;
//! - a string enum is an enum with `Other(String)` for values this client does
//!   not know yet; a `oneOf`/`anyOf` whose objects share a constant property
//!   (`kind`) is an internally tagged enum, any other an untagged one;
//! - `type: [T, "null"]` (or `anyOf` with `{type: null}`) is `Option<T>`; a
//!   property that may be absent is `Option<T>` too, and one that may be
//!   absent or `null` is `Option<Option<T>>` (absent, `null`, a value);
//! - `type: null` alone (an answer that is only `null`) is `()`;
//! - integers are `format: int64`: `u64` with `minimum: 0`, else `i64`.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use eyre::{bail, eyre, Result as EyreResult};
use serde_json::Value;

use crate::naming::{ident, pascal, snake};

/// Components the client already has a type for: the generated code uses that
/// type (`pub use`), so what the hand-written flows check (a receipt, an
/// attestation) is the very value the routes answer.
/// The last column: whether that type is `PartialEq`, which a generated type
/// holding it derives only then.
const REPLACED: &[(&str, &str, bool)] = &[
    ("Assertion", "calimero_identity::Assertion", false),
    ("AttestationDoc", "crate::attest::AttestationDoc", false),
    ("Governance", "crate::attest::Governance", true),
    ("Receipt", "crate::attest::Receipt", true),
    ("Registration", "calimero_identity::Registration", false),
    ("Release", "crate::attest::Release", false),
    ("Session", "crate::Session", false),
];

/// Components not generated: every refusal is `crate::Error`.
const SKIPPED: &[&str] = &["Error"];

/// How the document says an object takes other properties than those it
/// lists (it leaves out `additionalProperties`, which JSON Schema admits).
const OTHER_PROPERTIES: &str = "Other properties than those listed may be present";

/// A Rust type.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Value,
    String,
    U64,
    I64,
    F64,
    Bool,
    Vec(Box<Ty>),
    Map(Box<Ty>),
    Object,
    Named(String),
    Nullable(Box<Ty>),
    Bytes,
    /// `type: null`: the value is always `null`.
    Unit,
}

impl Ty {
    pub fn rust(&self) -> String {
        match self {
            Self::Value => "Value".into(),
            Self::String => "String".into(),
            Self::U64 => "u64".into(),
            Self::I64 => "i64".into(),
            Self::F64 => "f64".into(),
            Self::Bool => "bool".into(),
            Self::Vec(t) => format!("Vec<{}>", t.rust()),
            Self::Map(t) => format!("BTreeMap<String, {}>", t.rust()),
            Self::Object => "Map<String, Value>".into(),
            Self::Named(n) => n.clone(),
            Self::Nullable(t) => format!("Option<{}>", t.rust()),
            Self::Bytes => "Vec<u8>".into(),
            Self::Unit => "()".into(),
        }
    }
}

/// One property of an object.
#[derive(Clone, Debug)]
pub struct Field {
    pub wire: String,
    pub ty: Ty,
    pub required: bool,
    pub doc: Option<String>,
}

#[derive(Clone, Debug)]
enum Variant {
    Newtype(Ty),
    Struct(Vec<Field>),
}

#[derive(Clone, Debug)]
enum Def {
    Struct {
        fields: Vec<Field>,
        /// Other fields may come: kept in `extra`.
        open: bool,
        /// `additionalProperties: false`: the node refuses any other property.
        closed: bool,
    },
    /// A string enum, open to values this client does not know.
    Enum(Vec<String>),
    /// One string, exactly: a variant of an untagged enum.
    Const(String),
    Tagged {
        tag: String,
        variants: Vec<(String, Vec<Field>)>,
    },
    Untagged(Vec<(String, Variant)>),
    Alias(Ty),
    /// A type the client has (its path), and whether it is `PartialEq`.
    Use(String, bool),
}

#[derive(Clone, Debug)]
struct TypeDef {
    name: String,
    doc: Option<String>,
    def: Def,
}

/// Every type the generated client names.
pub struct Types<'d> {
    doc: &'d Value,
    defs: Vec<TypeDef>,
    names: BTreeSet<String>,
    /// Types answers reach: their structs keep fields this client does not name.
    answered: BTreeSet<String>,
    /// Types a refusal's `details` reach (`x-calimero-error-details`): read
    /// from the node, so never closed to fields it adds.
    refused: BTreeSet<String>,
}

/// A schema's `description`, as doc-comment paragraphs: lines joined, list
/// items one per line, no indentation that would read as a code block.
pub fn doc_text(text: &str) -> String {
    let mut paragraphs = Vec::new();
    for para in text.split("\n\n") {
        let mut items: Vec<String> = Vec::new();
        for line in para.lines().map(str::trim).filter(|l| !l.is_empty()) {
            match items.last_mut() {
                Some(last) if !line.starts_with("- ") => {
                    last.push(' ');
                    last.push_str(line);
                }
                _ => items.push(line.to_owned()),
            }
        }
        if !items.is_empty() {
            paragraphs.push(items.join("\n"));
        }
    }
    paragraphs.join("\n\n")
}

/// `text` as `///` lines at `indent`.
pub fn doc_comment(out: &mut String, indent: &str, text: &str) {
    for line in text.lines() {
        if line.is_empty() {
            let _ = writeln!(out, "{indent}///");
        } else {
            let _ = writeln!(out, "{indent}/// {line}");
        }
    }
}

fn description(schema: &Value) -> Option<String> {
    schema["description"]
        .as_str()
        .map(doc_text)
        .filter(|d| !d.is_empty())
}

impl<'d> Types<'d> {
    /// The components' types (all but [`SKIPPED`]), in the document's order.
    pub fn new(doc: &'d Value) -> EyreResult<Self> {
        let mut types = Self {
            doc,
            defs: Vec::new(),
            names: BTreeSet::new(),
            answered: BTreeSet::new(),
            refused: BTreeSet::new(),
        };
        let components = doc["components"]["schemas"]
            .as_object()
            .ok_or_else(|| eyre!("the document has no components"))?;
        for (name, schema) in components {
            if SKIPPED.contains(&name.as_str()) {
                continue;
            }
            if let Some((_, path, eq)) = REPLACED.iter().find(|(n, ..)| n == name) {
                types.add(name, None, Def::Use((*path).to_owned(), *eq))?;
                continue;
            }
            match types.resolve(schema, name)? {
                Ty::Named(n) if n == *name => {}
                ty => types.add(name, description(schema), Def::Alias(ty))?,
            }
        }
        let details = doc["x-calimero-error-details"].as_object();
        for r in details.into_iter().flat_map(|m| m.values()) {
            let Some(r) = r["$ref"].as_str() else {
                bail!("x-calimero-error-details: a schema other than a $ref");
            };
            let ty = Ty::Named(ref_name(r).to_owned());
            let refused = std::mem::take(&mut types.refused);
            types.refused = types.reach(&ty, refused);
        }
        Ok(types)
    }

    fn add(&mut self, name: &str, doc: Option<String>, def: Def) -> EyreResult<()> {
        if !self.names.insert(name.to_owned()) {
            bail!("two types named {name}");
        }
        self.defs.push(TypeDef {
            name: name.to_owned(),
            doc,
            def,
        });
        Ok(())
    }

    fn deref<'s>(&'s self, schema: &'s Value) -> &'s Value {
        let mut s = schema;
        while let Some(r) = s["$ref"].as_str() {
            s = &self.doc["components"]["schemas"][ref_name(r)];
        }
        s
    }

    /// `schema` after its `$ref`s.
    pub fn resolved<'s>(&'s self, schema: &'s Value) -> &'s Value {
        self.deref(schema)
    }

    /// Mark `ty` (and every type it reaches) as part of an answer.
    pub fn answer(&mut self, ty: &Ty) {
        let answered = std::mem::take(&mut self.answered);
        self.answered = self.reach(ty, answered);
    }

    /// `set` with `ty` and every type it reaches added.
    fn reach(&self, ty: &Ty, mut set: BTreeSet<String>) -> BTreeSet<String> {
        match ty {
            Ty::Vec(t) | Ty::Map(t) | Ty::Nullable(t) => self.reach(t, set),
            Ty::Named(n) => {
                if !set.insert(n.clone()) {
                    return set;
                }
                let Some(def) = self.defs.iter().find(|d| d.name == *n) else {
                    return set;
                };
                for t in reached(&def.def) {
                    set = self.reach(&t, set);
                }
                set
            }
            _ => set,
        }
    }

    /// The Rust type of `schema`, defining the types it needs under `name`.
    pub fn resolve(&mut self, schema: &Value, name: &str) -> EyreResult<Ty> {
        if let Some(r) = schema["$ref"].as_str() {
            return Ok(Ty::Named(ref_name(r).to_owned()));
        }
        if let Some(Value::Array(types)) = schema.get("type") {
            let rest: Vec<&Value> = types.iter().filter(|t| *t != "null").collect();
            let [one] = rest.as_slice() else {
                bail!("{name}: a type list other than [T, \"null\"]");
            };
            let mut single = schema.clone();
            single["type"] = (*one).clone();
            let ty = self.resolve(&single, name)?;
            return Ok(if rest.len() < types.len() {
                Ty::Nullable(Box::new(ty))
            } else {
                ty
            });
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(Value::Array(variants)) = schema.get(key) {
                return self.union(schema, variants, name);
            }
        }
        if schema.get("allOf").is_some() {
            bail!("{name}: allOf is not supported");
        }
        let doc = description(schema);
        if let Some(Value::Array(values)) = schema.get("enum") {
            let values: Vec<String> = values
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| eyre!("{name}: an enum of other than strings"))
                })
                .collect::<EyreResult<_>>()?;
            self.add(name, doc, Def::Enum(values))?;
            return Ok(Ty::Named(name.to_owned()));
        }
        Ok(match schema["type"].as_str() {
            Some("string") if schema["format"] == "binary" => Ty::Bytes,
            Some("string") => Ty::String,
            Some("integer") if schema["minimum"].as_i64().is_some_and(|m| m >= 0) => Ty::U64,
            Some("integer") => Ty::I64,
            Some("number") => Ty::F64,
            Some("boolean") => Ty::Bool,
            Some("null") => Ty::Unit,
            Some("array") => {
                let items = self.resolve(&schema["items"], &format!("{name}Item"))?;
                Ty::Vec(Box::new(items))
            }
            Some("object") | None if schema.get("properties").is_some() => {
                let fields = self.fields(schema, name)?;
                let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
                let open = match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => false,
                    Some(_) => true,
                    None => schema["description"]
                        .as_str()
                        .is_some_and(|d| d.contains(OTHER_PROPERTIES)),
                };
                self.add(
                    name,
                    doc,
                    Def::Struct {
                        fields,
                        open,
                        closed,
                    },
                )?;
                Ty::Named(name.to_owned())
            }
            Some("object") => match schema.get("additionalProperties") {
                Some(ap @ Value::Object(o)) if !o.is_empty() => {
                    Ty::Map(Box::new(self.resolve(ap, &format!("{name}Value"))?))
                }
                _ => Ty::Object,
            },
            None => Ty::Value,
            Some(other) => bail!("{name}: type {other} is not supported"),
        })
    }

    fn fields(&mut self, schema: &Value, name: &str) -> EyreResult<Vec<Field>> {
        let required: BTreeSet<&str> = schema["required"]
            .as_array()
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut out = Vec::new();
        for (wire, prop) in schema["properties"].as_object().into_iter().flatten() {
            let ty = self.resolve(prop, &format!("{name}{}", pascal(wire)))?;
            out.push(Field {
                wire: wire.clone(),
                ty,
                required: required.contains(wire.as_str()),
                doc: description(prop),
            });
        }
        Ok(out)
    }

    fn union(&mut self, schema: &Value, variants: &[Value], name: &str) -> EyreResult<Ty> {
        let rest: Vec<&Value> = variants.iter().filter(|v| v["type"] != "null").collect();
        let nullable = rest.len() < variants.len();
        let wrap = |t: Ty| {
            if nullable {
                Ty::Nullable(Box::new(t))
            } else {
                t
            }
        };
        if let [one] = rest.as_slice() {
            return Ok(wrap(self.resolve(one, name)?));
        }
        let doc = description(schema);
        if let Some(tag) = self.tag_of(schema, &rest) {
            let mut out = Vec::new();
            for v in &rest {
                let v = self.deref(v).clone();
                let wire = v["properties"][&tag]["enum"][0]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let mut body = v.clone();
                if let Some(props) = body["properties"].as_object_mut() {
                    let _tag = props.remove(&tag);
                }
                let fields = self.fields(&body, &format!("{name}{}", pascal(&wire)))?;
                out.push((wire, fields));
            }
            self.add(name, doc, Def::Tagged { tag, variants: out })?;
            return Ok(wrap(Ty::Named(name.to_owned())));
        }
        let objects: Vec<BTreeSet<String>> = rest
            .iter()
            .map(|v| self.deref(v))
            .filter(|v| v.get("properties").is_some())
            .map(|v| {
                v["required"]
                    .as_array()
                    .map(|r| {
                        r.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        let mut out: Vec<(String, Variant)> = Vec::new();
        for v in &rest {
            let (vname, variant) = if let Some(r) = v["$ref"].as_str() {
                let n = ref_name(r).to_owned();
                (n.clone(), Variant::Newtype(Ty::Named(n)))
            } else if let (Some("string"), Some([Value::String(c)])) =
                (v["type"].as_str(), v["enum"].as_array().map(Vec::as_slice))
            {
                let tname = format!("{name}{}", pascal(c));
                self.add(&tname, None, Def::Const(c.clone()))?;
                (pascal(c), Variant::Newtype(Ty::Named(tname)))
            } else if v.get("properties").is_some() {
                let req: BTreeSet<String> = v["required"]
                    .as_array()
                    .map(|r| {
                        r.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let own = req
                    .iter()
                    .find(|p| objects.iter().filter(|o| o.contains(*p)).count() == 1)
                    .ok_or_else(|| eyre!("{name}: object variants no property tells apart"))?;
                let vname = pascal(own);
                let fields = self.fields(v, &format!("{name}{vname}"))?;
                (vname, Variant::Struct(fields))
            } else {
                let vname = match v["type"].as_str() {
                    Some("string") => "Text",
                    Some("integer") => "Integer",
                    Some("number") => "Number",
                    Some("boolean") => "Bool",
                    Some("array") => "List",
                    _ => "Object",
                };
                let ty = self.resolve(v, &format!("{name}{vname}"))?;
                (vname.to_owned(), Variant::Newtype(ty))
            };
            if out.iter().any(|(n, _)| *n == vname) {
                bail!("{name}: two variants named {vname}");
            }
            out.push((vname, variant));
        }
        self.add(name, doc, Def::Untagged(out))?;
        Ok(wrap(Ty::Named(name.to_owned())))
    }

    /// The property that tells a union's object variants apart: its
    /// `discriminator`, else one every variant requires as a single constant.
    fn tag_of(&self, schema: &Value, variants: &[&Value]) -> Option<String> {
        if let Some(t) = schema["discriminator"]["propertyName"].as_str() {
            return Some(t.to_owned());
        }
        let first = self.deref(variants.first()?);
        let candidates = first["required"].as_array()?;
        candidates
            .iter()
            .filter_map(Value::as_str)
            .find(|p| {
                variants.iter().all(|v| {
                    let v = self.deref(v);
                    v["required"]
                        .as_array()
                        .is_some_and(|r| r.iter().any(|x| x == *p))
                        && v["properties"][*p]["enum"]
                            .as_array()
                            .is_some_and(|e| e.len() == 1)
                })
            })
            .map(str::to_owned)
    }

    fn def(&self, name: &str) -> Option<&Def> {
        self.defs.iter().find(|d| d.name == name).map(|d| &d.def)
    }

    /// Whether `ty` has a `Default` (every struct whose fields all do derives one).
    fn has_default(&self, ty: &Ty, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            Ty::Named(n) => {
                if !seen.insert(n.clone()) {
                    return false;
                }
                match self.def(n) {
                    Some(Def::Struct { fields, .. }) => fields
                        .iter()
                        .all(|f| !f.required || self.has_default(&f.ty, seen)),
                    Some(Def::Alias(t)) => self.has_default(t, seen),
                    _ => false,
                }
            }
            _ => true,
        }
    }

    /// Whether `ty` is `PartialEq`: every generated type is, unless it holds
    /// a client type that is not.
    fn has_eq(&self, ty: &Ty, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            Ty::Vec(t) | Ty::Map(t) | Ty::Nullable(t) => self.has_eq(t, seen),
            Ty::Named(n) => {
                if !seen.insert(n.clone()) {
                    return true;
                }
                match self.def(n) {
                    Some(Def::Use(_, eq)) => *eq,
                    Some(def) => reached(def).iter().all(|t| self.has_eq(t, seen)),
                    None => true,
                }
            }
            _ => true,
        }
    }

    fn derives(&self, name: &str, default: bool) -> String {
        let eq = self.has_eq(&Ty::Named(name.to_owned()), &mut BTreeSet::new());
        format!(
            "#[derive(Clone, Debug{}{}, Serialize, Deserialize)]",
            if default { ", Default" } else { "" },
            if eq { ", PartialEq" } else { "" }
        )
    }

    /// `types.rs`.
    pub fn emit(&self) -> String {
        let mut body = String::new();
        for d in &self.defs {
            self.emit_def(&mut body, d);
        }
        let mut out = String::from(
            "//! The node API's types, generated by `calimero-client-gen` from\n\
             //! `docs/public/openapi.json`: do not edit. `cargo run -p calimero-client-gen`\n\
             //! regenerates them; its test fails while they differ from the document.\n\
             //!\n\
             //! Components keep their names; a type written inline in the document is\n\
             //! named after where it is (`{Operation}Request`, `{Operation}Response`, then\n\
             //! each property). A struct that answers carry keeps the fields this client\n\
             //! does not name in `extra`; one only requests carry that the document closes\n\
             //! (`additionalProperties: false`) refuses such a field when read, as the node\n\
             //! would. A string enum keeps a value it does not know in `Other`. A property\n\
             //! that may be absent or `null` is `Option<Option<T>>`.\n\n",
        );
        if uses_word(&body, "BTreeMap") {
            out.push_str("use std::collections::BTreeMap;\n\n");
        }
        let mut serde = vec!["Deserialize", "Serialize"];
        if body.contains("Deserializer<'de>") {
            serde = vec!["Deserialize", "Deserializer", "Serialize", "Serializer"];
        }
        let _ = writeln!(out, "use serde::{{{}}};", serde.join(", "));
        let json: Vec<&str> = ["Map", "Value"]
            .into_iter()
            .filter(|t| uses_word(&body, t))
            .collect();
        if !json.is_empty() {
            let _ = writeln!(out, "use serde_json::{{{}}};", json.join(", "));
        }
        out.push('\n');
        out.push_str(&body);
        out
    }

    fn emit_def(&self, out: &mut String, d: &TypeDef) {
        let name = &d.name;
        if let Def::Use(path, _) = &d.def {
            let _ = writeln!(out, "pub use {path};\n");
            return;
        }
        if let Some(doc) = &d.doc {
            doc_comment(out, "", doc);
        }
        match &d.def {
            Def::Struct {
                fields,
                open,
                closed,
            } => {
                let extra = *open || self.answered.contains(name);
                let default = self.has_default(&Ty::Named(name.clone()), &mut BTreeSet::new());
                let _ = writeln!(out, "{}", self.derives(name, default));
                // A closed object only ever sent (no answer or refusal reads
                // it) refuses a property it does not list before it is sent,
                // as the node would; what the node sends stays open to fields
                // it adds later.
                if *closed && !extra && !self.refused.contains(name) {
                    out.push_str("#[serde(deny_unknown_fields)]\n");
                }
                let _ = writeln!(out, "pub struct {name} {{");
                for f in fields {
                    emit_field(out, "    ", "pub ", f);
                }
                if extra {
                    out.push_str(
                        "    /// The fields this client does not name, as the node sent them.\n    \
                         #[serde(flatten)]\n    pub extra: Map<String, Value>,\n",
                    );
                }
                out.push_str("}\n\n");
            }
            Def::Enum(values) => emit_enum(out, name, values),
            Def::Const(value) => {
                let _ = writeln!(
                    out,
                    "#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]\n\
                     pub enum {name} {{\n    #[serde(rename = {value:?})]\n    {},\n}}\n",
                    pascal(value)
                );
            }
            Def::Tagged { tag, variants } => {
                let _ = writeln!(
                    out,
                    "{}\n#[serde(tag = {tag:?})]\npub enum {name} {{",
                    self.derives(name, false)
                );
                for (wire, fields) in variants {
                    let _ = writeln!(out, "    #[serde(rename = {wire:?})]");
                    if fields.is_empty() {
                        let _ = writeln!(out, "    {},", pascal(wire));
                    } else {
                        let _ = writeln!(out, "    {} {{", pascal(wire));
                        for f in fields {
                            emit_field(out, "        ", "", f);
                        }
                        out.push_str("    },\n");
                    }
                }
                out.push_str("}\n\n");
            }
            Def::Untagged(variants) => {
                let _ = writeln!(
                    out,
                    "{}\n#[serde(untagged)]\n#[allow(clippy::large_enum_variant, reason = \"an answer, read once\")]\npub enum {name} {{",
                    self.derives(name, false)
                );
                for (vname, v) in variants {
                    match v {
                        Variant::Newtype(t) => {
                            let _ = writeln!(out, "    {vname}({}),", t.rust());
                        }
                        Variant::Struct(fields) => {
                            let _ = writeln!(out, "    {vname} {{");
                            for f in fields {
                                emit_field(out, "        ", "", f);
                            }
                            out.push_str("    },\n");
                        }
                    }
                }
                out.push_str("}\n\n");
            }
            Def::Alias(t) => {
                let _ = writeln!(out, "pub type {name} = {};\n", t.rust());
            }
            Def::Use(..) => {}
        }
    }
}

/// The types `def` names directly.
fn reached(def: &Def) -> Vec<Ty> {
    match def {
        Def::Struct { fields, .. } => fields.iter().map(|f| f.ty.clone()).collect(),
        Def::Tagged { variants, .. } => variants
            .iter()
            .flat_map(|(_, f)| f.iter().map(|f| f.ty.clone()))
            .collect(),
        Def::Untagged(variants) => variants
            .iter()
            .flat_map(|(_, v)| match v {
                Variant::Newtype(t) => vec![t.clone()],
                Variant::Struct(f) => f.iter().map(|f| f.ty.clone()).collect(),
            })
            .collect(),
        Def::Alias(t) => vec![t.clone()],
        Def::Enum(_) | Def::Const(_) | Def::Use(..) => Vec::new(),
    }
}

/// Whether `code` names `word` as an identifier of its own.
pub fn uses_word(code: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    code.match_indices(word).any(|(i, _)| {
        !code[..i].ends_with(is_ident) && !code[i + word.len()..].starts_with(is_ident)
    })
}

fn ref_name(r: &str) -> &str {
    r.rsplit('/').next().unwrap_or(r)
}

/// A property as a struct field: `Option<T>` when it may be absent,
/// `Option<Option<T>>` when it may also be `null`, a `Value` that may be
/// absent keeping a `null` it was sent (`crate::route::present`).
fn emit_field(out: &mut String, indent: &str, vis: &str, f: &Field) {
    if let Some(doc) = &f.doc {
        doc_comment(out, indent, doc);
    }
    let rust = ident(&f.wire);
    if snake(&f.wire) != f.wire {
        let _ = writeln!(out, "{indent}#[serde(rename = {:?})]", f.wire);
    }
    let ty = match (&f.ty, f.required) {
        (t, true) => t.rust(),
        (t @ (Ty::Nullable(_) | Ty::Value), false) => {
            let _ = writeln!(
                out,
                "{indent}#[serde(default, skip_serializing_if = \"Option::is_none\", with = \"crate::route::present\")]"
            );
            format!("Option<{}>", t.rust())
        }
        (t, false) => {
            let _ = writeln!(
                out,
                "{indent}#[serde(default, skip_serializing_if = \"Option::is_none\")]"
            );
            format!("Option<{}>", t.rust())
        }
    };
    let _ = writeln!(out, "{indent}{vis}{rust}: {ty},");
}

/// A string enum, open: a value this client does not know is `Other`.
fn emit_enum(out: &mut String, name: &str, values: &[String]) {
    let _ = writeln!(
        out,
        "#[derive(Clone, Debug, PartialEq, Eq, Hash)]\npub enum {name} {{"
    );
    for v in values {
        let _ = writeln!(out, "    /// `{v}`.\n    {},", pascal(v));
    }
    let _ = writeln!(
        out,
        "    /// A value this client does not know yet.\n    Other(String),\n}}\n"
    );
    let _ = writeln!(
        out,
        "impl {name} {{\n    /// The value on the wire.\n    #[must_use]\n    pub fn as_str(&self) -> &str {{\n        match self {{"
    );
    for v in values {
        let _ = writeln!(out, "            Self::{} => {v:?},", pascal(v));
    }
    let _ = writeln!(
        out,
        "            Self::Other(s) => s,\n        }}\n    }}\n}}\n\n\
         impl Serialize for {name} {{\n    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {{\n        \
         s.serialize_str(self.as_str())\n    }}\n}}\n\n\
         impl<'de> Deserialize<'de> for {name} {{\n    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {{\n        \
         Ok(match String::deserialize(d)?.as_str() {{"
    );
    for v in values {
        let _ = writeln!(out, "            {v:?} => Self::{},", pascal(v));
    }
    let _ = writeln!(
        out,
        "            other => Self::Other(other.to_owned()),\n        }})\n    }}\n}}\n"
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Ty, Types};

    /// The emitted definition of `name`, from its derive to its closing brace.
    fn def_of<'a>(code: &'a str, name: &str) -> &'a str {
        let end = code
            .find(&format!("pub struct {name} {{"))
            .expect("the struct is emitted");
        let start = code[..end].rfind("#[derive").expect("a derive");
        let close = code[end..].find("\n}\n").expect("a closing brace");
        &code[start..end + close]
    }

    /// `additionalProperties: false` closes a body that is only sent; what the
    /// node sends (an answer, a refusal's `details`) stays open to fields it adds.
    #[test]
    fn only_what_is_sent_refuses_unknown_fields() {
        let closed = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "a": { "type": "string" } }
        });
        let doc = json!({
            "x-calimero-error-details": {
                "SOME_CODE": { "$ref": "#/components/schemas/Detail" }
            },
            "components": { "schemas": {
                "Sent": closed,
                "Answer": closed,
                "Detail": closed,
                "Unsaid": {
                    "type": "object",
                    "properties": { "a": { "type": "string" } }
                }
            } }
        });
        let mut types = Types::new(&doc).expect("the types");
        types.answer(&Ty::Named("Answer".into()));
        let code = types.emit();
        assert!(
            def_of(&code, "Sent").contains("#[serde(deny_unknown_fields)]"),
            "{code}"
        );
        for open in ["Answer", "Detail", "Unsaid"] {
            assert!(
                !def_of(&code, open).contains("deny_unknown_fields"),
                "{open}: {code}"
            );
        }
    }
}
