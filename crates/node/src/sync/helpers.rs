//! Common helper functions for sync protocols.
//!
//! **DRY Principle**: Extract repeated logic from protocol implementations.

use calimero_node_primitives::sync::TreeLeafData;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, Metadata};
use calimero_storage::index::Index;
use calimero_storage::interface::{Action, ApplyContext, Interface};
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

/// Extract the authorization triple to put on the HashComparison wire
/// for an entity, if any. `Shared` / `User` entities need the writer's
/// signature data + access-control list on the wire so the receiver
/// can verify the signature without consulting the originator's tree
/// state. `Public` / `Frozen` entities don't need it (no signature
/// required).
///
/// The local index entry is expected to carry a real `signature_data`
/// by the time HashComparison ships it: the runtime executor's
/// `sign_authorized_actions` step writes the signed `signature_data`
/// back to the local index via `Interface::update_signature_in_place`
/// (see `crates/context/src/handlers/execute/mod.rs::persist_signed_signatures`).
/// If an entity ever does carry `signature_data: None` here (e.g.
/// inside a test fixture that skips the runtime sign step), the
/// receiver will reject it with `"Remote Shared/User action must be
/// signed"` — that's the intended error: unsigned state isn't sync'd.
///
/// Single source of truth — all `TreeLeafData` construction sites in
/// the sync senders go through this helper rather than open-coding the
/// match arm, so a future addition (e.g. a new storage type that needs
/// authorization) only has to be added in one place.
pub fn wire_authorization_for(
    metadata: &Metadata,
) -> Option<calimero_storage::entities::StorageType> {
    use calimero_storage::entities::StorageType;
    match &metadata.storage_type {
        StorageType::Public | StorageType::Frozen => None,
        StorageType::Shared { .. } | StorageType::User { .. } => {
            Some(metadata.storage_type.clone())
        }
    }
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

    // Build metadata from leaf info.
    //
    // `created_at` matters: `ChildInfo` orders a parent's children by
    // `created_at` (then `id`), and that order feeds the parent's — and
    // the root's — Merkle hash. For a *new* entity received here we must
    // use the originating `created_at` carried in the leaf, not the
    // `Metadata::default()` zero, or this node sorts the entity
    // differently from one that received it via delta-apply → diverging
    // root hash (the #2319 "Same DAG heads, different root hash" bug).
    // For an *existing* entity the storage layer keeps the stored
    // `created_at` and ignores this value, so setting it unconditionally
    // is harmless. (`leaf.metadata.created_at` is `0` only when the peer
    // ran pre-#2322 code that didn't transmit it.)
    let mut metadata = Metadata::default();
    metadata.crdt_type = Some(leaf.metadata.crdt_type.clone());
    metadata.updated_at = leaf.metadata.hlc_timestamp.into();
    metadata.created_at = leaf.metadata.created_at;

    // Storage-type provenance:
    //
    // 1. Wire-carried authorization (Shared/User) — use it verbatim.
    //    The apply path's signature verifier will check the sig_data
    //    inside this `StorageType` against the new (tree-state-free)
    //    `payload_for_signing`, which the receiver reconstructs from
    //    the action's components (id, data, this storage_type).
    //    Bootstrap entities now carry a real signature (see
    //    `persist_signed_signatures` in
    //    `crates/context/src/handlers/execute/mod.rs`) so this path
    //    is always verifiable.
    //
    // 2. Existing entity, no wire authorization — preserve the
    //    stored storage_type. Avoids the v1 silent storage-type-flip
    //    bug where every sync apply downgraded entities to `Public`
    //    via `Metadata::default()`.
    //
    // 3. New entity, no wire authorization — default to `Public`.
    //    Non-Public new entities require creation-time invariants
    //    (writer-set, owner) that arrive via the wire authorization
    //    or the delta path.
    if let Some(wire_auth) = leaf.metadata.authorization.as_ref() {
        metadata.storage_type = wire_auth.clone();
    } else if let Some(ref existing) = existing_index {
        metadata.storage_type = existing.metadata.storage_type.clone();
    }

    let action = if existing_index.is_some() {
        // Update existing entity - storage layer handles CRDT merge
        Action::Update {
            id: entity_id,
            data: leaf.value.clone(),
            ancestors: vec![], // No ancestors needed for update
            metadata,
        }
    } else {
        // Add new entity. The leaf carries the *originating peer's*
        // `parent_id` on the wire (see senders in
        // `hash_comparison{,_protocol}.rs::get_local_tree_node` and
        // `collect_leaves_recursive`); use it as the ancestor so the
        // entity lands at the same Merkle position the originator has —
        // critical for nested entities (e.g. `Root<KvStore>::items["k"]`
        // lives under the items collection, not directly under the
        // context root). Pre-fix this unconditionally used the context
        // root, which silently corrupted the Merkle topology for any
        // nested-collection entity and made the resulting root hashes
        // irreconcilable: HashComparison would keep merging the same
        // entities round after round with no convergence (38+ identical-
        // stat sessions on bdc61af's Round 2).
        //
        // If the peer didn't transmit `parent_id` (legacy / out-of-sync
        // peer), fall back to the context root — same behaviour as
        // before this fix.
        let parent_id = leaf.metadata.parent_id.map(Id::new).unwrap_or(root_id);

        // Ensure the chosen parent has an index. For a freshly-pulled
        // nested entity the parent may not yet exist locally — when the
        // sender's `index.parent_id()` points at a parent we haven't
        // pulled yet (HashComparison walks the tree top-down but a
        // single EntityPush batch can deliver a child before its parent
        // due to BFS-vs-DFS ordering and batch boundaries), create a
        // placeholder index here so `Action::Add { ancestors: [parent] }`
        // has something to attach to. When the parent itself arrives via
        // a later push it'll go through the Update path (existing entity)
        // and its real data + metadata replaces the placeholder; the
        // child's `parent_id` link is preserved across that.
        if Index::<MainStorage>::get_index(parent_id)
            .ok()
            .flatten()
            .is_none()
        {
            let parent_init = Action::Update {
                id: parent_id,
                data: vec![],
                ancestors: vec![],
                metadata: Metadata::default(),
            };
            // #2266: snapshot leaf push has no `CausalDelta` in scope —
            // these bytes come from a peer who already verified them.
            // Empty ctx → verifier falls back to v2 stored-writers, which
            // is the safe semantic for already-verified replicated state.
            Interface::<MainStorage>::apply_action(parent_init, &ApplyContext::empty())?;
        }

        let parent_hash = Index::<MainStorage>::get_hashes_for(parent_id)
            .ok()
            .flatten()
            .map(|(full, _)| full)
            .unwrap_or([0; 32]);
        let parent_metadata = Index::<MainStorage>::get_index(parent_id)
            .ok()
            .flatten()
            .map(|idx| idx.metadata.clone())
            .unwrap_or_default();

        let ancestor = ChildInfo::new(parent_id, parent_hash, parent_metadata);

        // Tree-shape integrity NOT cryptographically asserted here:
        // `ancestor.merkle_hash` is fetched live from the local
        // index, so `Interface::apply_action`'s
        // `verify_ancestor_integrity` always passes on this path
        // (the hash matches what's locally stored). This is the
        // documented design trade-off: HashComparison sync runs
        // precisely because tree shapes have drifted between
        // peers, so asserting "the signer observed the same
        // parent hash" would reject every legitimate divergence
        // repair. Authorization (the signature inside
        // `metadata.storage_type`) still verifies — what we
        // forgo is sender-vs-receiver agreement on the parent's
        // subtree hash. The delta-replay path carries the
        // signer's ancestor list and does check it.
        Action::Add {
            id: entity_id,
            data: leaf.value.clone(),
            ancestors: vec![ancestor],
            metadata,
        }
    };

    // #2266: snapshot leaf push has no `CausalDelta` in scope — these
    // bytes come from a peer who already verified them. Empty ctx →
    // verifier falls back to v2 stored-writers, which is the safe
    // semantic for already-verified replicated state.
    Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())?;
    Ok(())
}

/// Maximum entities per `EntityPush` message (shared between initiator and responder).
///
/// The initiator batches at this limit; the responder truncates messages exceeding it.
pub const MAX_ENTITIES_PER_PUSH: usize = 500;

/// Handle an incoming `EntityPush` by applying CRDT merge for each entity.
///
/// Shared between the production responder (`hash_comparison.rs`) and the
/// protocol responder (`hash_comparison_protocol.rs`).
///
/// Must be called within a `with_runtime_env` scope for each entity.
/// Truncates to `MAX_ENTITIES_PER_PUSH` entities per message for DoS protection.
///
/// Returns the number of entities successfully applied.
pub fn handle_entity_push(
    runtime_env: &calimero_storage::env::RuntimeEnv,
    context_id: ContextId,
    entities: &[TreeLeafData],
) -> u32 {
    let entities = if entities.len() > MAX_ENTITIES_PER_PUSH {
        tracing::warn!(
            %context_id,
            received = entities.len(),
            max = MAX_ENTITIES_PER_PUSH,
            "EntityPush exceeds max, truncating"
        );
        &entities[..MAX_ENTITIES_PER_PUSH]
    } else {
        entities
    };

    calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
        let mut applied = 0u32;
        for leaf in entities {
            if !leaf.is_valid() {
                tracing::warn!(
                    %context_id,
                    key = %hex::encode(leaf.key),
                    len = leaf.value.len(),
                    "pushed entity failed TreeLeafData::is_valid(), skipping"
                );
                continue;
            }
            match apply_leaf_with_crdt_merge(context_id, leaf) {
                Ok(()) => applied += 1,
                Err(e) => {
                    tracing::warn!(
                        %context_id,
                        key = %hex::encode(leaf.key),
                        error = %e,
                        "Failed to apply pushed entity"
                    );
                }
            }
        }
        applied
    })
}

/// Extract a [`SignedNamespaceOp`](calimero_context_client::local_governance::SignedNamespaceOp)
/// from a `skeleton_bytes` store value.
///
/// The store encodes entries as `StoredNamespaceEntry::Signed(op)`. Returns
/// `None` for opaque skeletons (non-member rows) or if the bytes do not
/// decode as either form.
///
/// Prefer this over [`extract_signed_op_bytes`] when the caller needs the
/// typed op (e.g. to wrap in `NamespaceTopicMsg::Op` for gossip publish) —
/// it avoids a redundant `borsh::to_vec` + `borsh::from_slice` round-trip.
pub fn extract_signed_op(
    skeleton_bytes: &[u8],
) -> Option<calimero_context_client::local_governance::SignedNamespaceOp> {
    use calimero_context_client::local_governance::{SignedNamespaceOp, StoredNamespaceEntry};

    if let Ok(StoredNamespaceEntry::Signed(op)) =
        borsh::from_slice::<StoredNamespaceEntry>(skeleton_bytes)
    {
        return Some(op);
    }
    // Fallback: already raw SignedNamespaceOp bytes (legacy / direct-publish path).
    borsh::from_slice::<SignedNamespaceOp>(skeleton_bytes).ok()
}

/// Extract raw `SignedNamespaceOp` bytes from a `skeleton_bytes` store value.
///
/// The store encodes entries as `StoredNamespaceEntry::Signed(op)`. The
/// **stream-based** wire paths (sync backfill response, namespace-join
/// response) consume the bytes returned here directly so the receiver can
/// `borsh::from_slice::<SignedNamespaceOp>(...)`.
///
/// The **gossip** publish path (`BroadcastMessage::NamespaceGovernanceDelta`)
/// requires its payload to be a `NamespaceTopicMsg::Op(op)` envelope after
/// Phase 2 of #2237 — gossip callers should prefer [`extract_signed_op`]
/// to avoid an unnecessary serialization round-trip.
pub fn extract_signed_op_bytes(skeleton_bytes: &[u8]) -> Option<Vec<u8>> {
    extract_signed_op(skeleton_bytes).and_then(|op| borsh::to_vec(&op).ok())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::application::ApplicationId;

    #[test]
    fn test_validate_application_id_matching() {
        let app_id = ApplicationId::from([1u8; 32]);
        assert!(validate_application_id(&app_id, &app_id).is_ok());
    }

    #[test]
    fn test_validate_application_id_mismatch() {
        let app1 = ApplicationId::from([1u8; 32]);
        let app2 = ApplicationId::from([2u8; 32]);
        let result = validate_application_id(&app1, &app2);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("application mismatch"));
    }

    #[test]
    fn test_generate_nonce_returns_value() {
        let nonce = generate_nonce();
        // Nonce should be non-zero (extremely unlikely to be all zeros)
        // Nonce is NONCE_LEN = 12 bytes
        assert_ne!(nonce, [0u8; 12]);
    }

    #[test]
    fn test_generate_nonce_is_random() {
        // Generate two nonces - they should be different
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        assert_ne!(nonce1, nonce2, "Nonces should be randomly generated");
    }

    // Note: `apply_leaf_with_crdt_merge` requires a full storage runtime environment
    // (via `with_runtime_env`). It is tested indirectly through the sync_sim
    // integration tests which set up `SimStorage` with proper storage backends.
    // See: crates/node/tests/sync_sim/
}
