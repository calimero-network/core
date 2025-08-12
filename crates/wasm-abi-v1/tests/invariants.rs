use calimero_wasm_abi_v1::schema::{Manifest, TypeRef, TypeDef, Method, Event, Error, Field, Variant, Parameter};
use calimero_wasm_abi_v1::validate::validate_manifest;

#[test]
fn test_invariant_events_use_payload_not_type() {
    let mut manifest = Manifest::default();
    
    // Add a simple type
    manifest.types.insert("TestType".to_string(), TypeDef::Record {
        fields: vec![]
    });
    
    // Add an event with payload (should pass)
    manifest.events.push(Event {
        name: "TestEvent".to_string(),
        payload: Some(TypeRef::string()),
    });
    
    // Should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_variable_bytes_no_size() {
    let mut manifest = Manifest::default();
    
    // Add a type with variable bytes (should pass)
    manifest.types.insert("TestType".to_string(), TypeDef::Record {
        fields: vec![
            Field {
                name: "data".to_string(),
                type_: TypeRef::bytes(), // Variable bytes
                nullable: None,
            }
        ]
    });
    
    // Should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_map_string_key() {
    let mut manifest = Manifest::default();
    
    // Add a type with valid map (string key)
    manifest.types.insert("TestType".to_string(), TypeDef::Record {
        fields: vec![
            Field {
                name: "map".to_string(),
                type_: TypeRef::map(TypeRef::i32()), // Map with string key
                nullable: None,
            }
        ]
    });
    
    // Should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_no_dangling_refs() {
    let mut manifest = Manifest::default();
    
    // Add a type that exists
    manifest.types.insert("ExistingType".to_string(), TypeDef::Record {
        fields: vec![]
    });
    
    // Add a method that references the existing type
    manifest.methods.push(Method {
        name: "test".to_string(),
        params: vec![],
        returns: Some(TypeRef::reference("ExistingType")),
        returns_nullable: None,
        errors: vec![],
    });
    
    // Should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_detects_dangling_refs() {
    let mut manifest = Manifest::default();
    
    // Add a method that references a non-existent type
    manifest.methods.push(Method {
        name: "test".to_string(),
        params: vec![],
        returns: Some(TypeRef::reference("NonExistentType")),
        returns_nullable: None,
        errors: vec![],
    });
    
    // Should fail validation
    let result = validate_manifest(&manifest);
    assert!(result.is_err());
    match result.unwrap_err() {
        calimero_wasm_abi_v1::validate::ValidationError::DanglingRef { ref_name, .. } => {
            assert_eq!(ref_name, "NonExistentType");
        }
        _ => panic!("Expected DanglingRef error"),
    }
}

#[test]
fn test_invariant_deterministic_ordering() {
    let mut manifest = Manifest::default();
    
    // Add types in non-sorted order
    manifest.types.insert("ZType".to_string(), TypeDef::Record { fields: vec![] });
    manifest.types.insert("AType".to_string(), TypeDef::Record { fields: vec![] });
    
    // Add methods in non-sorted order
    manifest.methods.push(Method {
        name: "z_method".to_string(),
        params: vec![],
        returns: Some(TypeRef::i32()),
        returns_nullable: None,
        errors: vec![],
    });
    manifest.methods.push(Method {
        name: "a_method".to_string(),
        params: vec![],
        returns: Some(TypeRef::i32()),
        returns_nullable: None,
        errors: vec![],
    });
    
    // Add events in non-sorted order
    manifest.events.push(Event {
        name: "z_event".to_string(),
        payload: None,
    });
    manifest.events.push(Event {
        name: "a_event".to_string(),
        payload: None,
    });
    
    // Should fail validation (not sorted)
    assert!(validate_manifest(&manifest).is_err());
    
    // Sort them manually
    manifest.methods.sort_by(|a, b| a.name.cmp(&b.name));
    manifest.events.sort_by(|a, b| a.name.cmp(&b.name));
    
    // Should pass validation after sorting
    assert!(validate_manifest(&manifest).is_ok());
    
    // Verify they are sorted
    let method_names: Vec<_> = manifest.methods.iter().map(|m| &m.name).collect();
    let mut sorted_names = method_names.clone();
    sorted_names.sort();
    assert_eq!(method_names, sorted_names);
    
    let event_names: Vec<_> = manifest.events.iter().map(|e| &e.name).collect();
    let mut sorted_event_names = event_names.clone();
    sorted_event_names.sort();
    assert_eq!(event_names, sorted_event_names);
}

#[test]
fn test_invariant_variant_payload_structure() {
    let mut manifest = Manifest::default();
    
    // Add a variant type with payload
    manifest.types.insert("TestVariant".to_string(), TypeDef::Variant {
        variants: vec![
            Variant {
                name: "UnitVariant".to_string(),
                code: None,
                payload: None,
            },
            Variant {
                name: "StringVariant".to_string(),
                code: None,
                payload: Some(TypeRef::string()),
            },
            Variant {
                name: "RecordVariant".to_string(),
                code: None,
                payload: Some(TypeRef::Collection(calimero_wasm_abi_v1::schema::CollectionType::Record {
                    fields: vec![
                        Field {
                            name: "value".to_string(),
                            type_: TypeRef::i32(),
                            nullable: None,
                        }
                    ]
                })),
            },
        ]
    });
    
    // Should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_error_payload_structure() {
    let mut manifest = Manifest::default();
    
    // Add a method with errors that have payload
    manifest.methods.push(Method {
        name: "test".to_string(),
        params: vec![],
        returns: Some(TypeRef::i32()),
        returns_nullable: None,
        errors: vec![
            Error {
                code: "SIMPLE_ERROR".to_string(),
                payload: None,
            },
            Error {
                code: "DETAILED_ERROR".to_string(),
                payload: Some(TypeRef::string()),
            },
        ],
    });
    
    // Should pass validation
    assert!(validate_manifest(&manifest).is_ok());
} 