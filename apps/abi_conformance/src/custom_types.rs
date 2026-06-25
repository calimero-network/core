//! Custom types module for testing multi-file ABI generation

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_storage::collections::{Counter, LwwRegister};

/// A custom struct defined in a separate module
/// This tests that the ABI generator can discover types from module files
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct CustomRecord {
    /// A string field
    pub name: String,
    /// A numeric counter
    pub value: u64,
    /// A flag
    pub active: bool,
}

/// Another custom type to test nested references
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct NestedRecord {
    /// Reference to CustomRecord
    pub record: CustomRecord,
    /// A list of strings
    pub tags: Vec<String>,
}

/// Custom enum in module
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub enum Status {
    Pending,
    Active { timestamp: u64 },
    Completed { result: String },
}

/// Mergeable struct from module (tests CRDT in modules).
///
/// `#[derive(Mergeable)]` generates the field-by-field merge AND the matching
/// `RekeyTarget` impl (re-keying the `counter` collection, no-op for the
/// `LwwRegister` leaf) — no hand-written `RekeyTarget` boilerplate.
#[derive(Debug, BorshSerialize, BorshDeserialize, calimero_storage::collections::Mergeable)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct MergeableRecord {
    pub counter: Counter,
    pub name: LwwRegister<String>,
}
