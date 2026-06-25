//! Common helper functions for sync protocols.
//!
//! **DRY Principle**: Extract repeated logic from protocol implementations.
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::sync::{EntityDeletion, TreeLeafData};
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

/// This node's `scope_root` for `context_id` at `entities_root` (the storage
/// Merkle root): resolve the context's owning group, then fold the governance
/// projection's ACL + membership/admin hashes onto `entities_root`
/// ([`ScopeProjections::group_scope_root_ephemeral`]).
///
/// Folds an EPHEMERAL projection from the `store` (rather than the node's
/// maintained one) so the HC initiator — which has the store but no `NodeState` —
/// and the responder compute the signal the same way. `None` for a non-group
/// context (no governance plane to fold) or a store/DAG fault — the caller MUST
/// then **skip** the scope_root shadow comparison, never read it as a divergence
/// (unified-causal-log cutover C0).
///
/// **Observe-only in C0:** the result is logged for the hash-neutral-rotation
/// shadow, never fed into any sync decision. C1 promotes it to the authoritative
/// convergence signal (and switches to the maintained projection).
///
/// TODO(perf, C1+): each call is a full `collect_namespace_ops` RocksDB DAG walk,
/// and a sync session folds independently on both peers (responder + initiator),
/// so a namespace with deep governance history pays an unbounded O(n) read per
/// sync tick. Acceptable while this is observe-only, but bound it before/with the
/// C1 flip — the node-side responders hold a `NodeState`, so they can read the
/// already-maintained projection (`scope_root_for` on `read_scope_projections()`)
/// instead of re-folding; the initiator can take a per-session cache or have the
/// scope_root threaded down rather than recomputed.
pub(crate) fn local_scope_root(
    store: &Store,
    context_id: &ContextId,
    entities_root: [u8; 32],
) -> Option<[u8; 32]> {
    let group = calimero_context::group_store::get_group_for_context(store, context_id)
        .ok()
        .flatten()?;
    calimero_context::scope_projection::ScopeProjections::group_scope_root_ephemeral(
        store,
        &group,
        entities_root,
    )
}

/// The cross-plane convergence verdict between two peers (cutover P6.S1 — the single
/// source of truth all sync protocols decide against).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeVerdict {
    /// Both planes agree — the authoritative `scope_root` matched, or (when a scope
    /// can't be folded) the bare entity roots matched.
    Converged,
    /// Entity roots agree but `scope_root` differs ⇒ pure ACL/governance divergence
    /// (the hash-neutral case the entity root hides; awaits governance sync). Carries
    /// the two resolved scope roots `(local, peer)` so callers log them without
    /// re-destructuring the `Option`s the verdict already proved were `Some`.
    GovDiverged([u8; 32], [u8; 32]),
    /// Entity roots differ ⇒ the data plane needs reconciliation.
    DataDiverged,
}

impl ScopeVerdict {
    pub(crate) fn converged(self) -> bool {
        matches!(self, ScopeVerdict::Converged)
    }
}

/// The authoritative convergence verdict (C1): `scope_root` folds entities + ACL +
/// membership/admin, so when BOTH sides resolve one it alone decides — closing the
/// hash-neutral rotation blind spot. When either side can't fold the scope (cold
/// projection / non-group context, `None`), fall back to the bare entity-root compare
/// — exactly the pre-C1 behaviour, so no context regresses to a weaker check.
///
/// Previously this verdict was open-coded identically in `hash_comparison_protocol`,
/// `level_sync`, and the delta paths; P6.S1 makes it one function so every sync
/// protocol reaches the same conclusion and the later stages can route off it.
pub(crate) fn scope_verdict(
    local_scope_root: Option<[u8; 32]>,
    peer_scope_root: Option<[u8; 32]>,
    local_entity_root: [u8; 32],
    peer_entity_root: [u8; 32],
) -> ScopeVerdict {
    match (local_scope_root, peer_scope_root) {
        (Some(local), Some(peer)) if local == peer => ScopeVerdict::Converged,
        (Some(local), Some(peer)) if local_entity_root == peer_entity_root => {
            ScopeVerdict::GovDiverged(local, peer)
        }
        (Some(_), Some(_)) => ScopeVerdict::DataDiverged,
        // Asymmetric `None` (exactly one side has a cold projection / non-group
        // context) intentionally falls back to the bare entity-root compare rather
        // than reporting GovDiverged: we can't fold a scope_root we don't have, so
        // treating the mismatch as governance divergence would raise a false alarm
        // on a partially-warmed node. This is the pre-C1 check — don't "fix" it.
        _ if local_entity_root == peer_entity_root => ScopeVerdict::Converged,
        _ => ScopeVerdict::DataDiverged,
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
        StorageType::Shared { .. }
        | StorageType::User { .. }
        | StorageType::SharedMember { .. } => Some(metadata.storage_type.clone()),
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
        StorageType::Shared { signature_data, .. }
        | StorageType::SharedMember { signature_data, .. } => {
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
    session_peer: Option<PublicKey>,
) -> bool {
    let author = match extract_author_from_leaf_authorization(leaf.metadata.authorization.as_ref())
    {
        Some(author) => author,
        None => {
            // Authorless PLAIN (Public) leaf — carries no signer, so the
            // author-based gate below can't apply. Fall back to the
            // authenticated SESSION PEER's current membership: a peer that is
            // no longer an authorized member of the context must not be able
            // to launder a plain-entity write into our store via HC/LevelWise
            // (the gossip path already rejects its signed delta by author, but
            // HC merges *state*, which a Public entity carries no authorship
            // for). A removed peer's push is dropped at the first hop, so the
            // write never propagates further. When there is no session peer
            // (local / snapshot apply, or a path that can't attribute one),
            // keep the historical allow — the per-action checks downstream
            // remain the backstop.
            return match session_peer {
                Some(peer) => calimero_context::group_store::is_currently_authorized_for_context(
                    store, context_id, &peer,
                )
                .unwrap_or(false),
                None => true,
            };
        }
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

/// Resolve a peer's hosted identities to one `session_peer` for an authorless
/// leaf gate: an authorized identity if the peer hosts one, else any hosted one
/// (so the downstream check drops the leaf), else `None` (peer hosts none →
/// unattributable, historical allow).
pub(crate) fn select_attributable_peer_identity(
    hosted: &std::collections::BTreeSet<PublicKey>,
    is_authorized: impl Fn(&PublicKey) -> bool,
) -> Option<PublicKey> {
    if let Some(authorized) = hosted.iter().find(|id| is_authorized(id)) {
        return Some(*authorized);
    }
    // Peer hosts identities but none authorized: return any so the gate drops
    // it. Empty set → None → unattributable (historical allow).
    hosted.iter().next().copied()
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
/// Whether a leaf that arrived with **no** wire-supplied ancestor chain can
/// be placed safely using only its `parent_id`.
///
/// Safe iff the parent is the context root (`parent_is_root` — no
/// intermediate ancestors, so a single-parent chain is exact) or the parent
/// already exists locally (`parent_present_locally` — its ancestry is
/// already established and `apply_action` links the leaf directly under it).
///
/// When neither holds, the single-parent fallback makes `apply_action`
/// `add_root` the missing parent, placing a nested entity directly under the
/// context root — the wrong Merkle position, which produces a root hash that
/// diverges from peers holding the full chain while the DAG heads still
/// match (the same-DAG-heads / different-root split-brain HashComparison
/// cannot heal). The caller must then decline to place the leaf and reapply
/// it once the parent has synced.
fn empty_chain_placement_is_safe(parent_is_root: bool, parent_present_locally: bool) -> bool {
    parent_is_root || parent_present_locally
}

/// Outcome of a schema-gated sync-repair leaf apply (PR-6b Task 6b.7).
///
/// The sync-repair paths (HashComparison / LevelSync / snapshot) bypass the
/// gossip state-delta fence, so the readability check lives here instead. A
/// receiver whose *loaded* reader cannot read a leaf authored under a newer
/// schema declines + buffers it rather than LWW-storing unreadable bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafOutcome {
    /// The leaf was applied to storage (schema matches / legacy leaf / no
    /// gating context).
    Applied,
    /// The leaf was declined and buffered into the absorb buffer — its
    /// `schema_app_key` is newer than the receiver's loaded reader. It will be
    /// re-applied verbatim once the reader advances.
    Buffered,
}

/// Schema-gated wrapper around [`apply_leaf_with_crdt_merge`] for the
/// sync-repair paths (PR-6b Task 6b.7 / #2539).
///
/// The HashComparison / LevelSync / snapshot repair paths bypass the gossip
/// state-delta fence entirely, so without this a receiver still on an older
/// reader would LWW-store unreadable future-schema bytes (the
/// "v1-binary-fed-v2-bytes" corruption hazard). This gate keys on the
/// receiver's **loaded** reader (`loaded_app_key`, i.e. `loaded_reader_app_key`)
/// rather than the replicated `GroupMeta.app_key` (the O3 correction):
///
/// * `leaf.metadata.schema_app_key == Some(k)` with `k != loaded_app_key` —
///   the receiver lacks a reader for the incoming schema. **Decline + buffer**
///   the leaf verbatim into the absorb buffer (a leaf-shaped [`AbsorbRecord`])
///   and return [`LeafOutcome::Buffered`]. The bytes are NEVER stored; the
///   drain re-applies them once the reader advances.
/// * `schema_app_key == None` (legacy peer) or `== Some(loaded_app_key)` —
///   apply as today and return [`LeafOutcome::Applied`].
///
/// Must be called inside a `with_runtime_env(...)` scope (it delegates to
/// [`apply_leaf_with_crdt_merge`] on the apply branch).
pub fn apply_leaf_with_crdt_merge_gated(
    store: &Store,
    context_id: ContextId,
    leaf: &TreeLeafData,
    loaded_app_key: [u8; 32],
) -> Result<LeafOutcome> {
    if let Some(schema) = leaf.metadata.schema_app_key {
        if schema != loaded_app_key {
            // The receiver's loaded reader can't read this leaf — buffer it
            // verbatim instead of storing unreadable bytes. Keyed by the leaf
            // key (idempotent overwrite on re-delivery), under the *sender's*
            // schema so the drain only re-applies once this node advances to it.
            let leaf_bytes = borsh::to_vec(leaf)?;
            let record = calimero_context::group_store::AbsorbRecord::from_leaf(
                leaf.key, leaf_bytes, schema,
            );
            calimero_context::group_store::AbsorbRepository::new(store).save(
                &context_id,
                schema,
                &record,
            )?;
            crate::node_metrics::record_delta_outcome("absorbed_leaf_future_schema");
            tracing::warn!(
                %context_id,
                key = %hex::encode(leaf.key),
                ?schema,
                ?loaded_app_key,
                "sync-repair leaf authored under a newer schema than the loaded \
                 reader — buffered into the absorb buffer instead of storing \
                 unreadable bytes (will replay once the reader advances)"
            );
            return Ok(LeafOutcome::Buffered);
        }
    }
    apply_leaf_with_crdt_merge(context_id, leaf)?;
    Ok(LeafOutcome::Applied)
}

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
    } else if matches!(
        leaf.metadata.crdt_type,
        calimero_primitives::crdt::CrdtType::FrozenStorage
    ) {
        // New entity, no wire authorization, but the wire-carried `crdt_type`
        // says this is Frozen storage. Frozen entities carry no authorization
        // (content-addressed + immutable, so `wire_authorization_for` returns
        // None), so without this they'd fall through to the `Public` default
        // below. A peer that then receives the real `Frozen` entity via a delta
        // would reject the `Public -> Frozen` storage-type change in
        // `apply_action` ("Cannot change StorageType"), panicking the guest's
        // frozen-value merge — the HC/LevelWise frozen-push split-brain. The
        // `crdt_type` IS on the wire, so infer `Frozen` from it.
        metadata.storage_type = StorageType::Frozen;
    }

    let action = if existing_index.is_some() {
        // Frozen entities are content-addressed and immutable: an entry
        // that already exists locally is by definition the correct
        // content (its id is derived from its content hash), so there is
        // nothing to update. Critically, the storage layer categorically
        // REJECTS `Action::Update` for `Frozen` ("Frozen data cannot be
        // updated"). Emitting one here — e.g. when a bulk leaf push
        // re-sends an already-present frozen leaf while repairing a
        // divergence in a *sibling* entity — fails and aborts the ENTIRE
        // HashComparison repair, leaving the actually-divergent entity
        // unreconciled. That is the intermittent scaffolding-e2e "Frozen
        // data cannot be updated" split-brain: a frozen leaf is the
        // victim that blocks recovery, not the source of divergence.
        // Skip it; the immutable entry is already present and correct.
        if matches!(metadata.storage_type, StorageType::Frozen) {
            return Ok(());
        }
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

        // Initialise the context root entry if it's not in the local
        // index yet. `apply_action`'s `id.is_root()` branch then runs
        // `add_root` and `save_internal` writes empty `Key::Entry(root_id)`
        // so the root's `own_hash = Sha256::digest(empty)` matches the
        // sender's (which produced its root the same way via `init_root`
        // or equivalent). Without this, the receiver's root own_hash
        // stays `[0; 32]` and diverges from the sender's. Gated on
        // `parent_id.is_root()` because non-root parents are now handled
        // by the wire-supplied ancestor chain (see comment below).
        if parent_id.is_root()
            && Index::<MainStorage>::get_index(parent_id)
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

        // Prefer the wire-supplied ancestor chain (immediate parent →
        // root_child, root excluded). `apply_action`'s ancestor loop
        // walks it in reverse and links each entry to the next up,
        // placing intermediate ancestors at the correct tree level.
        //
        // Legacy fallback for peers shipping only `parent_id`: a
        // one-element chain. `apply_action` will then `add_root`
        // missing grandparents, which can misplace deeply nested
        // entities until the real ancestors arrive via their own leaf
        // pushes.
        let ancestors = if !leaf.metadata.ancestors.is_empty() {
            leaf.metadata.ancestors.clone()
        } else {
            // No wire-supplied chain. The single-parent fallback is only
            // safe when this entity's position is already unambiguous
            // locally: the parent is the context root (no intermediate
            // ancestors), or the parent already exists in our index (so
            // its ancestry is established and `apply_action` links this
            // entity directly under it). Otherwise `apply_action` would
            // `add_root(parent)` for the missing parent, placing this
            // nested entity directly under the context root — the wrong
            // Merkle position. That yields a root hash that diverges from
            // peers holding the full chain while the DAG heads still
            // match, the same-DAG-heads / different-root split-brain that
            // HashComparison cannot heal (scaffolding-e2e run
            // 26679287804). Decline to place it; a later round reapplies
            // it once the parent collection has synced (the responder
            // pushes containers before leaves).
            let parent_index = Index::<MainStorage>::get_index(parent_id).ok().flatten();
            if !empty_chain_placement_is_safe(parent_id.is_root(), parent_index.is_some()) {
                tracing::warn!(
                    %context_id,
                    %entity_id,
                    %parent_id,
                    "HC apply: leaf arrived without an ancestor chain and its parent \
                     is not present locally; deferring rather than guessing its tree \
                     position (avoids a divergent root hash HashComparison cannot heal)"
                );
                return Ok(());
            }
            let parent_hash = Index::<MainStorage>::get_hashes_for(parent_id)
                .ok()
                .flatten()
                .map(|(full, _)| full)
                .unwrap_or([0; 32]);
            let parent_metadata = parent_index
                .map(|idx| idx.metadata.clone())
                .unwrap_or_default();
            vec![ChildInfo::new(parent_id, parent_hash, parent_metadata)]
        };

        // Tree-shape integrity NOT cryptographically asserted here:
        // the chain's `merkle_hash` values come either from the
        // peer's wire (not signed; see `LeafMetadata::ancestors`
        // field doc on the trust model) or from the receiver's own
        // index (legacy fallback above) — in either case
        // `verify_ancestor_integrity` is informational only on this
        // path. This is the documented design trade-off:
        // HashComparison sync runs precisely because tree shapes
        // have drifted between peers, so asserting "the signer
        // observed the same parent hash" would reject every
        // legitimate divergence repair. Authorization (the
        // signature inside `metadata.storage_type`) still verifies
        // — what we forgo is sender-vs-receiver agreement on the
        // ancestor chain's subtree hashes. The delta-replay path
        // carries the signer's ancestor list and does check it.
        Action::Add {
            id: entity_id,
            data: leaf.value.clone(),
            ancestors,
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
    session_peer: Option<PublicKey>,
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

    // PR-6b Task 6b.7: the schema this node can read *right now* (its loaded
    // reader). A future-schema leaf is declined+buffered rather than stored.
    //
    // Fail CLOSED on a store error: keep the full `Result` rather than
    // collapsing it with `.ok().flatten()`. `Ok(None)` legitimately means
    // "no group / unresolvable meta" ⇒ no gate, apply as today. But an `Err`
    // means we CANNOT determine readability — silently applying ungated would
    // let a future-schema leaf the node can't read get LWW-stored (the exact
    // v1-binary-fed-v2-bytes corruption this gate prevents). These pushed
    // leaves are non-destructive sync-repair leaves that get re-pushed on the
    // next sync cycle, so skipping the batch here is safe.
    let loaded_app_key = calimero_context::hlc_fence::loaded_reader_app_key(store, &context_id);
    apply_entity_push_batch(
        store,
        runtime_env,
        context_id,
        entities,
        loaded_app_key,
        session_peer,
    )
}

/// Apply (or buffer) a pre-truncated, pre-resolved `EntityPush` batch.
///
/// `loaded_app_key` is the resolution of the receiver's loaded reader schema:
/// * `Ok(Some(k))` — gate active; future-schema leaves are buffered.
/// * `Ok(None)` — legitimately no group / unresolvable meta ⇒ no gate, apply
///   as today.
/// * `Err(_)` — a STORE ERROR; readability cannot be determined. Fail closed:
///   log and SKIP the batch (return an empty outcome). The leaves are
///   non-destructive and are re-pushed on the next sync cycle.
fn apply_entity_push_batch(
    store: &Store,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    context_id: ContextId,
    entities: &[TreeLeafData],
    loaded_app_key: Result<Option<[u8; 32]>>,
    session_peer: Option<PublicKey>,
) -> EntityPushOutcome {
    let loaded_app_key = match loaded_app_key {
        Ok(key) => key,
        Err(e) => {
            tracing::warn!(
                %context_id,
                error = %e,
                count = entities.len(),
                "EntityPush: could not resolve loaded reader schema (store error); \
                 skipping batch fail-closed — leaves will be re-pushed next sync"
            );
            return EntityPushOutcome::default();
        }
    };

    calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
        let mut applied = 0u32;
        let mut dropped_unauthorized = 0u32;
        let mut buffered = 0u32;
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
            if !is_leaf_currently_authorized(store, &context_id, leaf, session_peer) {
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
            let apply_result = match loaded_app_key {
                Some(loaded) => apply_leaf_with_crdt_merge_gated(store, context_id, leaf, loaded)
                    .map(|outcome| match outcome {
                        LeafOutcome::Applied => true,
                        LeafOutcome::Buffered => false,
                    }),
                // No loaded reader resolvable — apply as before (no gate).
                None => apply_leaf_with_crdt_merge(context_id, leaf).map(|()| true),
            };
            match apply_result {
                Ok(true) => applied += 1,
                Ok(false) => buffered += 1,
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
        if buffered > 0 {
            tracing::info!(
                %context_id,
                buffered,
                "EntityPush: buffered future-schema entities into the absorb buffer"
            );
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

/// Run a host-side storage mutation while holding the per-context execution
/// lock, so it cannot interleave with a concurrent `__calimero_sync_next`
/// delta merge running in the executor.
///
/// The sync session and the executor live in different actors. The executor
/// holds the context's `Arc<Mutex<_>>` for the whole of a WASM run, but the
/// sync apply paths historically wrote storage directly, guarded only by the
/// byte-level `index_mutation_guard`. That guard makes each individual mutator
/// atomic but does NOT make a whole logical apply (a multi-write read-modify-
/// write that recomputes ancestor hashes up to the root) atomic against another
/// logical apply. Two such operations then interleave their recomputes and
/// record a torn root hash that delta-sync can't repair — a permanent
/// split-brain. Taking the same lock here serializes them.
///
/// `context_client` is `None` only on paths with no executor running
/// concurrently (the single-threaded sync-sim harness); there the apply runs
/// unguarded, exactly as before.
pub async fn apply_under_context_lock<R>(
    context_client: Option<&ContextClient>,
    context_id: ContextId,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    f: impl FnOnce() -> R,
) -> R {
    // Held across the synchronous `with_runtime_env` body below; dropped on
    // return. The guard owns a clone of the context's lock `Arc`, so the
    // context-cache eviction invariant (evict only when strong_count == 1)
    // continues to treat this context as busy while we apply.
    let _guard = match context_client {
        Some(client) => client.acquire_lock(&context_id).await,
        None => None,
    };
    calimero_storage::env::with_runtime_env(runtime_env.clone(), f)
}

/// [`handle_entity_push`] under the per-context execution lock.
///
/// Use this from every production responder/initiator path. The lock is
/// released before the caller dispatches `deferred_root_merges` (those re-enter
/// the executor via `ContextClient::merge_root_state`, which would deadlock
/// against a held guard).
pub async fn handle_entity_push_locked(
    context_client: Option<&ContextClient>,
    store: &Store,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    context_id: ContextId,
    entities: &[TreeLeafData],
    session_peer: Option<PublicKey>,
) -> EntityPushOutcome {
    let _guard = match context_client {
        Some(client) => client.acquire_lock(&context_id).await,
        None => None,
    };
    handle_entity_push(store, runtime_env, context_id, entities, session_peer)
}

/// Apply a batch of tombstones (delete-wins by HLC) through the authenticated
/// `DeleteRef` path. Synchronous; the caller must already be holding the
/// per-context execution lock (see [`handle_entity_delete_push_locked`]).
///
/// A deletion that loses the LWW race or fails authorization is a safe no-op
/// and is not counted. Returns the number applied.
fn apply_entity_deletions(
    context_id: ContextId,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    deletions: &[EntityDeletion],
) -> u32 {
    calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
        let mut applied: u32 = 0;
        for deletion in deletions {
            let action = Action::DeleteRef {
                id: Id::new(deletion.id),
                deleted_at: deletion.deleted_at,
                metadata: deletion.metadata.clone(),
            };
            match Interface::<MainStorage>::apply_action(action, &ApplyContext::empty()) {
                Ok(_) => applied += 1,
                Err(e) => tracing::debug!(
                    %context_id,
                    id = %hex::encode(deletion.id),
                    error = %e,
                    "EntityDeletePush: skipped a tombstone (lost LWW or unauthorized)"
                ),
            }
        }
        applied
    })
}

/// Apply a batch of tombstones under the per-context execution lock.
///
/// Same split-brain guard as [`handle_entity_push_locked`]: a tombstone apply
/// is a read-modify-write up to the root and must not interleave with a
/// concurrent delta merge.
pub async fn handle_entity_delete_push_locked(
    context_client: Option<&ContextClient>,
    context_id: ContextId,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    deletions: &[EntityDeletion],
) -> u32 {
    let _guard = match context_client {
        Some(client) => client.acquire_lock(&context_id).await,
        None => None,
    };
    apply_entity_deletions(context_id, runtime_env, deletions)
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
    fn select_peer_identity_prefers_an_authorized_member() {
        // Peer hosts two identities; `b` sorts first but is unauthorized,
        // `a` is authorized. The authorized one must be chosen so a peer
        // hosting several identities (some stale) is not wrongly blocked.
        let a = PublicKey::from([0xAA; 32]);
        let b = PublicKey::from([0x01; 32]); // sorts before `a`
        let hosted = BTreeSet::from([a, b]);
        assert_eq!(
            select_attributable_peer_identity(&hosted, |id| *id == a),
            Some(a)
        );
    }

    #[test]
    fn select_peer_identity_returns_a_member_when_none_authorized_so_gate_drops_it() {
        // A revoked peer hosts only unauthorized identities. We must still
        // return `Some(_)`: passing `None` to `is_leaf_currently_authorized`
        // would ALLOW the authorless leaf (the no-attribution path). Returning
        // a now-unauthorized identity makes that gate drop the leaf.
        let b = PublicKey::from([0x02; 32]);
        let hosted = BTreeSet::from([b]);
        assert_eq!(
            select_attributable_peer_identity(&hosted, |_| false),
            Some(b)
        );
    }

    #[test]
    fn select_peer_identity_is_none_when_peer_hosts_no_identities() {
        let hosted: BTreeSet<PublicKey> = BTreeSet::new();
        assert_eq!(select_attributable_peer_identity(&hosted, |_| true), None);
    }

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
            writers: std::collections::BTreeMap::from([(
                signer,
                calimero_storage::entities::OpMask::FULL,
            )]),
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
            writers: std::collections::BTreeMap::from([(
                PublicKey::from([1u8; 32]),
                calimero_storage::entities::OpMask::FULL,
            )]),
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

    // ---- PR-6b / #2539 sync-repair coverage: future-schema leaf is buffered ----

    use calimero_node_primitives::sync::{LeafMetadata, TreeLeafData};
    use calimero_primitives::context::ContextId;
    use calimero_primitives::crdt::CrdtType;
    use calimero_storage::address::Id;
    use calimero_storage::index::Index;
    use calimero_storage::store::MainStorage;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use std::sync::Arc;

    fn opaque_leaf_with_schema(key: [u8; 32], schema: Option<[u8; 32]>) -> TreeLeafData {
        // An opaque (non-root) LWW leaf — the simplest leaf the apply path
        // stores directly without WASM dispatch.
        let mut md = LeafMetadata::new(CrdtType::lww_register("test"), 100, [0u8; 32]);
        if let Some(k) = schema {
            md = md.with_schema_app_key(k);
        }
        TreeLeafData::new(key, b"v2-bytes".to_vec(), md)
    }

    #[test]
    fn leaf_with_future_schema_is_buffered_not_stored() {
        // The v1-binary-fed-v2-bytes corruption hazard: a receiver whose loaded
        // reader is v1 must DECLINE + BUFFER a leaf authored under v2 instead of
        // LWW-storing unreadable bytes. The leaf must NOT be persisted.
        let context_id = ContextId::from([0xCA; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = calimero_node_primitives::sync::storage_bridge::create_runtime_env(
            &store, context_id, identity,
        );

        let leaf_key = [0x42u8; 32];
        let leaf = opaque_leaf_with_schema(leaf_key, Some([2u8; 32])); // v2
        let loaded_v1 = [1u8; 32];

        let outcome = calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
            apply_leaf_with_crdt_merge_gated(&store, context_id, &leaf, loaded_v1)
        })
        .expect("gated apply must not error");

        assert!(
            matches!(outcome, LeafOutcome::Buffered),
            "future-schema leaf must be buffered, got {outcome:?}"
        );

        // Must not have persisted the unreadable bytes.
        let stored = calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
            Index::<MainStorage>::get_index(Id::new(leaf_key))
                .ok()
                .flatten()
        });
        assert!(stored.is_none(), "future-schema leaf must NOT be stored");

        // And it landed in the absorb buffer for a later drain.
        let pending = calimero_context::group_store::AbsorbRepository::new(&store)
            .enumerate_pending(&context_id)
            .expect("enumerate pending");
        assert_eq!(pending.len(), 1, "future-schema leaf must be buffered");
    }

    #[test]
    fn leaf_with_matching_schema_applies() {
        let context_id = ContextId::from([0xCB; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = calimero_node_primitives::sync::storage_bridge::create_runtime_env(
            &store, context_id, identity,
        );

        let leaf_key = [0x43u8; 32];
        let loaded = [1u8; 32];
        let leaf = opaque_leaf_with_schema(leaf_key, Some(loaded)); // same schema

        let outcome = calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
            apply_leaf_with_crdt_merge_gated(&store, context_id, &leaf, loaded)
        })
        .expect("gated apply must not error");

        assert!(matches!(outcome, LeafOutcome::Applied));
    }

    #[test]
    fn legacy_leaf_without_schema_marker_applies() {
        // Back-compat: an older peer's leaf carries `schema_app_key = None`.
        // Treat as "no newer schema" → Apply (never buffer).
        let context_id = ContextId::from([0xCC; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = calimero_node_primitives::sync::storage_bridge::create_runtime_env(
            &store, context_id, identity,
        );

        let leaf_key = [0x44u8; 32];
        let leaf = opaque_leaf_with_schema(leaf_key, None); // legacy: no marker

        let outcome = calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
            apply_leaf_with_crdt_merge_gated(&store, context_id, &leaf, [1u8; 32])
        })
        .expect("gated apply must not error");

        assert!(matches!(outcome, LeafOutcome::Applied));
    }

    // ---- PR-6b fail-closed: a store error resolving the loaded reader must
    //      NOT disable the schema gate (no silent ungated apply). ----

    #[test]
    fn store_error_resolving_gate_skips_batch_not_applies() {
        // A transient store error while resolving the loaded reader schema must
        // fail CLOSED: the batch is skipped (re-pushed next sync), NOT applied
        // ungated. The old `.ok().flatten()` collapsed `Err` into `None` and
        // would have LWW-stored the (possibly future-schema) leaf.
        let context_id = ContextId::from([0xCD; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = calimero_node_primitives::sync::storage_bridge::create_runtime_env(
            &store, context_id, identity,
        );

        let leaf_key = [0x45u8; 32];
        // No schema marker — under `Ok(None)` (no gate) this WOULD apply, so
        // the only thing that keeps it out of storage is the fail-closed skip.
        let leaf = opaque_leaf_with_schema(leaf_key, None);

        let outcome = apply_entity_push_batch(
            &store,
            &runtime_env,
            context_id,
            std::slice::from_ref(&leaf),
            Err(eyre::eyre!("simulated transient store error")),
            None,
        );

        assert_eq!(
            outcome.applied, 0,
            "fail-closed: a store error must skip the batch, not apply it"
        );

        let stored = calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
            Index::<MainStorage>::get_index(Id::new(leaf_key))
                .ok()
                .flatten()
        });
        assert!(
            stored.is_none(),
            "store error must NOT result in an ungated apply/store"
        );
    }

    #[test]
    fn no_gate_ok_none_still_applies_leaf() {
        // Distinct from the `Err` case: `Ok(None)` is the legitimate
        // "no group / unresolvable meta" case and MUST still apply as today.
        let context_id = ContextId::from([0xCE; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = calimero_node_primitives::sync::storage_bridge::create_runtime_env(
            &store, context_id, identity,
        );

        let leaf_key = [0x46u8; 32];
        let leaf = opaque_leaf_with_schema(leaf_key, None);

        let outcome = apply_entity_push_batch(
            &store,
            &runtime_env,
            context_id,
            std::slice::from_ref(&leaf),
            Ok(None),
            None,
        );

        assert_eq!(
            outcome.applied, 1,
            "Ok(None) legitimate-no-gate case must apply the leaf"
        );
    }
}

#[cfg(test)]
mod empty_chain_placement_tests {
    // Regression tests for the HashComparison receiver guard that prevents
    // the same-DAG-heads / different-root split-brain (scaffolding-e2e run
    // 26679287804): a nested entity pushed without its ancestor chain must
    // not be guessed onto the context root.

    #[test]
    fn safe_when_parent_is_the_context_root() {
        // Direct child of the root has no intermediate ancestors, so the
        // single-parent fallback lands it at the correct Merkle position,
        // whether or not the root entry is materialised locally yet.
        assert!(super::empty_chain_placement_is_safe(true, false));
        assert!(super::empty_chain_placement_is_safe(true, true));
    }

    #[test]
    fn safe_when_nonroot_parent_already_exists_locally() {
        // The parent collection is present, so its ancestry is already
        // established and linking the leaf directly under it is exact.
        assert!(super::empty_chain_placement_is_safe(false, true));
    }

    #[test]
    fn unsafe_for_nested_entity_whose_parent_is_absent() {
        // The bug: a non-root parent that is NOT present locally. Falling
        // back to a single-parent chain makes `apply_action` `add_root` the
        // missing parent and misplace the entity under the context root,
        // producing a divergent root hash HashComparison cannot heal. The
        // apply path must defer instead of placing it, so the predicate
        // must report "unsafe" here.
        assert!(!super::empty_chain_placement_is_safe(false, false));
    }

    use super::{scope_verdict, ScopeVerdict};
    const A: [u8; 32] = [0xAA; 32];
    const B: [u8; 32] = [0xBB; 32];
    const E1: [u8; 32] = [0x11; 32];
    const E2: [u8; 32] = [0x22; 32];

    #[test]
    fn scope_verdict_both_resolved_and_equal_is_converged() {
        assert_eq!(
            scope_verdict(Some(A), Some(A), E1, E2),
            ScopeVerdict::Converged
        );
        assert!(scope_verdict(Some(A), Some(A), E1, E2).converged());
    }

    #[test]
    fn scope_verdict_scope_differs_entities_agree_is_gov_diverged() {
        // scope_root is authoritative: entities matching doesn't mean converged.
        // The verdict carries the resolved roots so callers log without re-unwrapping.
        assert_eq!(
            scope_verdict(Some(A), Some(B), E1, E1),
            ScopeVerdict::GovDiverged(A, B)
        );
        assert!(!scope_verdict(Some(A), Some(B), E1, E1).converged());
    }

    #[test]
    fn scope_verdict_scope_and_entities_both_differ_is_data_diverged() {
        assert_eq!(
            scope_verdict(Some(A), Some(B), E1, E2),
            ScopeVerdict::DataDiverged
        );
    }

    #[test]
    fn scope_verdict_cold_projection_falls_back_to_entity_compare() {
        // Either side `None` ⇒ pre-C1 entity-root compare (no regression to weaker).
        assert_eq!(
            scope_verdict(None, Some(A), E1, E1),
            ScopeVerdict::Converged
        );
        assert_eq!(
            scope_verdict(Some(A), None, E1, E2),
            ScopeVerdict::DataDiverged
        );
        assert_eq!(scope_verdict(None, None, E1, E1), ScopeVerdict::Converged);
    }
}
