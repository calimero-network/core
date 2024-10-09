//! Types used for integration with the runtime.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};

use crate::interface::ComparisonData;

/// Comparison data for synchronisation.
#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Debug,
    Deserialize,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
#[expect(clippy::exhaustive_structs, reason = "Exhaustive")]
pub struct Comparison {
    /// The type of the entity.
    pub type_id: u8,

    /// The serialised data of the entity.
    pub data: Vec<u8>,

    /// The comparison data for the entity.
    pub comparison_data: ComparisonData,
}
