use calimero_wasm_abi::schema::{Error, Event, Manifest, Method, TypeDef, TypeRef, Variant};
use calimero_wasm_abi::validate::validate_manifest;

#[test]
fn test_invariant_events_use_payload_not_type() {
    // This test ensures that events use 'payload' key instead of 'type'
    // The schema enforces this, so we just need to verify the validation passes
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add an event with payload
    manifest.events.push(Event {
        name: "TestEvent".to_string(),
        payload: Some(TypeRef::string()),
    });

    // This should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_variant_payload_structure() {
    // This test ensures that variants use 'payload' key instead of 'type'
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add a variant type with payload
    let _ = manifest.types.insert(
        "TestVariant".to_string(),
        TypeDef::Variant {
            variants: vec![Variant {
                name: "TestVariant".to_string(),
                code: None,
                payload: Some(TypeRef::string()),
            }],
        },
    );

    // This should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_error_payload_structure() {
    // This test ensures that errors use 'payload' key instead of 'type'
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add a method with error that has payload
    manifest.methods.push(Method {
        name: "test_method".to_string(),
        params: vec![],
        returns: None,
        returns_nullable: None,
        errors: vec![Error {
            code: "TEST_ERROR".to_string(),
            payload: Some(TypeRef::string()),
        }],
    });

    // This should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_variable_bytes_no_size() {
    // This test ensures that variable bytes don't have size=0
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add a method with variable bytes (no size)
    manifest.methods.push(Method {
        name: "test_method".to_string(),
        params: vec![],
        returns: Some(TypeRef::Scalar(
            calimero_wasm_abi::schema::ScalarType::Bytes {
                size: None,
                encoding: None,
            },
        )),
        returns_nullable: None,
        errors: vec![],
    });

    // This should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_map_string_key() {
    // This test ensures that map keys are string
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add a method with map that has string key
    manifest.methods.push(Method {
        name: "test_method".to_string(),
        params: vec![],
        returns: Some(TypeRef::Collection {
            collection: calimero_wasm_abi::schema::CollectionType::Map {
                key: Box::new(TypeRef::Scalar(
                    calimero_wasm_abi::schema::ScalarType::String,
                )),
                value: Box::new(TypeRef::u32()),
            },
            crdt_type: None,
            inner_type: None,
        }),
        returns_nullable: None,
        errors: vec![],
    });

    // This should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_no_dangling_refs() {
    // This test ensures that all referenced types exist
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add a type definition
    let _ = manifest
        .types
        .insert("TestType".to_string(), TypeDef::Record { fields: vec![] });

    // Add a method that references the type
    manifest.methods.push(Method {
        name: "test_method".to_string(),
        params: vec![],
        returns: Some(TypeRef::Reference {
            ref_: "TestType".to_string(),
        }),
        returns_nullable: None,
        errors: vec![],
    });

    // This should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invariant_detects_dangling_refs() {
    // This test ensures that dangling refs are detected
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add a method that references a non-existent type
    manifest.methods.push(Method {
        name: "test_method".to_string(),
        params: vec![],
        returns: Some(TypeRef::Reference {
            ref_: "NonExistentType".to_string(),
        }),
        returns_nullable: None,
        errors: vec![],
    });

    // This should fail validation
    let result = validate_manifest(&manifest);
    assert!(result.is_err());
    match result.unwrap_err() {
        calimero_wasm_abi::validate::ValidationError::InvalidTypeReference { ref_name, path } => {
            assert_eq!(ref_name, "NonExistentType");
            assert_eq!(path, "method test_method.returns");
        }
        _ => panic!("Expected InvalidTypeReference error"),
    }
}

#[test]
fn test_invariant_deterministic_ordering() {
    // This test ensures that methods and events are sorted deterministically
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add methods in unsorted order
    manifest.methods.push(Method {
        name: "z_method".to_string(),
        params: vec![],
        returns: None,
        returns_nullable: None,
        errors: vec![],
    });
    manifest.methods.push(Method {
        name: "a_method".to_string(),
        params: vec![],
        returns: None,
        returns_nullable: None,
        errors: vec![],
    });

    // This should fail validation because methods are not sorted
    let result = validate_manifest(&manifest);
    assert!(result.is_err());
    match result.unwrap_err() {
        calimero_wasm_abi::validate::ValidationError::MethodsNotSorted { first, second } => {
            assert_eq!(first, "z_method");
            assert_eq!(second, "a_method");
        }
        _ => panic!("Expected MethodsNotSorted error"),
    }

    // Now sort the methods
    manifest.methods.sort_by(|a, b| a.name.cmp(&b.name));

    // This should pass validation
    assert!(validate_manifest(&manifest).is_ok());
}
