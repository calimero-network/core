//! Synchronization action types for CRDT operations.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::entities::{ChildInfo, Metadata, StorageType};

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

        /// Metadata required for verification.
        metadata: Metadata,
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

impl Action {
    /// Helper to get ID from Action enum.
    pub fn id(&self) -> Id {
        match self {
            Action::Add { id, .. } => *id,
            Action::Update { id, .. } => *id,
            Action::DeleteRef { id, .. } => *id,
            Action::Compare { id, .. } => *id,
        }
    }

    /// Hash the bytes a writer commits to when signing this action.
    ///
    /// Scoped to assertions that are **transferable across tree-state
    /// boundaries**: id, data, nonce, signer, and storage-type access
    /// control. Tree-shape commitments (ancestor merkle hashes, full
    /// metadata) are deliberately omitted — they were redundant
    /// cryptographic packaging for what the apply layer already
    /// enforces structurally via [`AncestorIntegrity::verify`].
    ///
    /// Subtractive vs the v1 payload:
    /// * **Out**: `ancestors` (ids + merkle hashes), `created_at`,
    ///   `updated_at` outside the nonce.
    /// * **In**: prefix, id, data, nonce, storage-type access-control
    ///   triple (type tag + writer-set or owner).
    ///
    /// This restores signature portability across the receive paths
    /// (delta-replay and sync-reconcile) — both can reconstruct these
    /// bytes from what travels on the wire / what's stored locally,
    /// regardless of how much tree state has drifted.
    ///
    /// Wire-format break: v2 prefix bytes (`v2_upsert` / `v2_delete`)
    /// so a v1-signed action against v2 verifier (or vice versa) fails
    /// loudly rather than silently mis-verifying.
    pub fn payload_for_signing(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        match self {
            Action::Add {
                id, data, metadata, ..
            }
            | Action::Update {
                id, data, metadata, ..
            } => {
                hasher.update(b"v2_upsert");
                hasher.update(id.as_bytes());
                hasher.update(data);
                hash_authorization_for_payload(&mut hasher, metadata);
            }
            Action::DeleteRef {
                id,
                deleted_at,
                metadata,
            } => {
                hasher.update(b"v2_delete");
                hasher.update(id.as_bytes());
                hasher.update(deleted_at.to_le_bytes());
                hash_authorization_for_payload(&mut hasher, metadata);
            }
            Action::Compare { id } => {
                // Compare actions are not signed.
                hasher.update(b"v2_compare");
                hasher.update(id.as_bytes());
            }
        }
        hasher.finalize().into()
    }
}

/// Hash the access-control + nonce triple the signature commits to.
///
/// Single responsibility: produce a deterministic byte sequence that
/// commits to "who can write this entity at what nonce." Replaces the
/// old `hash_metadata_for_payload` which mixed in tree-shape-dependent
/// fields (created_at, updated_at outside its nonce role) on top of
/// the access-control commitment.
///
/// **What this commits to**:
/// * Storage-type tag (`Public` / `Frozen` / `User` / `Shared`) — so a
///   signed User action can't be re-purposed as a Shared one.
/// * Owner pubkey (User) or writers set (Shared) — the access-control
///   list the signer authorized against.
/// * Nonce — replay protection within an entity.
/// * Signer pubkey hint (Shared) — when present, locks the signature
///   to a specific writer rather than letting any writer-set member
///   pose as the signer.
///
/// **What it doesn't commit to**: anything tied to tree state. The
/// signature is transferable across receive paths; tree-shape
/// integrity is enforced by [`AncestorIntegrity::verify`] at apply
/// time, separately.
fn hash_authorization_for_payload(hasher: &mut Sha256, metadata: &Metadata) {
    match &metadata.storage_type {
        StorageType::Public => {
            hasher.update([0u8]); // type tag
                                  // Public has no access-control commitment beyond the tag.
        }
        StorageType::Frozen => {
            hasher.update([1u8]); // type tag
                                  // Frozen is content-addressed; no signing in practice.
        }
        StorageType::User {
            owner,
            signature_data,
        } => {
            hasher.update([2u8]); // type tag
            hasher.update(owner.as_ref() as &[u8; 32]);
            if let Some(sig_data) = signature_data.as_ref() {
                hasher.update(sig_data.nonce.to_le_bytes());
            }
        }
        StorageType::Shared {
            writers,
            signature_data,
        } => {
            hasher.update([3u8]); // type tag
                                  // Writers serialized deterministically: BTreeSet iteration
                                  // is sorted, so this is stable across signer / verifier.
            for writer in writers {
                hasher.update(writer.as_ref() as &[u8; 32]);
            }
            if let Some(sig_data) = signature_data.as_ref() {
                hasher.update(sig_data.nonce.to_le_bytes());
                if let Some(signer_hint) = sig_data.signer {
                    hasher.update([1u8]); // hint-present tag
                    hasher.update(signer_hint.as_ref() as &[u8; 32]);
                } else {
                    hasher.update([0u8]); // hint-absent tag
                }
            }
        }
    }
}
