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

/// Mergeable struct from module (tests CRDT in modules)
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct MergeableRecord {
    pub counter: Counter,
    pub name: LwwRegister<String>,
}

impl calimero_storage::collections::Mergeable for MergeableRecord {
    fn merge(
        &mut self,
        other: &Self,
    ) -> Result<(), calimero_storage::collections::crdt_meta::MergeError> {
        self.counter.merge(&other.counter)?;
        self.name.merge(&other.name);
        Ok(())
    }
}

// `RekeyTarget` is a supertrait of `Mergeable`: a hand-written `impl Mergeable`
// must also implement `RekeyTarget` (`#[derive(Mergeable)]` generates both). The
// `counter` field is a collection whose nested id is re-keyed deterministically
// here; `name` (`LwwRegister`) is a leaf, for which the macro dispatches to a
// no-op.
impl calimero_storage::collections::rekey::RekeyTarget for MergeableRecord {
    fn rekey_relative_to(&mut self, parent_id: calimero_storage::address::Id) {
        use calimero_storage::collections::rekey::field_child_id;
        calimero_storage::rekey_field_if_supported!(
            &mut self.counter,
            field_child_id(parent_id, "counter")
        );
        calimero_storage::rekey_field_if_supported!(
            &mut self.name,
            field_child_id(parent_id, "name")
        );
    }
}
