//! The registry's notation for a JSON value's shape, and what it becomes: a JSON Schema
//! (2020-12, as OpenAPI 3.1 uses) and a sample value the request checks deserialize.
//!
//! ```text
//! shape   := alt ('|' alt)*                       one of these
//! alt     := atom '[]'*                           '[]': a list of it
//! atom    := 'string' | 'int' | 'uint' | 'number' | 'bool' | 'null' | 'any'
//!          | 'u64'                                a decimal string (D20)
//!          | 'b64url'                             base64url without padding
//!          | '"text"'                             this string
//!          | '#Name'                              components/schemas/Name
//!          | '{' (field (',' field)* ','?)? '}'   an object
//!          | '(' shape ')'
//! field   := name '?'? ':' shape                   '?': may be absent
//!          | '[' 'string' ']' ':' shape            any other property: a map
//! ```

use serde_json::{json, Map, Value};

/// A parsed shape.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Shape {
    String,
    Int,
    Uint,
    Number,
    Bool,
    Null,
    Any,
    U64,
    B64url,
    Lit(String),
    Ref(String),
    List(Box<Shape>),
    /// An object: its fields, and the shape of any other property (`[string]: shape`).
    Object(Vec<Field>, Option<Box<Shape>>),
    OneOf(Vec<Shape>),
}

/// One property of an object.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Field {
    pub name: String,
    pub optional: bool,
    pub shape: Shape,
}

/// Parses `text`.
///
/// # Panics
///
/// On a malformed shape: the registry is source code, and a test builds every shape.
pub(super) fn parse(text: &str) -> Shape {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    let shape = p.shape();
    p.ws();
    assert!(
        p.i == p.s.len(),
        "shape {text:?}: unexpected text at {}",
        p.i
    );
    shape
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        let hit = self.s.get(self.i) == Some(&c);
        if hit {
            self.i += 1;
        }
        hit
    }

    fn expect(&mut self, c: u8) {
        assert!(
            self.eat(c),
            "shape {:?}: expected {:?} at {}",
            String::from_utf8_lossy(self.s),
            c as char,
            self.i
        );
    }

    fn word(&mut self) -> String {
        self.ws();
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    fn shape(&mut self) -> Shape {
        let mut alts = vec![self.alt()];
        while self.eat(b'|') {
            alts.push(self.alt());
        }
        if alts.len() == 1 {
            alts.remove(0)
        } else {
            Shape::OneOf(alts)
        }
    }

    fn alt(&mut self) -> Shape {
        let mut shape = self.atom();
        loop {
            self.ws();
            if self.s[self.i..].starts_with(b"[]") {
                self.i += 2;
                shape = Shape::List(Box::new(shape));
            } else {
                return shape;
            }
        }
    }

    fn atom(&mut self) -> Shape {
        if self.eat(b'(') {
            let inner = self.shape();
            self.expect(b')');
            return inner;
        }
        if self.eat(b'"') {
            let start = self.i;
            while self.s[self.i] != b'"' {
                self.i += 1;
            }
            let lit = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
            self.i += 1;
            return Shape::Lit(lit);
        }
        if self.eat(b'#') {
            return Shape::Ref(self.word());
        }
        if self.eat(b'{') {
            return self.object();
        }
        let word = self.word();
        match word.as_str() {
            "string" => Shape::String,
            "int" => Shape::Int,
            "uint" => Shape::Uint,
            "number" => Shape::Number,
            "bool" => Shape::Bool,
            "null" => Shape::Null,
            "any" => Shape::Any,
            "u64" => Shape::U64,
            "b64url" => Shape::B64url,
            other => panic!(
                "shape {:?}: unknown type {other:?}",
                String::from_utf8_lossy(self.s)
            ),
        }
    }

    fn object(&mut self) -> Shape {
        let mut fields = Vec::new();
        let mut rest = None;
        loop {
            if self.eat(b'}') {
                return Shape::Object(fields, rest);
            }
            if self.eat(b'[') {
                let key = self.word();
                assert_eq!(key, "string", "a map's keys are strings");
                self.expect(b']');
                self.expect(b':');
                rest = Some(Box::new(self.shape()));
            } else {
                let name = self.word();
                assert!(!name.is_empty(), "shape: a field name at {}", self.i);
                let optional = self.eat(b'?');
                self.expect(b':');
                let shape = self.shape();
                fields.push(Field {
                    name,
                    optional,
                    shape,
                });
            }
            if !self.eat(b',') {
                self.expect(b'}');
                return Shape::Object(fields, rest);
            }
        }
    }
}

impl Shape {
    /// The JSON Schema of the shape.
    pub(super) fn schema(&self) -> Value {
        match self {
            Self::String => json!({ "type": "string" }),
            Self::Int => json!({ "type": "integer" }),
            Self::Uint => json!({ "type": "integer", "minimum": 0 }),
            Self::Number => json!({ "type": "number" }),
            Self::Bool => json!({ "type": "boolean" }),
            Self::Null => json!({ "type": "null" }),
            Self::Any => json!({}),
            Self::U64 => json!({
                "type": "string",
                "pattern": "^(0|[1-9][0-9]*)$",
                "description": "an unsigned 64-bit integer as a decimal string (D20)",
            }),
            Self::B64url => json!({ "type": "string", "contentEncoding": "base64url" }),
            Self::Lit(s) => json!({ "const": s }),
            Self::Ref(name) => json!({ "$ref": format!("#/components/schemas/{name}") }),
            Self::List(item) => json!({ "type": "array", "items": item.schema() }),
            Self::Object(fields, rest) => {
                let mut properties = Map::new();
                let mut required = Vec::new();
                for f in fields {
                    let _prev = properties.insert(f.name.clone(), f.shape.schema());
                    if !f.optional {
                        required.push(Value::String(f.name.clone()));
                    }
                }
                let mut schema = json!({ "type": "object" });
                if !properties.is_empty() {
                    schema["properties"] = Value::Object(properties);
                }
                if !required.is_empty() {
                    schema["required"] = Value::Array(required);
                }
                if let Some(rest) = rest {
                    schema["additionalProperties"] = rest.schema();
                }
                schema
            }
            Self::OneOf(alts) => {
                if alts.iter().all(|a| matches!(a, Self::Lit(_))) {
                    let values: Vec<Value> = alts
                        .iter()
                        .filter_map(|a| match a {
                            Self::Lit(s) => Some(Value::String(s.clone())),
                            _ => None,
                        })
                        .collect();
                    return json!({ "type": "string", "enum": values });
                }
                // Objects are exclusive alternatives (a tagged or externally named enum):
                // `oneOf`, which generators make a union of; anything else may overlap.
                let key = if alts.iter().all(|a| matches!(a, Self::Object(..))) {
                    "oneOf"
                } else {
                    "anyOf"
                };
                json!({ key: alts.iter().map(Self::schema).collect::<Vec<_>>() })
            }
        }
    }

    /// A value of this shape, every optional field present; `refs` resolves `#Name`.
    pub(super) fn sample(&self, refs: &dyn Fn(&str) -> Shape) -> Value {
        match self {
            Self::String => json!("x"),
            Self::Int | Self::Uint => json!(1),
            Self::Number => json!(1.5),
            Self::Bool => json!(true),
            Self::Null => Value::Null,
            Self::Any => json!({}),
            Self::U64 => json!("1"),
            Self::B64url => json!("AAAA"),
            Self::Lit(s) => json!(s),
            Self::Ref(name) => refs(name).sample(refs),
            Self::List(item) => json!([item.sample(refs)]),
            Self::Object(fields, rest) => {
                let mut out: Map<String, Value> = fields
                    .iter()
                    .map(|f| (f.name.clone(), f.shape.sample(refs)))
                    .collect();
                if let (true, Some(rest)) = (fields.is_empty(), rest) {
                    let _prev = out.insert("k".to_owned(), rest.sample(refs));
                }
                Value::Object(out)
            }
            Self::OneOf(alts) => alts
                .iter()
                .find(|a| **a != Self::Null)
                .unwrap_or(&Self::Null)
                .sample(refs),
        }
    }

    /// The object's fields; empty for any other shape.
    pub(super) fn fields(&self) -> &[Field] {
        match self {
            Self::Object(fields, _) => fields,
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shape_becomes_a_schema_and_a_sample() {
        let s = parse(
            r#"{ a: string, b?: u64 | null, c: ("x" | "y")[], d: { [string]: #T }, [string]: any }"#,
        );
        assert_eq!(
            s.schema(),
            json!({
                "type": "object",
                "properties": {
                    "a": { "type": "string" },
                    "b": { "anyOf": [Shape::U64.schema(), { "type": "null" }] },
                    "c": { "type": "array", "items": { "type": "string", "enum": ["x", "y"] } },
                    "d": { "type": "object", "additionalProperties": { "$ref": "#/components/schemas/T" } },
                },
                "required": ["a", "c", "d"],
                "additionalProperties": {},
            })
        );
        let sample = s.sample(&|_| Shape::Bool);
        assert_eq!(
            sample,
            json!({ "a": "x", "b": "1", "c": ["x"], "d": { "k": true } })
        );
        // Alternatives that are all objects are exclusive.
        assert_eq!(
            parse(r#"{ kind: "a" } | { kind: "b", n?: uint }"#).schema(),
            json!({ "oneOf": [
                { "type": "object", "properties": { "kind": { "const": "a" } }, "required": ["kind"] },
                { "type": "object", "properties": { "kind": { "const": "b" }, "n": { "type": "integer", "minimum": 0 } }, "required": ["kind"] },
            ] })
        );
    }
}
