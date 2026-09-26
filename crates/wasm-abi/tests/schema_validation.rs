use calimero_wasm_abi::schema::{
    Event, Field, Manifest, Method, MethodIntent, Parameter, TypeDef, TypeRef, Variant,
};
use jsonschema::validator_for;
use serde_json::{json, Value};

#[test]
fn test_schema_validation_basic() {
    // Load the schema
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

    // Create a basic manifest
    let mut manifest = calimero_wasm_abi::schema::Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    // Add a simple method
    manifest.methods.push(calimero_wasm_abi::schema::Method {
        name: "test_method".to_string(),
        returns: Some(calimero_wasm_abi::schema::TypeRef::u32()),
        ..Default::default()
    });

    // Serialize to JSON
    let manifest_json = serde_json::to_value(&manifest).unwrap();

    // Validate against schema
    let validation_result = schema.validate(&manifest_json);
    assert!(
        validation_result.is_ok(),
        "Schema validation failed: {:?}",
        validation_result.err()
    );
}

#[test]
fn test_schema_validation_migration_edges() {
    // A manifest carrying state_version + migration edges must validate: the
    // root is `additionalProperties: false`, so the schema would reject the
    // fields unless it describes them (the gap this guards).
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

    let manifest = calimero_wasm_abi::schema::Manifest {
        schema_version: "wasm-abi/1".to_string(),
        state_version: Some(2),
        migrations: vec![calimero_wasm_abi::schema::MigrationEdgeAbi {
            method: "migrate_v1_to_v2".to_string(),
            from_version: 1,
        }],
        ..Default::default()
    };

    let manifest_json = serde_json::to_value(&manifest).unwrap();
    // Serializes as camelCase `fromVersion` — the schema must match.
    assert_eq!(manifest_json["migrations"][0]["fromVersion"], 1);

    let validation_result = schema.validate(&manifest_json);
    assert!(
        validation_result.is_ok(),
        "edge-bearing manifest must validate against wasm-abi.schema.json: {:?}",
        validation_result.err()
    );
}

#[test]
fn test_schema_validation_shared_storage_crdt_type() {
    use calimero_wasm_abi::schema::{
        CollectionType, CrdtCollectionType, Manifest, Method, TypeRef,
    };

    // Load the crate's own JSON Schema.
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

    // A `SharedStorage<String>` field normalizes to a single-slot Record
    // collection carrying `crdt_type: shared_storage`. The published JSON Schema
    // must accept that string, or every manifest with a SharedStorage field fails
    // validation against the crate's own schema.
    let shared = TypeRef::Collection {
        collection: CollectionType::Record { fields: vec![] },
        crdt_type: Some(CrdtCollectionType::SharedStorage),
        inner_type: Some(Box::new(TypeRef::string())),
    };
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };
    manifest.methods.push(Method {
        name: "shared".to_string(),
        returns: Some(shared),
        ..Default::default()
    });

    let manifest_json = serde_json::to_value(&manifest).unwrap();
    let validation_result = schema.validate(&manifest_json);
    assert!(
        validation_result.is_ok(),
        "SharedStorage crdt_type must validate against wasm-abi.schema.json: {:?}",
        validation_result.err()
    );
}

#[test]
fn test_schema_validation_conformance() {
    // Load the schema
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

    // Load the conformance manifest
    let conformance_json = include_str!("../../../apps/abi_conformance/abi.expected.json");
    let conformance_value: Value = serde_json::from_str(conformance_json).unwrap();

    // Validate against schema
    let validation_result = schema.validate(&conformance_value);
    assert!(
        validation_result.is_ok(),
        "Conformance manifest validation failed: {:?}",
        validation_result.err()
    );
}

#[test]
fn test_schema_validation_bytes_types() {
    // Load the schema
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

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
        validation_result.err()
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
        validation_result.err()
    );
}

#[test]
fn test_schema_validation_map_keys() {
    // Load the schema
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

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
        validation_result.err()
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
    let schema = validator_for(&schema_value).unwrap();

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
        validation_result.err()
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
        validation_result.err()
    );
}

#[test]
fn test_schema_validation_tuple() {
    // `wasm-abi.schema.json` is hand-maintained, so a new CollectionType is only
    // really shipped once the mirror describes it. `additionalProperties: false`
    // on each collection branch means an undescribed `tuple` is rejected here.
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

    let mut manifest = calimero_wasm_abi::schema::Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };
    manifest.methods.push(calimero_wasm_abi::schema::Method {
        name: "sorted_scores_range".to_string(),
        returns: Some(calimero_wasm_abi::schema::TypeRef::list(
            calimero_wasm_abi::schema::TypeRef::tuple(vec![
                calimero_wasm_abi::schema::TypeRef::string(),
                calimero_wasm_abi::schema::TypeRef::u64(),
            ]),
        )),
        intent: MethodIntent::ReadOnly,
        ..Default::default()
    });

    let manifest_json = serde_json::to_value(&manifest).unwrap();
    let validation_result = schema.validate(&manifest_json);
    assert!(
        validation_result.is_ok(),
        "Schema validation failed: {:?}",
        validation_result.err()
    );
}

#[test]
fn test_schema_validation_doc_on_every_object() {
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

    let doc = || Some("Documented.".to_owned());
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };
    let field = Field {
        name: "a".to_owned(),
        type_: TypeRef::u32(),
        nullable: None,
        doc: doc(),
    };
    let _ = manifest.types.insert(
        "R".to_owned(),
        TypeDef::Record {
            doc: doc(),
            fields: vec![field],
        },
    );
    let _ = manifest.types.insert(
        "V".to_owned(),
        TypeDef::Variant {
            doc: doc(),
            variants: vec![Variant {
                name: "A".to_owned(),
                code: None,
                payload: None,
                doc: doc(),
            }],
        },
    );
    let _ = manifest.types.insert(
        "B".to_owned(),
        TypeDef::Bytes {
            doc: doc(),
            size: Some(32),
            encoding: None,
        },
    );
    let _ = manifest.types.insert(
        "L".to_owned(),
        TypeDef::Alias {
            doc: doc(),
            target: TypeRef::string(),
            pattern: None,
        },
    );
    manifest.methods.push(Method {
        name: "m".to_owned(),
        doc: doc(),
        params: vec![Parameter {
            name: "p".to_owned(),
            type_: TypeRef::reference("R"),
            nullable: None,
            doc: doc(),
        }],
        returns_doc: doc(),
        destructive: true,
        idempotent: true,
        ..Default::default()
    });
    manifest.events.push(Event {
        name: "E".to_owned(),
        payload: None,
        doc: doc(),
    });

    let manifest_json = serde_json::to_value(&manifest).unwrap();
    let validation_result = schema.validate(&manifest_json);
    assert!(
        validation_result.is_ok(),
        "doc must be accepted on every documented object: {:?}",
        validation_result.err()
    );
}

#[test]
fn test_schema_validation_doc_does_not_loosen_other_keys() {
    // `doc` is one more described key, not a relaxed `additionalProperties`: a
    // misspelt key, a non-string doc, and a doc on an inline type reference all fail.
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

    let rejected = [
        json!({"schema_version":"wasm-abi/1","types":{},"events":[],
               "methods":[{"name":"m","params":[],"docs":"x"}]}),
        json!({"schema_version":"wasm-abi/1","types":{},"events":[],
               "methods":[{"name":"m","params":[],"doc":7}]}),
        json!({"schema_version":"wasm-abi/1","types":{},"events":[],
               "methods":[{"name":"m","params":[{"name":"p","type":{"kind":"u32","doc":"x"}}]}]}),
        json!({"schema_version":"wasm-abi/1","types":{},"events":[],
               "methods":[{"name":"m","params":[{"name":"p","type":{"kind":"bytes","doc":"x"}}]}]}),
        json!({"schema_version":"wasm-abi/1","types":{"R":{"kind":"record","fields":[],"docs":"x"}},
               "methods":[],"events":[]}),
    ];
    for manifest in rejected {
        assert!(
            schema.validate(&manifest).is_err(),
            "must be rejected: {manifest}"
        );
    }
}

#[test]
fn test_schema_validation_method_hints() {
    let schema_json = include_str!("../wasm-abi.schema.json");
    let schema_value: Value = serde_json::from_str(schema_json).unwrap();
    let schema = validator_for(&schema_value).unwrap();

    let manifest = |method: Value| json!({"schema_version":"wasm-abi/1","types":{},"methods":[method],"events":[]});
    let accepted = manifest(json!({"name":"wipe","params":[],"returns":{"kind":"u32"},
        "returns_doc":"How many entries were removed.","destructive":true,"idempotent":true}));
    assert!(
        schema.validate(&accepted).is_ok(),
        "{:?}",
        schema.validate(&accepted).err()
    );
    for rejected in [
        json!({"name":"wipe","params":[],"destructive":"yes"}),
        json!({"name":"wipe","params":[],"idempotent":1}),
        json!({"name":"wipe","params":[],"returns_doc":7}),
        json!({"name":"wipe","params":[],"destructive_hint":true}),
    ] {
        assert!(
            schema.validate(&manifest(rejected.clone())).is_err(),
            "{rejected}"
        );
    }
}
