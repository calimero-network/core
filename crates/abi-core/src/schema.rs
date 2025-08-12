// Copyright 2024 Calimero Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// ABI metadata with deterministic fields
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AbiMetadata {
    /// Schema version for compatibility
    pub schema_version: String,
    /// Rust toolchain version used for compilation
    pub toolchain_version: String,
    /// SHA256 hash of the source code (for determinism)
    pub source_hash: String,
}

/// ABI type reference with canonical ordering
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
pub enum AbiTypeRef {
    /// Reference to a type in the registry
    Ref {
        #[serde(rename = "$ref")]
        r#ref: String,
    },
    /// Inline primitive types (for backward compatibility)
    InlinePrimitive {
        #[serde(rename = "type")]
        kind: String,
    },
    /// Inline composite types (for backward compatibility)
    InlineComposite {
        #[serde(rename = "type")]
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<Box<AbiTypeRef>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        items: Option<Vec<AbiTypeRef>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        len: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<Box<AbiTypeRef>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<MapMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<FieldDef>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        newtype: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        variants: Option<Vec<VariantDef>>,
    },
}

/// Map mode for Map types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum MapMode {
    /// Object mode for String-keyed maps
    Object,
    /// Entries mode for other key types
    Entries,
}

/// Field definition for structs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct FieldDef {
    /// Field name
    pub name: String,
    /// Field type
    pub ty: AbiTypeRef,
}

/// Variant definition for enums
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct VariantDef {
    /// Variant name
    pub name: String,
    /// Variant kind
    pub kind: VariantKind,
}

/// Variant kind
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind")]
pub enum VariantKind {
    /// Unit variant (no payload)
    Unit,
    /// Tuple variant (unnamed fields)
    Tuple {
        /// Tuple field types
        items: Vec<AbiTypeRef>,
    },
    /// Struct variant (named fields)
    Struct {
        /// Struct field definitions
        fields: Vec<FieldDef>,
    },
}

/// Type definition for the registry
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind")]
pub enum TypeDef {
    /// Primitive type
    Primitive {
        /// Type name
        name: String,
    },
    /// Bytes type
    Bytes,
    /// String type
    String,
    /// Vector type
    Vec {
        /// Item type
        item: AbiTypeRef,
    },
    /// Optional type
    Option {
        /// Item type
        item: AbiTypeRef,
    },
    /// Tuple type (1-4 items)
    Tuple {
        /// Tuple item types
        items: Vec<AbiTypeRef>,
    },
    /// Fixed-size array
    Array {
        /// Item type
        item: AbiTypeRef,
        /// Array length
        len: u32,
    },
    /// Map type with dual mode
    Map {
        /// Key type
        key: AbiTypeRef,
        /// Value type
        value: AbiTypeRef,
        /// Map mode
        mode: MapMode,
    },
    /// Struct type
    Struct {
        /// Struct fields
        fields: Vec<FieldDef>,
        /// Whether this is a newtype struct
        newtype: bool,
    },
    /// Enum type
    Enum {
        /// Enum variants
        variants: Vec<VariantDef>,
    },
}

/// Function parameter with direction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AbiParameter {
    /// Parameter name
    pub name: String,
    /// Parameter type
    pub ty: AbiTypeRef,
    /// Parameter direction (input/output)
    pub direction: ParameterDirection,
}

/// Parameter direction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ParameterDirection {
    Input,
    Output,
}

/// Error information for function returns
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ErrorAbi {
    /// Error variant name (exact Rust identifier, case-preserving)
    pub name: String,
    /// Error code (SCREAMING_SNAKE_CASE, stable)
    pub code: String,
    /// Error payload type (for tuple/struct variants, None for unit variants)
    pub ty: Option<AbiTypeRef>,
}

/// ABI function (query or command)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AbiFunction {
    /// Function name
    pub name: String,
    /// Function kind (query/command)
    pub kind: FunctionKind,
    /// Function parameters (preserving declared order)
    pub parameters: Vec<AbiParameter>,
    /// Return type (success payload, None for unit type ())
    pub returns: Option<AbiTypeRef>,
    /// Error variants (sorted by code)
    pub errors: Vec<ErrorAbi>,
}

/// Function kind
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FunctionKind {
    Query,
    Command,
}

/// ABI event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AbiEvent {
    /// Event name
    pub name: String,
    /// Event payload type (for now, just a placeholder)
    pub payload_type: Option<AbiTypeRef>,
}

/// Complete ABI definition
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Abi {
    /// ABI metadata
    pub metadata: AbiMetadata,
    /// Module name
    pub module_name: String,
    /// Module version
    pub module_version: String,
    /// Type registry (optional, for advanced types)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<BTreeMap<String, TypeDef>>,
    /// Functions (sorted by kind, then name)
    pub functions: BTreeMap<String, AbiFunction>,
    /// Events (sorted by name)
    pub events: BTreeMap<String, AbiEvent>,
}

impl Abi {
    /// Create a new ABI with canonical ordering
    pub fn new(
        module_name: String,
        module_version: String,
        toolchain_version: String,
        source_hash: String,
    ) -> Self {
        Self {
            metadata: AbiMetadata {
                schema_version: "0.1.1".to_string(),
                toolchain_version,
                source_hash,
            },
            module_name,
            module_version,
            types: None,
            functions: BTreeMap::new(),
            events: BTreeMap::new(),
        }
    }
    
    /// Add a function to the ABI
    pub fn add_function(&mut self, function: AbiFunction) {
        self.functions.insert(function.name.clone(), function);
    }
    
    /// Add an event to the ABI
    pub fn add_event(&mut self, event: AbiEvent) {
        self.events.insert(event.name.clone(), event);
    }
    
    /// Add a type to the registry
    pub fn add_type(&mut self, name: String, ty: TypeDef) {
        if self.types.is_none() {
            self.types = Some(BTreeMap::new());
        }
        if let Some(types) = &mut self.types {
            types.insert(name, ty);
        }
    }
}

/// Trait for types that can be represented in the ABI
pub trait AbiType {
    /// Return the ABI type name for this type
    fn abi_type() -> &'static str;
}

// Helper functions for creating type references
impl AbiTypeRef {
    /// Create a reference to a type in the registry
    pub fn ref_(name: String) -> Self {
        AbiTypeRef::Ref { r#ref: name }
    }
    
    /// Create an inline primitive type
    pub fn inline_primitive(kind: String) -> Self {
        AbiTypeRef::InlinePrimitive { kind }
    }
    
    /// Create an inline composite type
    pub fn inline_composite(
        kind: String,
        value: Option<Box<AbiTypeRef>>,
        items: Option<Vec<AbiTypeRef>>,
        len: Option<u32>,
        key: Option<Box<AbiTypeRef>>,
        mode: Option<MapMode>,
        fields: Option<Vec<FieldDef>>,
        newtype: Option<bool>,
        variants: Option<Vec<VariantDef>>,
    ) -> Self {
        AbiTypeRef::InlineComposite {
            kind,
            value,
            items,
            len,
            key,
            mode,
            fields,
            newtype,
            variants,
        }
    }
} 