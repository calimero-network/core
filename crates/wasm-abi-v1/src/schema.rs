use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
            schema_version: "wasm-abi/1".to_string(),
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
    Record {
        fields: Vec<Field>,
    },
    #[serde(rename = "variant")]
    Variant {
        variants: Vec<Variant>,
    },
}

/// Field in a record type
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_: Option<TypeRef>,
}

/// Method definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Method {
    pub name: String,
    pub params: Vec<Parameter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<TypeRef>,
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
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_: Option<TypeRef>,
}

/// Event definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_: Option<TypeRef>,
}

/// Type reference - either inline or reference to a named type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypeRef {
    /// Reference to a named type
    Reference { #[serde(rename = "$ref")] ref_: String },
    /// Inline scalar type
    Scalar(ScalarType),
    /// Inline collection type
    Collection(CollectionType),
}

/// Scalar types
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        size: usize,
        encoding: String,
    },
    #[serde(rename = "unit")]
    Unit,
}

/// Collection types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum CollectionType {
    #[serde(rename = "list")]
    List {
        items: Box<TypeRef>,
    },
    #[serde(rename = "map")]
    Map {
        key: Box<TypeRef>,
        value: Box<TypeRef>,
    },
}

impl TypeRef {
    /// Create a reference to a named type
    pub fn reference(name: &str) -> Self {
        TypeRef::Reference {
            ref_: name.to_string(),
        }
    }

    /// Create a boolean type
    pub fn bool() -> Self {
        TypeRef::Scalar(ScalarType::Bool)
    }

    /// Create an i32 type
    pub fn i32() -> Self {
        TypeRef::Scalar(ScalarType::I32)
    }

    /// Create an i64 type
    pub fn i64() -> Self {
        TypeRef::Scalar(ScalarType::I64)
    }

    /// Create a u32 type
    pub fn u32() -> Self {
        TypeRef::Scalar(ScalarType::U32)
    }

    /// Create a u64 type
    pub fn u64() -> Self {
        TypeRef::Scalar(ScalarType::U64)
    }

    /// Create an f32 type
    pub fn f32() -> Self {
        TypeRef::Scalar(ScalarType::F32)
    }

    /// Create an f64 type
    pub fn f64() -> Self {
        TypeRef::Scalar(ScalarType::F64)
    }

    /// Create a string type
    pub fn string() -> Self {
        TypeRef::Scalar(ScalarType::String)
    }

    /// Create a bytes type
    pub fn bytes() -> Self {
        TypeRef::Scalar(ScalarType::Bytes {
            size: 0,
            encoding: "hex".to_string(),
        })
    }

    /// Create a bytes type with size and encoding
    pub fn bytes_with_size(size: usize, encoding: &str) -> Self {
        TypeRef::Scalar(ScalarType::Bytes {
            size,
            encoding: encoding.to_string(),
        })
    }

    /// Create a unit type
    pub fn unit() -> Self {
        TypeRef::Scalar(ScalarType::Unit)
    }

    /// Create a list type
    pub fn list(items: TypeRef) -> Self {
        TypeRef::Collection(CollectionType::List {
            items: Box::new(items),
        })
    }

    /// Create a map type (key must be string)
    pub fn map(value: TypeRef) -> Self {
        TypeRef::Collection(CollectionType::Map {
            key: Box::new(TypeRef::string()),
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
            params: vec![
                Parameter {
                    name: "param1".to_string(),
                    type_: TypeRef::string(),
                    nullable: None,
                }
            ],
            returns: Some(TypeRef::i32()),
            errors: Vec::new(),
        });
        
        // Serialize and deserialize
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let deserialized: Manifest = serde_json::from_str(&json).unwrap();
        
        assert_eq!(manifest.schema_version, deserialized.schema_version);
        assert_eq!(manifest.methods.len(), deserialized.methods.len());
        assert_eq!(manifest.methods[0].name, deserialized.methods[0].name);
    }
} 