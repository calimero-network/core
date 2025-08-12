use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::app;
use std::collections::BTreeMap;
use thiserror::Error;

// Use the same pattern as plantr for UserId
mod types {
    pub mod id;
}

types::id::define!(pub UserId<32, 44>);

// Record
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[app::abi_type]
pub struct Person {
    id: UserId,
    name: String,
    age: u32,
}

// Variants
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[app::abi_type]
pub enum Action {
    Ping,
    SetName(String),
    Update { age: u32 },
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
pub enum Event {
    Ping,
    Named(String),
    Data(Vec<u8>),
    Updated(Person),
}

// State (record used by init)
#[app::state(emits = Event)]
#[derive(Debug, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AbiState {
    counters: BTreeMap<String, u32>,   // map<string,u32>
    users: Vec<UserId>,                // list<UserId>
}

// Expose AbiState as a public type for ABI
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[app::abi_type]
pub struct AbiStateExposed {
    counters: BTreeMap<String, u32>,   // map<string,u32>
    users: Vec<UserId>,                // list<UserId>
}

#[app::logic]
impl AbiState {
    #[app::init]
    pub fn init() -> AbiState {
        AbiState {
            counters: BTreeMap::new(),
            users: Vec::new(),
        }
    }

    // Scalars
    pub fn echo_scalars(b: bool, i32v: i32, u64v: u64, s: String) -> String {
        format!("bool:{}, i32:{}, u64:{}, string:{}", b, i32v, u64v, s)
    }

    // Optionals
    pub fn opt_number(x: Option<u32>) -> Option<u32> {
        x.map(|v| v + 1)
    }

    // Lists
    pub fn sum_i64(xs: Vec<i64>) -> i64 {
        xs.iter().sum()
    }

    // Maps (string key only)
    pub fn score_of(m: BTreeMap<String, u32>, who: String) -> Option<u32> {
        m.get(&who).copied()
    }

    // Records
    pub fn make_person(p: Person) -> Person {
        Person {
            id: p.id,
            name: format!("{}_modified", p.name),
            age: p.age + 1,
        }
    }

    // Variants
    pub fn act(a: Action) -> u32 {
        match a {
            Action::Ping => 1,
            Action::SetName(_) => 2,
            Action::Update { age } => age,
        }
    }

    // Bytes newtype
    pub fn roundtrip_id(id: UserId) -> UserId {
        id
    }

    // Errors
    pub fn may_fail(flag: bool) -> app::Result<u32, ConformanceError> {
        if flag {
            Ok(42)
        } else {
            Err(ConformanceError::BadInput)
        }
    }

    pub fn may_fail_not_found(name: String) -> app::Result<u32, ConformanceError> {
        if name.is_empty() {
            Err(ConformanceError::NotFound("empty name".to_string()))
        } else {
            Ok(name.len() as u32)
        }
    }
} 