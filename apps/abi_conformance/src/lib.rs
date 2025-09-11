use std::collections::BTreeMap;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use thiserror::Error;

// Include the generated ABI code
include!(env!("GENERATED_ABI_PATH"));

// Newtype bytes
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
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
    Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
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

// Events - now just a regular enum, no macro
#[derive(Debug)]
pub enum Event {
    Ping,
    Named(String),
    Data(Vec<u8>),
    PersonUpdated(Person),
    ActionTaken(Action),
}

// State - now just a regular struct, no macro
#[derive(Debug, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AbiState {
    counters: BTreeMap<String, u32>, // map<string,u32>
    users: Vec<UserId32>,            // list<UserId32>
}

// Expose AbiState as a public type for ABI
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct AbiStateExposed {
    counters: BTreeMap<String, u32>, // map<string,u32>
    users: Vec<UserId32>,            // list<UserId32>
}

// Implementation - now just a regular impl, no macro
impl AbiState {
    pub fn init() -> AbiState {
        AbiState {
            counters: BTreeMap::new(),
            users: Vec::new(),
        }
    }

    // Unit return
    pub fn noop() {}

    // Scalar types
    pub fn echo_bool(b: bool) -> bool {
        b
    }

    pub fn echo_i32(x: i32) -> i32 {
        x
    }

    pub fn echo_i64(x: i64) -> i64 {
        x
    }

    pub fn echo_u32(x: u32) -> u32 {
        x
    }

    pub fn echo_u64(x: u64) -> u64 {
        x
    }

    pub fn echo_f32(x: f32) -> f32 {
        x
    }

    pub fn echo_f64(x: f64) -> f64 {
        x
    }

    pub fn echo_string(s: String) -> String {
        s
    }

    pub fn echo_bytes(b: Vec<u8>) -> Vec<u8> {
        b
    }

    // Optionals
    pub fn opt_u32(x: Option<u32>) -> Option<u32> {
        x
    }

    pub fn opt_string(x: Option<String>) -> Option<String> {
        x
    }

    pub fn opt_record(p: Option<Person>) -> Option<Person> {
        p
    }

    pub fn opt_id(x: Option<UserId32>) -> Option<UserId32> {
        x
    }

    // Lists
    pub fn list_u32(xs: Vec<u32>) -> Vec<u32> {
        xs
    }

    pub fn list_strings(xs: Vec<String>) -> Vec<String> {
        xs
    }

    pub fn list_records(ps: Vec<Person>) -> Vec<Person> {
        ps
    }

    pub fn list_ids(xs: Vec<UserId32>) -> Vec<UserId32> {
        xs
    }

    // Maps (string key only)
    pub fn map_u32(m: BTreeMap<String, u32>) -> BTreeMap<String, u32> {
        m
    }

    pub fn map_list_u32(m: BTreeMap<String, Vec<u32>>) -> BTreeMap<String, Vec<u32>> {
        m
    }

    pub fn map_record(m: BTreeMap<String, Person>) -> BTreeMap<String, Person> {
        m
    }

    // Records
    pub fn make_person(p: Person) -> Person {
        p
    }

    pub fn profile_roundtrip(p: Profile) -> Profile {
        p
    }

    // Variants
    pub fn act(a: Action) -> u32 {
        match a {
            Action::Ping => 1,
            Action::SetName(_) => 2,
            Action::Update(payload) => payload.age,
        }
    }

    // Newtype bytes
    pub fn roundtrip_id(x: UserId32) -> UserId32 {
        x
    }

    pub fn roundtrip_hash(h: Hash64) -> Hash64 {
        h
    }

    // Errors
    pub fn may_fail(flag: bool) -> Result<u32, ConformanceError> {
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
}
