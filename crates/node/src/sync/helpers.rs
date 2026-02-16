//! Common helper functions for sync protocols.
//!
//! **DRY Principle**: Extract repeated logic from protocol implementations.

use calimero_node_primitives::sync::TreeLeafData;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, Metadata};
use calimero_storage::index::Index;
use calimero_storage::interface::{Action, Interface};
use calimero_storage::store::MainStorage;
use eyre::{bail, Result};
use rand::Rng;

/// Validates that peer's application ID matches ours.
///
/// # Errors
///
/// Returns error if application IDs don't match.
#[allow(dead_code, reason = "utility function for application validation")]
pub fn validate_application_id(ours: &ApplicationId, theirs: &ApplicationId) -> eyre::Result<()> {
    if ours != theirs {
        bail!("application mismatch: expected {}, got {}", ours, theirs);
    }
    Ok(())
}

/// Generates a random nonce for message encryption.
#[must_use]
pub fn generate_nonce() -> calimero_crypto::Nonce {
    rand::thread_rng().gen()
}

/// Apply leaf data using CRDT merge (Invariant I5: No Silent Data Loss).
///
/// This function must be called within a `with_runtime_env` scope.
/// Uses `Interface::apply_action` to properly update both the raw storage
/// and the Merkle tree Index.
///
/// # CRDT Merge Behavior
///
/// The storage layer uses the `crdt_type` and `updated_at` metadata fields
/// to perform appropriate CRDT merge semantics:
/// - LWWRegister: Last-writer-wins based on HLC timestamp
/// - GCounter: Monotonically increasing merge
/// - Other CRDTs: Type-specific merge logic
///
/// # Arguments
///
/// * `context_id` - The context being synchronized
/// * `leaf` - The leaf data containing entity key, value, and CRDT metadata
///
/// # Errors
///
/// Returns error if storage operations fail.
pub fn apply_leaf_with_crdt_merge(context_id: ContextId, leaf: &TreeLeafData) -> Result<()> {
    let entity_id = Id::new(leaf.key);
    let root_id = Id::new(*context_id.as_ref());

    // Check if entity already exists
    let existing_index = Index::<MainStorage>::get_index(entity_id).ok().flatten();

    // Build metadata from leaf info
    let mut metadata = Metadata::default();
    metadata.crdt_type = Some(leaf.metadata.crdt_type.clone());
    metadata.updated_at = leaf.metadata.hlc_timestamp.into();

    let action = if existing_index.is_some() {
        // Update existing entity - storage layer handles CRDT merge
        Action::Update {
            id: entity_id,
            data: leaf.value.clone(),
            ancestors: vec![], // No ancestors needed for update
            metadata,
        }
    } else {
        // Add new entity as child of root
        // First ensure root exists
        if Index::<MainStorage>::get_index(root_id)
            .ok()
            .flatten()
            .is_none()
        {
            let root_action = Action::Update {
                id: root_id,
                data: vec![],
                ancestors: vec![],
                metadata: Metadata::default(),
            };
            Interface::<MainStorage>::apply_action(root_action)?;
        }

        // Get root info for ancestor chain
        let root_hash = Index::<MainStorage>::get_hashes_for(root_id)
            .ok()
            .flatten()
            .map(|(full, _)| full)
            .unwrap_or([0; 32]);
        let root_metadata = Index::<MainStorage>::get_index(root_id)
            .ok()
            .flatten()
            .map(|idx| idx.metadata.clone())
            .unwrap_or_default();

        let ancestor = ChildInfo::new(root_id, root_hash, root_metadata);

        Action::Add {
            id: entity_id,
            data: leaf.value.clone(),
            ancestors: vec![ancestor],
            metadata,
        }
    };

    Interface::<MainStorage>::apply_action(action)?;
    Ok(())
}
