use calimero_wasm_abi::emitter::emit_manifest;
use calimero_wasm_abi::schema::TypeRef;
use syn::parse_file;

#[test]
#[ignore = "Emitter functionality not fully implemented in simplified version"]
fn test_abi_conformance_emitter() {
    // Parse the abi_conformance lib.rs file
    let source_code = r#"
use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use thiserror::Error;

// Newtype bytes
#[derive(
    Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct UserId32([u8; 32]);

// Records
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Person {
    id: UserId32,
    name: String,
    age: u32,
}

// Variants
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum Action {
    Ping,
    SetName(String),
    Update { age: u32 },
}

// Events
#[app::event]
pub enum Event {
    Ping,
    Named(String),
    Data(Vec<u8>),
    PersonUpdated(Person),
    ActionTaken(Action),
}

// State
#[app::state(emits = Event)]
#[derive(Debug, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AbiState {
    counters: BTreeMap<String, u32>,
    users: Vec<UserId32>,
}

#[app::logic]
impl AbiState {
    // Lists
    pub fn list_records(ps: Vec<Person>) -> Vec<Person> {
        ps
    }

    pub fn list_ids(xs: Vec<UserId32>) -> Vec<UserId32> {
        xs
    }

    // Maps
    pub fn map_record(m: BTreeMap<String, Person>) -> BTreeMap<String, Person> {
        m
    }
}
"#;

    let _file = parse_file(source_code).expect("Failed to parse source code");
    let manifest = emit_manifest(source_code).expect("Failed to emit manifest");

    // Test list_records method
    let list_records = manifest
        .methods
        .iter()
        .find(|m| m.name == "list_records")
        .expect("list_records method not found");

    assert_eq!(list_records.params.len(), 1);
    let ps_param = &list_records.params[0];
    assert_eq!(ps_param.name, "ps");

    // Check that ps parameter is a list of Person references
    match &ps_param.type_ {
        TypeRef::Collection(collection) => match collection {
            calimero_wasm_abi::schema::CollectionType::List { items } => match &**items {
                TypeRef::Reference { ref_ } => {
                    assert_eq!(ref_, "Person");
                }
                _ => panic!("Expected Person reference in list items"),
            },
            _ => panic!("Expected list collection type"),
        },
        _ => panic!("Expected collection type for ps parameter"),
    }

    // Check return type is also a list of Person references
    let returns = list_records.returns.as_ref().expect("Expected return type");
    match returns {
        TypeRef::Collection(collection) => match collection {
            calimero_wasm_abi::schema::CollectionType::List { items } => match &**items {
                TypeRef::Reference { ref_ } => {
                    assert_eq!(ref_, "Person");
                }
                _ => panic!("Expected Person reference in return list items"),
            },
            _ => panic!("Expected list collection type in return"),
        },
        _ => panic!("Expected collection type for return"),
    }

    // Test list_ids method
    let list_ids = manifest
        .methods
        .iter()
        .find(|m| m.name == "list_ids")
        .expect("list_ids method not found");

    assert_eq!(list_ids.params.len(), 1);
    let xs_param = &list_ids.params[0];
    assert_eq!(xs_param.name, "xs");

    // Check that xs parameter is a list of UserId32 references
    match &xs_param.type_ {
        TypeRef::Collection(collection) => match collection {
            calimero_wasm_abi::schema::CollectionType::List { items } => match &**items {
                TypeRef::Reference { ref_ } => {
                    assert_eq!(ref_, "UserId32");
                }
                _ => panic!("Expected UserId32 reference in list items"),
            },
            _ => panic!("Expected list collection type"),
        },
        _ => panic!("Expected collection type for xs parameter"),
    }

    // Test map_record method
    let map_record = manifest
        .methods
        .iter()
        .find(|m| m.name == "map_record")
        .expect("map_record method not found");

    assert_eq!(map_record.params.len(), 1);
    let m_param = &map_record.params[0];
    assert_eq!(m_param.name, "m");

    // Check that m parameter is a map with string key and Person value
    match &m_param.type_ {
        TypeRef::Collection(collection) => {
            match collection {
                calimero_wasm_abi::schema::CollectionType::Map { key, value } => {
                    // Check key is string
                    match &**key {
                        TypeRef::Scalar(scalar) => match scalar {
                            calimero_wasm_abi::schema::ScalarType::String => {}
                            _ => panic!("Expected string key type"),
                        },
                        _ => panic!("Expected scalar key type"),
                    }

                    // Check value is Person reference
                    match &**value {
                        TypeRef::Reference { ref_ } => {
                            assert_eq!(ref_, "Person");
                        }
                        _ => panic!("Expected Person reference in map value"),
                    }
                }
                _ => panic!("Expected map collection type"),
            }
        }
        _ => panic!("Expected collection type for m parameter"),
    }

    // Test events
    let person_updated = manifest
        .events
        .iter()
        .find(|e| e.name == "PersonUpdated")
        .expect("PersonUpdated event not found");

    // Check that PersonUpdated has Person payload
    let payload = person_updated.payload.as_ref().expect("Expected payload");
    match payload {
        TypeRef::Reference { ref_ } => {
            assert_eq!(ref_, "Person");
        }
        _ => panic!("Expected Person reference in PersonUpdated payload"),
    }

    let action_taken = manifest
        .events
        .iter()
        .find(|e| e.name == "ActionTaken")
        .expect("ActionTaken event not found");

    // Check that ActionTaken has Action payload
    let payload = action_taken.payload.as_ref().expect("Expected payload");
    match payload {
        TypeRef::Reference { ref_ } => {
            assert_eq!(ref_, "Action");
        }
        _ => panic!("Expected Action reference in ActionTaken payload"),
    }

    // Test that other events are unchanged
    let ping = manifest
        .events
        .iter()
        .find(|e| e.name == "Ping")
        .expect("Ping event not found");
    assert!(ping.payload.is_none());

    let named = manifest
        .events
        .iter()
        .find(|e| e.name == "Named")
        .expect("Named event not found");
    match &named.payload {
        Some(TypeRef::Scalar(scalar)) => match scalar {
            calimero_wasm_abi::schema::ScalarType::String => {}
            _ => panic!("Expected string payload for Named event"),
        },
        _ => panic!("Expected scalar payload for Named event"),
    }
}
