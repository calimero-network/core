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
#[serde(tag = "type", content = "value")]
pub enum AbiTypeRef {
    /// Primitive types
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    U128,
    I128,
    String,
    
    /// Bytes type
    Bytes,
    
    /// Optional type
    Option(Box<AbiTypeRef>),
    
    /// Vector type
    Vec(Box<AbiTypeRef>),
    
    /// Reference to another type (for structs/enums)
    #[serde(rename = "ref")]
    Ref(String),
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

/// ABI function (query or command)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AbiFunction {
    /// Function name
    pub name: String,
    /// Function kind (query/command)
    pub kind: FunctionKind,
    /// Function parameters (preserving declared order)
    pub parameters: Vec<AbiParameter>,
    /// Return type (if any)
    pub return_type: Option<AbiTypeRef>,
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
                schema_version: "0.1.0".to_string(),
                toolchain_version,
                source_hash,
            },
            module_name,
            module_version,
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
}

/// Trait for types that can be represented in the ABI
pub trait AbiType {
    /// Return the ABI type name for this type
    fn abi_type() -> &'static str;
} 