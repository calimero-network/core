//! Common helper functions for sync protocols.
//!
//! **DRY Principle**: Extract repeated logic from protocol implementations.
#![allow(deprecated)] // #2303: callers migrate per follow-up; group_store wrappers stable

use calimero_node_primitives::sync::TreeLeafData;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, Metadata, StorageType};
use calimero_storage::index::Index;
use calimero_storage::interface::{Action, ApplyContext, Interface};
use calimero_storage::store::MainStorage;
use calimero_store::Store;
use eyre::{bail, Result};
use rand::Rng;

/// Read the local root-hash for `context_id` from the index.
///
/// Returns `[0; 32]` if no root entry exists (empty tree) or if the
/// index read fails. Used by both HashComparison and LevelWise to
/// verify post-sync convergence (#2407).
///
/// Must be called inside a `with_runtime_env(...)` scope.
pub fn get_local_root_hash_for_context(context_id: ContextId) -> Result<[u8; 32]> {
    let root_id = Id::new(*context_id.as_ref());
    match Index::<MainStorage>::get_hashes_for(root_id) {
        Ok(Some((full_hash, _))) => Ok(full_hash),
        Ok(None) => Ok([0u8; 32]),
        Err(e) => {
            tracing::warn!(%context_id, error = %e, "Failed to get root hash");
            Ok([0u8; 32])
        }
    }
}

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
    match &metadata.storage_type {
        StorageType::Public | StorageType::Frozen => None,
        StorageType::Shared { .. } | StorageType::User { .. } => {
            Some(metadata.storage_type.clone())
        }
    }
}

/// Extract the claimed author of a sync'd leaf from its wire-carried
/// authorization, when the storage type admits one.
///
/// * `User { owner, .. }` → `owner` is the author by definition.
/// * `Shared { signature_data: Some(SignatureData { signer: Some(pk), .. }), .. }`
///   → the per-action signature carries the explicit signer. When `signer`
///   is `None` (older actions without the hint), returns `None` — the
///   author can't be identified without scanning the writer set, and the
///   caller treats this as "don't enforce membership here, defer to the
///   per-action signature check inside `apply_action`."
/// * `Public` / `Frozen` / authorization absent → `None`; no author to
///   check (the per-action signature path verifies what's verifiable).
fn extract_author_from_leaf_authorization(
    authorization: Option<&StorageType>,
) -> Option<PublicKey> {
    match authorization? {
        StorageType::User { owner, .. } => Some(*owner),
        StorageType::Shared { signature_data, .. } => {
            signature_data.as_ref().and_then(|sd| sd.signer)
        }
        StorageType::Public | StorageType::Frozen => None,
    }
}

/// Authorization gate for sync apply paths that don't carry a per-leaf
/// governance position on the wire (HashComparison EntityPush, snapshot
/// apply). Mirrors `state_delta_bridge`'s cross-DAG `membership_status_at`
/// check, coarsened to the receiver's *current* group state.
///
/// Returns `true` iff the entity should be applied:
/// * No identifiable author → applied (Public / Frozen / Shared without
///   `signer` hint; the per-action signature inside `apply_action`
///   remains the verifier).
/// * Author identified + currently a member of `context_id`'s owning
///   group → applied.
/// * Author identified + NOT currently a member (or lookup error) →
///   dropped. Closes the HC back door where a now-removed author's
///   entities entered storage without re-running the membership check
///   that the gossip path runs unconditionally. The trade-off (over-
///   rejection of legitimate pre-removal writes that propagate via HC)
///   is documented on
///   [`calimero_context::group_store::is_currently_authorized_for_context`].
pub fn is_leaf_currently_authorized(
    store: &Store,
    context_id: &ContextId,
    leaf: &TreeLeafData,
) -> bool {
    let Some(author) = extract_author_from_leaf_authorization(leaf.metadata.authorization.as_ref())
    else {
        return true;
    };
    match calimero_context::group_store::is_currently_authorized_for_context(
        store, context_id, &author,
    ) {
        Ok(true) => true,
        Ok(false) => {
            // Expected outcome under churn (post-removal authorship,
            // ReadOnly role); track separately from lookup errors so
            // operators can tell normal churn-driven drops from
            // I/O-driven drops at a glance. See `record_hc_leaf_drop`
            // for the ratio semantics.
            crate::node_metrics::record_hc_leaf_drop("unauthorized");
            false
        }
        Err(err) => {
            // Storage layer raised — drop the leaf rather than risk a
            // silent bypass, but escalate to ERROR (not WARN) so the
            // signal isn't lost in routine sync chatter, and emit the
            // counter so the operator dashboard reflects a non-trivial
            // rate of I/O trouble even if individual log lines get
            // dropped under load.
            tracing::error!(
                %context_id,
                %author,
                error = %err,
                "is_leaf_currently_authorized: membership lookup failed; dropping entity to avoid silent bypass"
            );
            crate::node_metrics::record_hc_leaf_drop("lookup_error");
            false
        }
    }
}

/// Detect the synthetic "opaque" CRDT type sync senders attach to leaves
/// whose stored metadata has no `crdt_type` (typically the `Root<T>`
/// entry for apps that don't use `#[app::state]`, plus test fixtures).
/// The sender wraps these in `CrdtType::LwwRegister { inner_type:
/// OPAQUE_LEAF_CRDT_TYPE_NAME }` so the wire never carries an absent
/// type; the receiver uses this helper to recognise them and route to a
/// direct LWW write rather than expecting WASM-side merge dispatch
/// (which doesn't exist for entities without a `Mergeable` impl).
fn is_opaque_crdt_type(crdt_type: &calimero_primitives::crdt::CrdtType) -> bool {
    use calimero_primitives::crdt::CrdtType;
    matches!(crdt_type, CrdtType::LwwRegister { inner_type }
        if inner_type == crate::sync::hash_comparison_protocol::OPAQUE_LEAF_CRDT_TYPE_NAME)
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

    // App root state — `ROOT_ID` or the `Root<T>` entry — needs the
    // app's typed `Mergeable::merge`, which only exists inside the
    // WASM module. The host's `merge_root_state` consults a
    // `MERGE_REGISTRY` static that's never populated in production
    // (the macro's `__calimero_register_merge` export writes the
    // WASM module's copy, not the host's — separate address spaces).
    // Two cases:
    //
    // * Root entity with `crdt_type: Some(_)` — real app state. Skip
    //   here; the caller (HC initiator's DFS, `handle_entity_push`)
    //   accumulates the bytes in `deferred_root_merges` and dispatches
    //   via `ContextClient::merge_root_state` after the sync loop.
    // * Root entity with `crdt_type: None` — opaque (no `Mergeable`
    //   available). WASM dispatch can't help — there's no
    //   `__calimero_merge_root_state` to invoke on a type with no
    //   `Mergeable` impl. The only sensible behavior is direct LWW
    //   write, which is what the old `AllFunctionsFailed` branch in
    //   `merge_root_state` did. Fires in test fixtures and apps that
    //   don't use `#[app::state]`; real apps always have a crdt_type
    //   and take the deferred-dispatch path.
    if calimero_storage::collections::is_app_root_entry(entity_id) {
        if is_opaque_crdt_type(&leaf.metadata.crdt_type) {
            let mut md = Metadata::default();
            md.created_at = leaf.metadata.created_at;
            md.updated_at = leaf.metadata.hlc_timestamp.into();
            calimero_storage::interface::Interface::<MainStorage>::write_pre_merged_root_state(
                entity_id,
                &leaf.value,
                md,
            )?;
            return Ok(());
        }
        // Crdt-bearing root entity — caller defers.
        tracing::warn!(
            %context_id,
            entity_id = %entity_id,
            "HC apply: skipping root-entity merge on host (no host-side merge dispatch); \
             caller dispatches via ContextClient::merge_root_state"
        );
        return Ok(());
    }

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

/// Outcome of an EntityPush batch.
///
/// `applied` is the count of leaves successfully written via the host
/// CRDT apply path. `deferred_root_merges` collects root-entity leaves
/// the host can't merge by itself (same rationale as
/// [`HashComparisonStats::deferred_root_merges`](crate::sync::hash_comparison_protocol::HashComparisonStats::deferred_root_merges)) —
/// the caller dispatches each through `ContextClient::merge_root_state`
/// after the batch returns.
#[derive(Debug, Default)]
pub struct EntityPushOutcome {
    pub applied: u32,
    /// `(entity_id_bytes, incoming_bytes, incoming_hlc_ts)` — same
    /// shape as [`crate::sync::hash_comparison_protocol::HashComparisonStats::deferred_root_merges`].
    /// Carrying the leaf's HLC timestamp lets the dispatcher use the
    /// actual remote write time instead of a synthetic value.
    pub deferred_root_merges: Vec<([u8; 32], Vec<u8>, u64)>,
}

/// Handle an incoming `EntityPush` by applying CRDT merge for each entity.
///
/// Shared between the production responder (`hash_comparison.rs`) and the
/// protocol responder (`hash_comparison_protocol.rs`).
///
/// Must be called within a `with_runtime_env` scope for each entity.
/// Truncates to `MAX_ENTITIES_PER_PUSH` entities per message for DoS protection.
///
/// Each leaf is first run through [`is_leaf_currently_authorized`] — entities
/// whose claimed author is not currently an authorized member of the
/// context's group are dropped before they touch storage. This closes the
/// HC EntityPush authorization back door (gossip rejects a now-removed
/// author's delta, but HC would re-import the same entity unverified).
///
/// Root-entity leaves are surfaced in `deferred_root_merges` for the
/// caller to dispatch via `ContextClient::merge_root_state` — the host
/// has no dispatch table for app-typed root state.
pub fn handle_entity_push(
    store: &Store,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    context_id: ContextId,
    entities: &[TreeLeafData],
) -> EntityPushOutcome {
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
        let mut dropped_unauthorized = 0u32;
        let mut deferred_root_merges: Vec<([u8; 32], Vec<u8>, u64)> = Vec::new();
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
            if !is_leaf_currently_authorized(store, &context_id, leaf) {
                dropped_unauthorized += 1;
                tracing::warn!(
                    %context_id,
                    key = %hex::encode(leaf.key),
                    "pushed entity dropped: claimed author is not currently authorized for this context"
                );
                continue;
            }
            // Root-entity leaves can't be merged on the host (same
            // reason as the HC / LevelWise initiator paths — see
            // `dispatch_deferred_root_merges` in `protocol_selector`).
            // Defer to the caller, which has the `ContextClient` needed
            // to invoke `__calimero_merge_root_state` inside WASM.
            //
            // Exception: a root-entity leaf with `crdt_type: None`
            // (no app-defined `Mergeable`) has nothing for WASM to
            // dispatch to — `__calimero_merge_root_state` would error
            // out, the deferred merge would be dropped, and the bytes
            // would never land. For these opaque entities the only
            // sensible behavior is LWW direct-write (matches the
            // pre-rewrite `AllFunctionsFailed` fallback in
            // `merge_root_state`). Fires in test fixtures + apps that
            // don't use `#[app::state]`; real apps always have a
            // `crdt_type` and go through the proper deferred dispatch.
            // Root entities with a real `crdt_type` get deferred for
            // WASM dispatch; opaque root entities (synthetic LWW marker
            // tagged with `OPAQUE_LEAF_CRDT_TYPE_NAME`) are handled
            // internally by `apply_leaf_with_crdt_merge` via direct LWW
            // write — see the comment there.
            let entity_id = Id::new(leaf.key);
            if calimero_storage::collections::is_app_root_entry(entity_id)
                && !is_opaque_crdt_type(&leaf.metadata.crdt_type)
            {
                deferred_root_merges.push((
                    leaf.key,
                    leaf.value.clone(),
                    leaf.metadata.hlc_timestamp,
                ));
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
        if dropped_unauthorized > 0 {
            tracing::info!(
                %context_id,
                dropped_unauthorized,
                "EntityPush: dropped entities whose author is no longer authorized"
            );
        }
        EntityPushOutcome {
            applied,
            deferred_root_merges,
        }
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

    use calimero_storage::entities::{SignatureData, StorageType};
    use std::collections::BTreeSet;

    #[test]
    fn extract_author_user_returns_owner() {
        let owner = PublicKey::from([7u8; 32]);
        let st = StorageType::User {
            owner,
            signature_data: None,
        };
        assert_eq!(
            extract_author_from_leaf_authorization(Some(&st)),
            Some(owner),
        );
    }

    #[test]
    fn extract_author_shared_with_signer_hint_returns_signer() {
        let signer = PublicKey::from([9u8; 32]);
        let st = StorageType::Shared {
            writers: BTreeSet::from([signer]),
            signature_data: Some(SignatureData {
                signer: Some(signer),
                signature: [0u8; 64],
                nonce: 0,
            }),
        };
        assert_eq!(
            extract_author_from_leaf_authorization(Some(&st)),
            Some(signer),
        );
    }

    #[test]
    fn extract_author_shared_without_signer_hint_returns_none() {
        // Older actions can omit the signer hint — caller treats `None`
        // as "defer to per-action signature verification inside apply_action."
        let st = StorageType::Shared {
            writers: BTreeSet::from([PublicKey::from([1u8; 32])]),
            signature_data: Some(SignatureData {
                signer: None,
                signature: [0u8; 64],
                nonce: 0,
            }),
        };
        assert_eq!(extract_author_from_leaf_authorization(Some(&st)), None);
    }

    #[test]
    fn extract_author_public_returns_none() {
        assert_eq!(
            extract_author_from_leaf_authorization(Some(&StorageType::Public)),
            None,
        );
    }

    #[test]
    fn extract_author_frozen_returns_none() {
        assert_eq!(
            extract_author_from_leaf_authorization(Some(&StorageType::Frozen)),
            None,
        );
    }

    #[test]
    fn extract_author_no_authorization_returns_none() {
        assert_eq!(extract_author_from_leaf_authorization(None), None);
    }
}
