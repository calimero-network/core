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
use crate::schema::{FunctionKind, ParameterDirection, ErrorAbi};

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
    assert_eq!(abi.metadata.schema_version, "0.1.1");
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
        returns: Some(AbiTypeRef::String),
        errors: vec![],
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
        returns: None,
        errors: vec![],
    };
    
    let function2 = AbiFunction {
        name: "func2".to_string(),
        kind: FunctionKind::Command,
        parameters: vec![],
        returns: None,
        errors: vec![],
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
        returns: None,
        errors: vec![],
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

#[test]
fn test_path_free_json() {
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
                name: "param1".to_string(),
                ty: AbiTypeRef::String,
                direction: ParameterDirection::Input,
            },
        ],
        returns: None,
        errors: vec![],
    };
    
    abi.add_function(function);
    
    // Serialize to JSON
    let mut output = Vec::new();
    write_canonical(&abi, &mut output).unwrap();
    
    let json_string = String::from_utf8(output).unwrap();
    
    // Check that JSON contains no absolute paths or backslashes
    assert!(!json_string.contains("\\"), "JSON should not contain backslashes");
    assert!(!json_string.contains("/Users/"), "JSON should not contain absolute paths");
    assert!(!json_string.contains("/home/"), "JSON should not contain absolute paths");
    assert!(!json_string.contains("C:\\"), "JSON should not contain Windows absolute paths");
    
    // Verify it's valid JSON
    let parsed: serde_json::Value = serde_json::from_str(&json_string).unwrap();
    assert!(parsed.is_object());
} 

#[test]
fn test_error_abi_creation() {
    let error = ErrorAbi {
        name: "InvalidInput".to_string(),
        code: "INVALID_INPUT".to_string(),
        ty: Some(AbiTypeRef::String),
    };
    
    assert_eq!(error.name, "InvalidInput");
    assert_eq!(error.code, "INVALID_INPUT");
    assert_eq!(error.ty, Some(AbiTypeRef::String));
}

#[test]
fn test_error_abi_unit_variant() {
    let error = ErrorAbi {
        name: "NotFound".to_string(),
        code: "NOT_FOUND".to_string(),
        ty: None,
    };
    
    assert_eq!(error.name, "NotFound");
    assert_eq!(error.code, "NOT_FOUND");
    assert_eq!(error.ty, None);
}

#[test]
fn test_error_abi_sorting() {
    let mut errors = vec![
        ErrorAbi {
            name: "ZError".to_string(),
            code: "Z_ERROR".to_string(),
            ty: None,
        },
        ErrorAbi {
            name: "AError".to_string(),
            code: "A_ERROR".to_string(),
            ty: None,
        },
        ErrorAbi {
            name: "BError".to_string(),
            code: "B_ERROR".to_string(),
            ty: None,
        },
    ];
    
    errors.sort();
    
    assert_eq!(errors[0].code, "A_ERROR");
    assert_eq!(errors[1].code, "B_ERROR");
    assert_eq!(errors[2].code, "Z_ERROR");
}

#[test]
fn test_function_with_errors() {
    let mut abi = Abi::new(
        "test_module".to_string(),
        "1.0.0".to_string(),
        "1.85.0".to_string(),
        "abc123".to_string(),
    );
    
    let function = AbiFunction {
        name: "test_function".to_string(),
        kind: FunctionKind::Command,
        parameters: vec![
            AbiParameter {
                name: "input".to_string(),
                ty: AbiTypeRef::String,
                direction: ParameterDirection::Input,
            },
        ],
        returns: None,
        errors: vec![
            ErrorAbi {
                name: "InvalidInput".to_string(),
                code: "INVALID_INPUT".to_string(),
                ty: Some(AbiTypeRef::String),
            },
            ErrorAbi {
                name: "NotFound".to_string(),
                code: "NOT_FOUND".to_string(),
                ty: None,
            },
        ],
    };
    
    abi.add_function(function);
    
    // Serialize and verify errors are included
    let mut output = Vec::new();
    write_canonical(&abi, &mut output).unwrap();
    
    let json_string = String::from_utf8(output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_string).unwrap();
    
    let errors = &parsed["functions"]["test_function"]["errors"];
    assert_eq!(errors.as_array().unwrap().len(), 2);
    assert_eq!(errors[0]["code"], "INVALID_INPUT");
    assert_eq!(errors[1]["code"], "NOT_FOUND");
}

#[test]
fn test_function_with_result_return() {
    let mut abi = Abi::new(
        "test_module".to_string(),
        "1.0.0".to_string(),
        "1.85.0".to_string(),
        "abc123".to_string(),
    );
    
    let function = AbiFunction {
        name: "compute".to_string(),
        kind: FunctionKind::Query,
        parameters: vec![
            AbiParameter {
                name: "value".to_string(),
                ty: AbiTypeRef::U64,
                direction: ParameterDirection::Input,
            },
        ],
        returns: Some(AbiTypeRef::U64),
        errors: vec![
            ErrorAbi {
                name: "Overflow".to_string(),
                code: "OVERFLOW".to_string(),
                ty: None,
            },
        ],
    };
    
    abi.add_function(function);
    
    // Serialize and verify returns and errors
    let mut output = Vec::new();
    write_canonical(&abi, &mut output).unwrap();
    
    let json_string = String::from_utf8(output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_string).unwrap();
    
    let func = &parsed["functions"]["compute"];
    assert_eq!(func["returns"]["type"], "U64");
    assert_eq!(func["errors"][0]["code"], "OVERFLOW");
} 