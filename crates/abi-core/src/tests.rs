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

use super::*;
use crate::schema::{FunctionKind, ParameterDirection};

#[test]
fn test_abi_creation() {
    let mut abi = Abi::new(
        "test_module".to_string(),
        "1.0.0".to_string(),
        "1.85.0".to_string(),
        "abc123".to_string(),
    );
    
    assert_eq!(abi.module_name, "test_module");
    assert_eq!(abi.module_version, "1.0.0");
    assert_eq!(abi.metadata.schema_version, "0.1.0");
    assert_eq!(abi.metadata.toolchain_version, "1.85.0");
    assert_eq!(abi.metadata.source_hash, "abc123");
}

#[test]
fn test_canonical_serialization() {
    let mut abi = Abi::new(
        "test_module".to_string(),
        "1.0.0".to_string(),
        "1.85.0".to_string(),
        "abc123".to_string(),
    );
    
    // Add a function
    let function = AbiFunction {
        name: "test_function".to_string(),
        kind: FunctionKind::Query,
        parameters: vec![
            AbiParameter {
                name: "param1".to_string(),
                ty: AbiTypeRef::String,
                direction: ParameterDirection::Input,
            },
        ],
        return_type: Some(AbiTypeRef::String),
    };
    abi.add_function(function);
    
    // Serialize to canonical JSON
    let mut output = Vec::new();
    write_canonical(&abi, &mut output).unwrap();
    
    let json_string = String::from_utf8(output).unwrap();
    
    // Verify it's valid JSON
    let parsed: serde_json::Value = serde_json::from_str(&json_string).unwrap();
    assert_eq!(parsed["module_name"], "test_module");
    assert_eq!(parsed["module_version"], "1.0.0");
}

#[test]
fn test_deterministic_serialization() {
    let mut abi1 = Abi::new(
        "test_module".to_string(),
        "1.0.0".to_string(),
        "1.85.0".to_string(),
        "abc123".to_string(),
    );
    
    let mut abi2 = Abi::new(
        "test_module".to_string(),
        "1.0.0".to_string(),
        "1.85.0".to_string(),
        "abc123".to_string(),
    );
    
    // Add functions in different order
    let function1 = AbiFunction {
        name: "func1".to_string(),
        kind: FunctionKind::Query,
        parameters: vec![],
        return_type: None,
    };
    
    let function2 = AbiFunction {
        name: "func2".to_string(),
        kind: FunctionKind::Command,
        parameters: vec![],
        return_type: None,
    };
    
    abi1.add_function(function1.clone());
    abi1.add_function(function2.clone());
    
    abi2.add_function(function2);
    abi2.add_function(function1);
    
    // Both should produce identical canonical output
    let mut output1 = Vec::new();
    let mut output2 = Vec::new();
    
    write_canonical(&abi1, &mut output1).unwrap();
    write_canonical(&abi2, &mut output2).unwrap();
    
    assert_eq!(output1, output2);
}

#[test]
fn test_sha256_hash() {
    let mut abi = Abi::new(
        "test_module".to_string(),
        "1.0.0".to_string(),
        "1.85.0".to_string(),
        "abc123".to_string(),
    );
    
    let hash1 = sha256(&abi).unwrap();
    let hash2 = sha256(&abi).unwrap();
    
    // Same ABI should produce same hash
    assert_eq!(hash1, hash2);
    
    // Different ABI should produce different hash
    let mut abi2 = Abi::new(
        "test_module".to_string(),
        "1.0.0".to_string(),
        "1.85.0".to_string(),
        "def456".to_string(), // Different source hash
    );
    
    let hash3 = sha256(&abi2).unwrap();
    assert_ne!(hash1, hash3);
}

#[test]
fn test_abi_type_ref_ordering() {
    let types = vec![
        AbiTypeRef::Bool,
        AbiTypeRef::U8,
        AbiTypeRef::U16,
        AbiTypeRef::U32,
        AbiTypeRef::U64,
        AbiTypeRef::I8,
        AbiTypeRef::I16,
        AbiTypeRef::I32,
        AbiTypeRef::I64,
        AbiTypeRef::U128,
        AbiTypeRef::I128,
        AbiTypeRef::String,
        AbiTypeRef::Bytes,
        AbiTypeRef::Option(Box::new(AbiTypeRef::String)),
        AbiTypeRef::Vec(Box::new(AbiTypeRef::U32)),
        AbiTypeRef::Ref("MyStruct".to_string()),
    ];
    
    // Test that types can be sorted
    let mut sorted = types.clone();
    sorted.sort();
    
    // Should be deterministic
    let mut sorted2 = types;
    sorted2.sort();
    
    assert_eq!(sorted, sorted2);
}

#[test]
fn test_parameter_order_preservation() {
    let mut abi = Abi::new(
        "test_module".to_string(),
        "1.0.0".to_string(),
        "1.85.0".to_string(),
        "abc123".to_string(),
    );
    
    let function = AbiFunction {
        name: "test_function".to_string(),
        kind: FunctionKind::Query,
        parameters: vec![
            AbiParameter {
                name: "first".to_string(),
                ty: AbiTypeRef::U32,
                direction: ParameterDirection::Input,
            },
            AbiParameter {
                name: "second".to_string(),
                ty: AbiTypeRef::String,
                direction: ParameterDirection::Input,
            },
            AbiParameter {
                name: "third".to_string(),
                ty: AbiTypeRef::Bool,
                direction: ParameterDirection::Input,
            },
        ],
        return_type: None,
    };
    
    abi.add_function(function);
    
    // Serialize and check parameter order
    let mut output = Vec::new();
    write_canonical(&abi, &mut output).unwrap();
    
    let json_string = String::from_utf8(output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_string).unwrap();
    
    let params = &parsed["functions"]["test_function"]["parameters"];
    assert_eq!(params[0]["name"], "first");
    assert_eq!(params[1]["name"], "second");
    assert_eq!(params[2]["name"], "third");
} 