use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use thiserror::Error;

// Test multi-file ABI generation
pub mod custom_types;
pub use custom_types::{CustomRecord, NestedRecord, Status};

// Include the generated ABI code
include!(env!("GENERATED_ABI_PATH"));

// Newtype bytes
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct UserId32([u8; 32]);

// Note: [u8; 64] doesn't implement Serialize/Deserialize, so we'll use Vec<u8> for Hash64
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Hash64(Vec<u8>);

// Records
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Person {
    id: UserId32,
    name: String,
    age: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Profile {
    bio: Option<String>,
    avatar: Option<Vec<u8>>,
    nicknames: Vec<String>,
}

// Update payload type
#[derive(Clone, Copy, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct UpdatePayload {
    age: u32,
}

// Variants
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum Action {
    Ping,
    SetName(String),
    Update(UpdatePayload),
    MultiTuple(u32, String), // Tuple variant with multiple unnamed fields
    MultiStruct { x: u32, y: String }, // Struct variant with multiple named fields
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum ConformanceError {
    #[error("bad input")]
    BadInput,
    #[error("not found: {0}")]
    NotFound(String),
}

// Events
#[app::event]
#[derive(Debug)]
pub enum Event {
    Ping,
    Named(String),
    Data(Vec<u8>),
    PersonUpdated(Person),
    ActionTaken(Action),
    TupleEvent(u32, String), // Tuple variant with multiple unnamed fields
    StructEvent { id: u32, name: String }, // Struct variant with multiple named fields
}

// State
#[app::state(emits = Event)]
#[derive(Debug, PartialEq, Eq, PartialOrd, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AbiState {
    counters: BTreeMap<String, u32>, // map<string,u32>
    users: Vec<UserId32>,            // list<UserId32>
}

// Implementation
#[app::logic]
impl AbiState {
    #[app::init]
    #[must_use]
    pub const fn init() -> Self {
        Self {
            counters: BTreeMap::new(),
            users: Vec::new(),
        }
    }

    // Unit return
    pub const fn noop() {}

    // Scalar types
    #[must_use]
    pub const fn echo_bool(b: bool) -> bool {
        b
    }

    #[must_use]
    pub const fn echo_i32(x: i32) -> i32 {
        x
    }

    #[must_use]
    pub const fn echo_i64(x: i64) -> i64 {
        x
    }

    #[must_use]
    pub const fn echo_u32(x: u32) -> u32 {
        x
    }

    #[must_use]
    pub const fn echo_u64(x: u64) -> u64 {
        x
    }

    #[must_use]
    pub const fn echo_f32(x: f32) -> f32 {
        x
    }

    #[must_use]
    pub const fn echo_f64(x: f64) -> f64 {
        x
    }

    #[must_use]
    pub const fn echo_string(s: String) -> String {
        s
    }

    #[must_use]
    pub const fn echo_bytes(b: Vec<u8>) -> Vec<u8> {
        b
    }

    // Optionals
    #[must_use]
    pub const fn opt_u32(x: Option<u32>) -> Option<u32> {
        x
    }

    #[must_use]
    pub const fn opt_string(x: Option<String>) -> Option<String> {
        x
    }

    #[must_use]
    pub const fn opt_record(p: Option<Person>) -> Option<Person> {
        p
    }

    #[must_use]
    pub const fn opt_id(x: Option<UserId32>) -> Option<UserId32> {
        x
    }

    // Lists
    #[must_use]
    pub const fn list_u32(xs: Vec<u32>) -> Vec<u32> {
        xs
    }

    #[must_use]
    pub const fn list_strings(xs: Vec<String>) -> Vec<String> {
        xs
    }

    #[must_use]
    pub const fn list_records(ps: Vec<Person>) -> Vec<Person> {
        ps
    }

    #[must_use]
    pub const fn list_ids(xs: Vec<UserId32>) -> Vec<UserId32> {
        xs
    }

    // Maps (string key only)
    #[must_use]
    pub const fn map_u32(m: BTreeMap<String, u32>) -> BTreeMap<String, u32> {
        m
    }

    #[must_use]
    pub const fn map_list_u32(m: BTreeMap<String, Vec<u32>>) -> BTreeMap<String, Vec<u32>> {
        m
    }

    #[must_use]
    pub const fn map_record(m: BTreeMap<String, Person>) -> BTreeMap<String, Person> {
        m
    }

    // Records
    #[must_use]
    pub const fn make_person(p: Person) -> Person {
        p
    }

    #[must_use]
    pub const fn profile_roundtrip(p: Profile) -> Profile {
        p
    }

    // Variants
    #[must_use]
    pub fn act(a: Action) -> u32 {
        match a {
            Action::Ping => 1,
            Action::SetName(_) => 2,
            Action::Update(payload) => payload.age,
            Action::MultiTuple(x, _) => x,
            Action::MultiStruct { x, .. } => x,
        }
    }

    // Test methods for enum variants with multiple fields
    #[must_use]
    pub fn handle_multi_tuple(a: Action) -> String {
        match a {
            Action::MultiTuple(_, s) => s,
            _ => String::new(),
        }
    }

    #[must_use]
    pub fn handle_multi_struct(a: Action) -> u32 {
        match a {
            Action::MultiStruct { x, .. } => x,
            _ => 0,
        }
    }

    // Newtype bytes
    #[must_use]
    pub const fn roundtrip_id(x: UserId32) -> UserId32 {
        x
    }

    #[must_use]
    pub const fn roundtrip_hash(h: Hash64) -> Hash64 {
        h
    }

    // Errors
    pub const fn may_fail(flag: bool) -> Result<u32, ConformanceError> {
        if flag {
            Ok(42)
        } else {
            Err(ConformanceError::BadInput)
        }
    }

    pub fn find_person(name: String) -> Result<Person, ConformanceError> {
        if name.is_empty() {
            Err(ConformanceError::NotFound("empty name".to_owned()))
        } else {
            Ok(Person {
                id: UserId32([0; 32]),
                name,
                age: 25,
            })
        }
    }

    // Test case: public method that calls a private method
    #[must_use]
    pub const fn public_with_private_helper(value: u32) -> u32 {
        Self::private_helper(value)
    }

    // Test case: public method that returns a type using internal struct
    #[must_use]
    pub const fn get_internal_result(value: u32) -> InternalResult {
        let internal_data = InternalData {
            value,
            multiplier: 3,
        };
        InternalResult {
            original: value,
            calculated: internal_data.calculate(),
        }
    }

    // Test methods using types from custom_types module
    // This verifies multi-file ABI generation works

    /// Create a custom record from module
    pub fn create_custom_record(&self, name: String, value: u64) -> app::Result<CustomRecord> {
        Ok(CustomRecord {
            name,
            value,
            active: true,
        })
    }

    /// Get a nested record from module
    pub fn get_nested_record(&self, name: String) -> app::Result<NestedRecord> {
        Ok(NestedRecord {
            record: CustomRecord {
                name: name.clone(),
                value: 42,
                active: true,
            },
            tags: vec!["test".to_owned(), "multi-file".to_owned()],
        })
    }

    /// Get status from module
    pub fn get_status(&self, timestamp: u64) -> app::Result<Status> {
        Ok(Status::Active { timestamp })
    }

    // Private method - should NOT appear in ABI
    const fn private_helper(value: u32) -> u32 {
        let internal_data = InternalData {
            value,
            multiplier: 2,
        };
        internal_data.calculate()
    }
}

// Internal struct - should NOT appear in ABI since it's only used in private methods
#[derive(Debug)]
struct InternalData {
    value: u32,
    multiplier: u32,
}

impl InternalData {
    const fn calculate(&self) -> u32 {
        self.value * self.multiplier
    }
}

// Public struct that uses internal struct - SHOULD appear in ABI
#[derive(Debug, Clone, Copy, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct InternalResult {
    original: u32,
    calculated: u32,
}
