use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The main ABI manifest containing all type definitions, methods, and events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: String,
    pub types: BTreeMap<String, TypeDef>,
    pub methods: Vec<Method>,
    pub events: Vec<Event>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            schema_version: "wasm-abi/1".to_owned(),
            types: BTreeMap::new(),
            methods: Vec::new(),
            events: Vec::new(),
        }
    }
}

/// Type definition for complex types (records, variants)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TypeDef {
    #[serde(rename = "record")]
    Record { fields: Vec<Field> },
    #[serde(rename = "variant")]
    Variant { variants: Vec<Variant> },
    #[serde(rename = "bytes")]
    Bytes {
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encoding: Option<String>,
    },
    #[serde(rename = "alias")]
    Alias { target: TypeRef },
}

/// Field in a record type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: TypeRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
}

/// Variant in a variant type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<TypeRef>,
}

/// Method definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Method {
    pub name: String,
    pub params: Vec<Parameter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<TypeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns_nullable: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub errors: Vec<Error>,
}

/// Parameter in a method
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: TypeRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
}

/// Error definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<TypeRef>,
}

/// Event definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    #[serde(rename = "payload")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<TypeRef>,
}

/// Type reference - either inline or reference to a named type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypeRef {
    /// Reference to a named type
    Reference {
        #[serde(rename = "$ref")]
        ref_: String,
    },
    /// Inline scalar type
    Scalar(ScalarType),
    /// Inline collection type
    Collection(CollectionType),
}

/// Scalar types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ScalarType {
    #[serde(rename = "bool")]
    Bool,
    #[serde(rename = "i32")]
    I32,
    #[serde(rename = "i64")]
    I64,
    #[serde(rename = "u32")]
    U32,
    #[serde(rename = "u64")]
    U64,
    #[serde(rename = "f32")]
    F32,
    #[serde(rename = "f64")]
    F64,
    #[serde(rename = "string")]
    String,
    #[serde(rename = "bytes")]
    Bytes {
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encoding: Option<String>,
    },
    #[serde(rename = "unit")]
    Unit,
}

/// Collection types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum CollectionType {
    #[serde(rename = "list")]
    List { items: Box<TypeRef> },
    #[serde(rename = "map")]
    Map {
        #[serde(
            serialize_with = "serialize_map_key",
            deserialize_with = "deserialize_map_key"
        )]
        key: Box<TypeRef>,
        value: Box<TypeRef>,
    },
    #[serde(rename = "record")]
    Record { fields: Vec<Field> },
}

/// Custom serializer for map keys to support compact string format
fn serialize_map_key<S>(key: &TypeRef, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    key.serialize(serializer)
}

/// Custom deserializer for map keys to support compact string format
fn deserialize_map_key<'de, D>(deserializer: D) -> Result<Box<TypeRef>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use std::fmt;

    use serde::de::{self, Visitor};

    struct MapKeyVisitor;

    impl<'de> Visitor<'de> for MapKeyVisitor {
        type Value = Box<TypeRef>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a string or a TypeRef object")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value == "string" {
                Ok(Box::new(TypeRef::string()))
            } else {
                Err(de::Error::invalid_value(de::Unexpected::Str(value), &self))
            }
        }

        fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
        where
            M: de::MapAccess<'de>,
        {
            // Deserialize as a normal TypeRef object
            let type_ref = TypeRef::deserialize(de::value::MapAccessDeserializer::new(map))?;
            Ok(Box::new(type_ref))
        }
    }

    deserializer.deserialize_any(MapKeyVisitor)
}

impl Manifest {
    /// Create a new manifest with default schema version
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema_version: "wasm-abi/1".to_owned(),
            types: BTreeMap::new(),
            methods: Vec::new(),
            events: Vec::new(),
        }
    }
}

impl TypeRef {
    /// Create a reference to a named type
    #[must_use]
    pub fn reference(name: &str) -> Self {
        Self::Reference {
            ref_: name.to_owned(),
        }
    }

    /// Create a boolean type
    #[must_use]
    pub const fn bool() -> Self {
        Self::Scalar(ScalarType::Bool)
    }

    /// Create an i32 type
    #[must_use]
    pub const fn i32() -> Self {
        Self::Scalar(ScalarType::I32)
    }

    /// Create an i64 type
    #[must_use]
    pub const fn i64() -> Self {
        Self::Scalar(ScalarType::I64)
    }

    /// Create a u32 type
    #[must_use]
    pub const fn u32() -> Self {
        Self::Scalar(ScalarType::U32)
    }

    /// Create a u64 type
    #[must_use]
    pub const fn u64() -> Self {
        Self::Scalar(ScalarType::U64)
    }

    /// Create an f32 type
    #[must_use]
    pub const fn f32() -> Self {
        Self::Scalar(ScalarType::F32)
    }

    /// Create an f64 type
    #[must_use]
    pub const fn f64() -> Self {
        Self::Scalar(ScalarType::F64)
    }

    /// Create a string type
    #[must_use]
    pub const fn string() -> Self {
        Self::Scalar(ScalarType::String)
    }

    /// Create a bytes type (variable length)
    #[must_use]
    pub const fn bytes() -> Self {
        Self::Scalar(ScalarType::Bytes {
            size: None,
            encoding: None,
        })
    }

    /// Create a bytes type with size and encoding
    #[must_use]
    pub fn bytes_with_size(size: usize, encoding: Option<&str>) -> Self {
        Self::Scalar(ScalarType::Bytes {
            size: Some(size),
            encoding: encoding.map(ToOwned::to_owned),
        })
    }

    /// Create a unit type
    #[must_use]
    pub const fn unit() -> Self {
        Self::Scalar(ScalarType::Unit)
    }

    /// Create a list type
    #[must_use]
    pub fn list(items: Self) -> Self {
        Self::Collection(CollectionType::List {
            items: Box::new(items),
        })
    }

    /// Create a map type (key must be string)
    #[must_use]
    pub fn map(value: Self) -> Self {
        Self::Collection(CollectionType::Map {
            key: Box::new(Self::string()),
            value: Box::new(value),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_serialization() {
        let mut manifest = Manifest::default();

        // Add a simple method
        manifest.methods.push(Method {
            name: "test_method".to_string(),
            params: vec![Parameter {
                name: "param1".to_string(),
                type_: TypeRef::string(),
                nullable: None,
            }],
            returns: Some(TypeRef::i32()),
            returns_nullable: None,
            errors: Vec::new(),
        });

        // Serialize and deserialize
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let deserialized: Manifest = serde_json::from_str(&json).unwrap();

        assert_eq!(manifest.schema_version, deserialized.schema_version);
        assert_eq!(manifest.methods.len(), deserialized.methods.len());
        assert_eq!(manifest.methods[0].name, deserialized.methods[0].name);
    }

    #[test]
    fn test_manifest_creation() {
        let mut manifest = Manifest::new();

        // Add a test method
        manifest.methods.push(Method {
            name: "test_method".to_owned(),
            params: vec![Parameter {
                name: "param1".to_owned(),
                type_: TypeRef::string(),
                nullable: None,
            }],
            returns: Some(TypeRef::string()),
            returns_nullable: None,
            errors: Vec::new(),
        });

        assert_eq!(manifest.schema_version, "wasm-abi/1");
        assert_eq!(manifest.methods.len(), 1);
        assert_eq!(manifest.methods[0].name, "test_method");
    }
}
