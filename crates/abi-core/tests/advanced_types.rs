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

use abi_core::schema::{
    Abi, AbiTypeRef, TypeDef, FieldDef, VariantDef, VariantKind, MapMode, 
    FunctionKind, ParameterDirection, ErrorAbi, AbiParameter, AbiFunction, AbiEvent
};

#[test]
fn test_advanced_types_serialization() {
    // Create a complex ABI with advanced types
    let mut abi = Abi::new(
        "advanced_types_demo".to_string(),
        "0.1.0".to_string(),
        "1.85.0".to_string(),
        "test_hash_for_determinism".to_string(),
    );
    
    // Add complex struct type
    let complex_struct_fields = vec![
        FieldDef {
            name: "id".to_string(),
            ty: AbiTypeRef::inline_primitive("u64".to_string()),
        },
        FieldDef {
            name: "name".to_string(),
            ty: AbiTypeRef::inline_primitive("string".to_string()),
        },
        FieldDef {
            name: "data".to_string(),
            ty: AbiTypeRef::inline_composite(
                "option".to_string(),
                Some(Box::new(AbiTypeRef::inline_composite(
                    "vec".to_string(),
                    Some(Box::new(AbiTypeRef::inline_primitive("u8".to_string()))),
                    None, None, None, None, None, None, None
                ))),
                None, None, None, None, None, None, None
            ),
        },
        FieldDef {
            name: "metadata".to_string(),
            ty: AbiTypeRef::inline_composite(
                "map".to_string(),
                None, None, None,
                Some(Box::new(AbiTypeRef::inline_primitive("string".to_string()))),
                Some(MapMode::Object),
                None, None, None
            ),
        },
    ];
    
    abi.add_type(
        "advanced_types_demo::ComplexStruct".to_string(),
        TypeDef::Struct {
            fields: complex_struct_fields,
            newtype: false,
        },
    );
    
    // Add newtype struct
    let newtype_fields = vec![
        FieldDef {
            name: "0".to_string(),
            ty: AbiTypeRef::inline_primitive("u128".to_string()),
        },
    ];
    
    abi.add_type(
        "advanced_types_demo::UserId".to_string(),
        TypeDef::Struct {
            fields: newtype_fields,
            newtype: true,
        },
    );
    
    // Add enum type
    let enum_variants = vec![
        VariantDef {
            name: "Pending".to_string(),
            kind: VariantKind::Unit,
        },
        VariantDef {
            name: "Active".to_string(),
            kind: VariantKind::Tuple {
                items: vec![AbiTypeRef::inline_primitive("u32".to_string())],
            },
        },
        VariantDef {
            name: "Completed".to_string(),
            kind: VariantKind::Struct {
                fields: vec![
                    FieldDef {
                        name: "timestamp".to_string(),
                        ty: AbiTypeRef::inline_primitive("u64".to_string()),
                    },
                    FieldDef {
                        name: "result".to_string(),
                        ty: AbiTypeRef::inline_primitive("string".to_string()),
                    },
                ],
            },
        },
    ];
    
    abi.add_type(
        "advanced_types_demo::Status".to_string(),
        TypeDef::Enum { variants: enum_variants },
    );
    
    // Add function with advanced types
    let function = AbiFunction {
        name: "get_user_info".to_string(),
        kind: FunctionKind::Query,
        parameters: vec![
            AbiParameter {
                name: "user_id".to_string(),
                ty: AbiTypeRef::ref_("advanced_types_demo::UserId".to_string()),
                direction: ParameterDirection::Input,
            },
        ],
        returns: Some(AbiTypeRef::ref_("advanced_types_demo::ComplexStruct".to_string())),
        errors: vec![
            ErrorAbi {
                name: "NotFound".to_string(),
                code: "NOT_FOUND".to_string(),
                ty: Some(AbiTypeRef::inline_primitive("u64".to_string())),
            },
        ],
    };
    
    abi.add_function(function);
    
    // Test serialization
    let json = serde_json::to_string_pretty(&abi).unwrap();
    
    // Verify the JSON contains expected elements
    assert!(json.contains("\"schema_version\": \"0.1.1\""));
    assert!(json.contains("\"module_name\": \"advanced_types_demo\""));
    assert!(json.contains("\"types\":"));
    assert!(json.contains("\"advanced_types_demo::ComplexStruct\""));
    assert!(json.contains("\"advanced_types_demo::UserId\""));
    assert!(json.contains("\"advanced_types_demo::Status\""));
    assert!(json.contains("\"get_user_info\""));
    assert!(json.contains("\"$ref\":"));
    
    // Test that types registry is present when types are added
    assert!(abi.types.is_some());
    let types = abi.types.as_ref().unwrap();
    assert!(types.contains_key("advanced_types_demo::ComplexStruct"));
    assert!(types.contains_key("advanced_types_demo::UserId"));
    assert!(types.contains_key("advanced_types_demo::Status"));
}

#[test]
fn test_backward_compatibility() {
    // Create ABI without advanced types (backward compatible)
    let abi = Abi::new(
        "simple_demo".to_string(),
        "0.1.0".to_string(),
        "1.85.0".to_string(),
        "test_hash".to_string(),
    );
    
    // Verify types registry is None when no types are added
    assert!(abi.types.is_none());
    
    // Test serialization doesn't include types field
    let json = serde_json::to_string_pretty(&abi).unwrap();
    assert!(!json.contains("\"types\":"));
}

#[test]
fn test_map_modes() {
    // Test object mode for String-keyed maps
    let object_map = AbiTypeRef::inline_composite(
        "map".to_string(),
        None, None, None,
        Some(Box::new(AbiTypeRef::inline_primitive("string".to_string()))),
        Some(MapMode::Object),
        None, None, None
    );
    
    // Test entries mode for other key types
    let entries_map = AbiTypeRef::inline_composite(
        "map".to_string(),
        None, None, None,
        Some(Box::new(AbiTypeRef::inline_primitive("u64".to_string()))),
        Some(MapMode::Entries),
        None, None, None
    );
    
    let object_json = serde_json::to_value(&object_map).unwrap();
    let entries_json = serde_json::to_value(&entries_map).unwrap();
    
    assert_eq!(object_json["mode"], "object");
    assert_eq!(entries_json["mode"], "entries");
}

#[test]
fn test_tuple_and_array_types() {
    // Test tuple type
    let tuple_type = AbiTypeRef::inline_composite(
        "tuple".to_string(),
        None,
        Some(vec![
            AbiTypeRef::inline_primitive("u8".to_string()),
            AbiTypeRef::inline_primitive("string".to_string()),
        ]),
        None, None, None, None, None, None
    );
    
    // Test array type
    let array_type = AbiTypeRef::inline_composite(
        "array".to_string(),
        Some(Box::new(AbiTypeRef::inline_primitive("u16".to_string()))),
        None,
        Some(4),
        None, None, None, None, None
    );
    
    let tuple_json = serde_json::to_value(&tuple_type).unwrap();
    let array_json = serde_json::to_value(&array_type).unwrap();
    
    assert_eq!(tuple_json["type"], "tuple");
    assert_eq!(tuple_json["items"].as_array().unwrap().len(), 2);
    
    assert_eq!(array_json["type"], "array");
    assert_eq!(array_json["len"], 4);
} 