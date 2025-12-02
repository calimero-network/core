use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

/// The main ABI manifest containing all type definitions, methods, and events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: String,
    pub types: BTreeMap<String, TypeDef>,
    pub methods: Vec<Method>,
    pub events: Vec<Event>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_root: Option<String>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            schema_version: "wasm-abi/1".to_owned(),
            types: BTreeMap::new(),
            methods: Vec::new(),
            events: Vec::new(),
            state_root: None,
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
    /// Inline collection type with optional CRDT metadata
    Collection {
        #[serde(flatten)]
        collection: CollectionType,
        /// Original Calimero CRDT collection type (if applicable)
        ///
        /// This is preserved when CRDT types are "unwrapped" during normalization.
        /// For example, `LwwRegister<String>` normalizes to a Collection with empty Record
        /// but preserves `crdt_type: LwwRegister` and the inner type in the collection structure.
        /// Deserializers use this to expect the CRDT format:
        /// - `LwwRegister<T>`: `(value: T, timestamp: HybridTimestamp, node_id: [u8; 32])`
        /// - `Counter`: `(positive: UnorderedMap<String, u64>, negative?: UnorderedMap<String, u64>)`
        /// - `UnorderedMap<K, V>`: entries with element IDs
        /// - `Vector<T>`: list with CRDT metadata
        #[serde(skip_serializing_if = "Option::is_none", rename = "crdt_type")]
        crdt_type: Option<CrdtCollectionType>,
        /// Inner type for CRDT wrappers (e.g., LwwRegister<T> needs to know T)
        ///
        /// This is used when the CRDT type wraps another type that was "unwrapped" during normalization.
        /// For LwwRegister<T>, this stores T so the deserializer knows what type to deserialize the value as.
        #[serde(skip_serializing_if = "Option::is_none", rename = "inner_type")]
        inner_type: Option<Box<TypeRef>>,
    },
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

/// Calimero CRDT collection types
///
/// These types have special serialization formats that include CRDT metadata
/// (timestamps, node IDs, element IDs, etc.) and must be preserved for correct deserialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrdtCollectionType {
    /// Last-Write-Wins Register: `(value: T, timestamp, node_id)`
    LwwRegister,
    /// Counter: `(positive: UnorderedMap<String, u64>, negative?: UnorderedMap<String, u64>)`
    Counter,
    /// Vector: List with CRDT metadata
    Vector,
    /// UnorderedMap: Map with element IDs and CRDT metadata
    UnorderedMap,
    /// UnorderedSet: Set with CRDT metadata
    UnorderedSet,
    /// ReplicatedGrowableArray: String with character-level CRDT
    ReplicatedGrowableArray,
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
            state_root: None,
        }
    }

    /// Extract the state schema (state root type and all its dependencies)
    ///
    /// Returns a new Manifest containing only the state root type and all types
    /// it references recursively. This is useful for serialization/deserialization
    /// of state without needing the full ABI.
    ///
    /// # Errors
    /// Returns an error if no state_root is defined or if type dependencies cannot be resolved.
    pub fn extract_state_schema(&self) -> Result<Self, Box<dyn std::error::Error>> {
        let state_root_name = self
            .state_root
            .as_ref()
            .ok_or_else(|| "No state_root defined in manifest")?;

        // Recursively collect all types referenced by the state root
        let mut collected_types = BTreeMap::new();
        let mut visited = HashSet::new();
        Self::collect_type_dependencies(
            state_root_name,
            &self.types,
            &mut collected_types,
            &mut visited,
        )?;

        Ok(Self {
            schema_version: self.schema_version.clone(),
            types: collected_types,
            methods: Vec::new(),
            events: Vec::new(),
            state_root: Some(state_root_name.clone()),
        })
    }

    /// Recursively collect all type dependencies starting from a root type
    fn collect_type_dependencies(
        type_name: &str,
        all_types: &BTreeMap<String, TypeDef>,
        collected: &mut BTreeMap<String, TypeDef>,
        visited: &mut HashSet<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Avoid infinite recursion
        if visited.contains(type_name) {
            return Ok(());
        }
        visited.insert(type_name.to_string());

        // Get the type definition
        let type_def = all_types
            .get(type_name)
            .ok_or_else(|| format!("Type '{}' not found in ABI types", type_name))?;

        // Add this type to collected types
        collected.insert(type_name.to_string(), type_def.clone());

        // Recursively collect dependencies from this type
        Self::collect_dependencies_from_type_def(type_def, all_types, collected, visited)?;

        Ok(())
    }

    /// Collect all type references from a TypeDef
    fn collect_dependencies_from_type_def(
        type_def: &TypeDef,
        all_types: &BTreeMap<String, TypeDef>,
        collected: &mut BTreeMap<String, TypeDef>,
        visited: &mut HashSet<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match type_def {
            TypeDef::Record { fields } => {
                for field in fields {
                    Self::collect_dependencies_from_type_ref(
                        &field.type_,
                        all_types,
                        collected,
                        visited,
                    )?;
                }
            }
            TypeDef::Variant { variants } => {
                for variant in variants {
                    if let Some(ref payload) = variant.payload {
                        Self::collect_dependencies_from_type_ref(
                            payload, all_types, collected, visited,
                        )?;
                    }
                }
            }
            TypeDef::Alias { target } => {
                Self::collect_dependencies_from_type_ref(target, all_types, collected, visited)?;
            }
            TypeDef::Bytes { .. } => {
                // No dependencies
            }
        }
        Ok(())
    }

    /// Collect all type references from a TypeRef
    fn collect_dependencies_from_type_ref(
        type_ref: &TypeRef,
        all_types: &BTreeMap<String, TypeDef>,
        collected: &mut BTreeMap<String, TypeDef>,
        visited: &mut HashSet<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match type_ref {
            TypeRef::Reference { ref_ } => {
                // This is a reference to another type - collect it recursively
                Self::collect_type_dependencies(ref_, all_types, collected, visited)?;
            }
            TypeRef::Scalar(_) => {
                // Scalar types have no dependencies
            }
            TypeRef::Collection {
                collection,
                inner_type,
                ..
            } => {
                // Also collect dependencies from inner_type if present (e.g., for LwwRegister<T>)
                if let Some(inner) = inner_type {
                    Self::collect_dependencies_from_type_ref(inner, all_types, collected, visited)?;
                }

                match collection {
                    CollectionType::List { items } => {
                        Self::collect_dependencies_from_type_ref(
                            items, all_types, collected, visited,
                        )?;
                    }
                    CollectionType::Map { key, value } => {
                        Self::collect_dependencies_from_type_ref(
                            key, all_types, collected, visited,
                        )?;
                        Self::collect_dependencies_from_type_ref(
                            value, all_types, collected, visited,
                        )?;
                    }
                    CollectionType::Record { fields } => {
                        for field in fields {
                            Self::collect_dependencies_from_type_ref(
                                &field.type_,
                                all_types,
                                collected,
                                visited,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
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
        Self::Collection {
            collection: CollectionType::List {
                items: Box::new(items),
            },
            crdt_type: None,
            inner_type: None,
        }
    }

    /// Create a map type (key must be string)
    #[must_use]
    pub fn map(value: Self) -> Self {
        Self::Collection {
            collection: CollectionType::Map {
                key: Box::new(Self::string()),
                value: Box::new(value),
            },
            crdt_type: None,
            inner_type: None,
        }
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
            name: "test_method".to_owned(),
            params: vec![Parameter {
                name: "param1".to_owned(),
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
        assert_eq!(manifest.state_root, deserialized.state_root);
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
        assert!(manifest.state_root.is_none());
    }
}
