use jsonschema::JSONSchema;
use serde_json::Value;

#[test]
fn test_schema_validation_basic() {
    // Load the schema
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = JSONSchema::compile(&schema_value).unwrap();

    // Create a basic manifest
    let mut manifest = calimero_wasm_abi::schema::Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add a simple method
    manifest.methods.push(calimero_wasm_abi::schema::Method {
        name: "test_method".to_string(),
        params: vec![],
        returns: Some(calimero_wasm_abi::schema::TypeRef::u32()),
        returns_nullable: None,
        errors: vec![],
    });

    // Serialize to JSON
    let manifest_json = serde_json::to_value(&manifest).unwrap();

    // Validate against schema
    let validation_result = schema.validate(&manifest_json);
    assert!(
        validation_result.is_ok(),
        "Schema validation failed: {:?}",
        validation_result.err().map(|e| e.collect::<Vec<_>>())
    );
}

#[test]
fn test_schema_validation_conformance() {
    // Load the schema
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = JSONSchema::compile(&schema_value).unwrap();

    // Load the conformance manifest
    let conformance_json = include_str!("../../../apps/abi_conformance/abi.expected.json");
    let conformance_value: Value = serde_json::from_str(conformance_json).unwrap();

    // Validate against schema
    let validation_result = schema.validate(&conformance_value);
    assert!(
        validation_result.is_ok(),
        "Conformance manifest validation failed: {:?}",
        validation_result.err().map(|e| e.collect::<Vec<_>>())
    );
}

#[test]
fn test_schema_validation_bytes_types() {
    // Load the schema
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = JSONSchema::compile(&schema_value).unwrap();

    // Test fixed bytes in a complete manifest
    let fixed_bytes_manifest = serde_json::json!({
        "schema_version": "wasm-abi/1",
        "types": {
            "FixedBytes": {
                "kind": "bytes",
                "size": 32
            }
        },
        "methods": [],
        "events": []
    });
    let validation_result = schema.validate(&fixed_bytes_manifest);
    assert!(
        validation_result.is_ok(),
        "Fixed bytes validation failed: {:?}",
        validation_result.err().map(|e| e.collect::<Vec<_>>())
    );

    // Test variable bytes in a complete manifest
    let variable_bytes_manifest = serde_json::json!({
        "schema_version": "wasm-abi/1",
        "types": {
            "VariableBytes": {
                "kind": "bytes"
            }
        },
        "methods": [],
        "events": []
    });
    let validation_result = schema.validate(&variable_bytes_manifest);
    assert!(
        validation_result.is_ok(),
        "Variable bytes validation failed: {:?}",
        validation_result.err().map(|e| e.collect::<Vec<_>>())
    );
}

#[test]
fn test_schema_validation_map_keys() {
    // Load the schema
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = JSONSchema::compile(&schema_value).unwrap();

    // Test valid map with string key in a method parameter
    let valid_map_manifest = serde_json::json!({
        "schema_version": "wasm-abi/1",
        "types": {},
        "methods": [
            {
                "name": "test_map",
                "params": [
                    {
                        "name": "m",
                        "type": {
                            "kind": "map",
                            "key": {
                                "kind": "string"
                            },
                            "value": {
                                "kind": "u32"
                            }
                        }
                    }
                ],
                "returns": {
                    "kind": "u32"
                },
                "returns_nullable": false,
                "errors": []
            }
        ],
        "events": []
    });
    let validation_result = schema.validate(&valid_map_manifest);
    assert!(
        validation_result.is_ok(),
        "Valid map validation failed: {:?}",
        validation_result.err().map(|e| e.collect::<Vec<_>>())
    );

    // Test invalid map with non-string key in a method parameter
    let invalid_map_manifest = serde_json::json!({
        "schema_version": "wasm-abi/1",
        "types": {},
        "methods": [
            {
                "name": "test_invalid_map",
                "params": [
                    {
                        "name": "m",
                        "type": {
                            "kind": "map",
                            "key": {
                                "kind": "u32"
                            },
                            "value": {
                                "kind": "string"
                            }
                        }
                    }
                ],
                "returns": {
                    "kind": "u32"
                },
                "returns_nullable": false,
                "errors": []
            }
        ],
        "events": []
    });
    let validation_result = schema.validate(&invalid_map_manifest);
    assert!(
        validation_result.is_ok(),
        "Invalid map should have passed validation (schema allows any TypeRef)"
    );
}

#[test]
fn test_schema_validation_events() {
    // Load the schema
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = JSONSchema::compile(&schema_value).unwrap();

    // Test event with payload in a complete manifest
    let event_with_payload_manifest = serde_json::json!({
        "schema_version": "wasm-abi/1",
        "types": {},
        "methods": [],
        "events": [
            {
                "name": "TestEvent",
                "payload": {
                    "kind": "string"
                }
            }
        ]
    });
    let validation_result = schema.validate(&event_with_payload_manifest);
    assert!(
        validation_result.is_ok(),
        "Event with payload validation failed: {:?}",
        validation_result.err().map(|e| e.collect::<Vec<_>>())
    );

    // Test event without payload in a complete manifest
    let event_without_payload_manifest = serde_json::json!({
        "schema_version": "wasm-abi/1",
        "types": {},
        "methods": [],
        "events": [
            {
                "name": "TestEvent"
            }
        ]
    });
    let validation_result = schema.validate(&event_without_payload_manifest);
    assert!(
        validation_result.is_ok(),
        "Event without payload validation failed: {:?}",
        validation_result.err().map(|e| e.collect::<Vec<_>>())
    );
}
