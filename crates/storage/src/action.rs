//! Synchronization action types for CRDT operations.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::Id;
use crate::entities::{ChildInfo, Metadata};

/// Actions to be taken during synchronisation.
///
/// The following variants represent the possible actions arising from either a
/// direct change or a comparison between two nodes.
///
///   - **Direct change**: When a direct change is made, in other words, when
///     there is local activity that results in data modification to propagate
///     to other nodes, the possible resulting actions are [`Add`](Action::Add),
///     [`DeleteRef`](Action::DeleteRef), and [`Update`](Action::Update). A comparison
///     is not needed in this case, as the deltas are known, and assuming all of
///     the actions are carried out, the nodes will be in sync.
///
///   - **Comparison**: When a comparison is made between two nodes, the
///     possible resulting actions are [`Add`](Action::Add), [`DeleteRef`](Action::DeleteRef),
///     [`Update`](Action::Update), and [`Compare`](Action::Compare). The extra
///     comparison action arises in the case of tree traversal, where a child
///     entity is found to differ between the two nodes. In this case, the child
///     entity is compared, and the resulting actions are added to the list of
///     actions to be taken. This process is recursive.
///
/// Note: Some actions contain the full entity, and not just the entity ID, as
/// the actions will often be in context of data that is not available locally
/// and cannot be sourced otherwise. The actions are stored in serialised form
/// because of type restrictions, and they are due to be sent around the network
/// anyway.
///
/// Note: This enum contains the entity type, for passing to the guest for
/// processing along with the ID and data.
///
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[expect(clippy::exhaustive_enums, reason = "Exhaustive")]
pub enum Action {
    /// Add an entity with the given ID, type, and data.
    Add {
        /// Unique identifier of the entity.
        id: Id,

        /// Serialised data of the entity.
        data: Vec<u8>,

        /// Details of the ancestors of the entity.
        ancestors: Vec<ChildInfo>,

        /// The metadata of the entity.
        metadata: Metadata,
    },

    /// Compare the entity with the given ID and type. Note that this results in
    /// a direct comparison of the specific entity in question, including data
    /// that is immediately available to it, such as the hashes of its children.
    /// This may well result in further actions being generated if children
    /// differ, leading to a recursive comparison.
    Compare {
        /// Unique identifier of the entity.
        id: Id,
    },

    /// Delete reference (tombstone-based deletion).
    ///
    /// More efficient than Delete variant - only sends ID and timestamp.
    /// Uses tombstone mechanism for proper CRDT semantics:
    /// - Handles delete vs update conflicts via timestamp comparison
    /// - Supports out-of-order message delivery
    /// - Enables 1-day retention + full resync strategy
    DeleteRef {
        /// Unique identifier of the entity to delete.
        id: Id,

        /// Timestamp when deletion occurred (for conflict resolution).
        deleted_at: u64,
    },

    /// Update the entity with the given ID and type to have the supplied data.
    Update {
        /// Unique identifier of the entity.
        id: Id,

        /// Serialised data of the entity.
        data: Vec<u8>,

        /// Details of the ancestors of the entity.
        ancestors: Vec<ChildInfo>,

        /// The metadata of the entity.
        metadata: Metadata,
    },
}

/// Comparison data for tree synchronization.
///
/// Contains entity metadata needed for Merkle tree comparison.
/// Used to determine if entities differ without transferring full data.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ComparisonData {
    /// Entity ID.
    pub id: Id,

    /// Full Merkle hash (entity + all descendants).
    pub full_hash: [u8; 32],

    /// Own hash (entity data only, excluding descendants).
    pub own_hash: [u8; 32],

    /// Children organized by collection name.
    ///
    /// Each collection maps to a vector of child metadata (ID, hash, timestamp).
    /// Used for recursive tree comparison.
    pub children: BTreeMap<String, Vec<ChildInfo>>,

    /// Ancestors of the entity.
    pub ancestors: Vec<ChildInfo>,

    /// Metadata of the entity.
    pub metadata: Metadata,
}
