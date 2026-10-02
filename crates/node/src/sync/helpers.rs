//! Common helper functions for sync protocols.
//!
//! **DRY Principle**: Extract repeated logic from protocol implementations.
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::sync::{
    EntityDeletion, InitPayload, MessagePayload, StreamMessage, SyncTransport, TreeLeafData,
    MAX_RESPONSE_BYTES,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::{CrdtType, CustomTypeId};
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::collections::ROOT_ENTRY_ID;
use calimero_storage::entities::{ChildInfo, Metadata, StorageType};
use calimero_storage::index::Index;
use calimero_storage::interface::{Action, ApplyContext, Interface};
use calimero_storage::store::MainStorage;
use calimero_store::Store;
use eyre::{bail, Result};
use rand::RngExt;

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
    let group = calimero_governance_store::get_group_for_context(store, context_id)
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
    rand::rng().random()
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
/// * `User` → the author is `signature_data.signer`, same as the two below.
///   Its `owner` names the ACCOUNT allowed to change the entry, and an account
///   is not a key this membership check can use — the device that actually
///   wrote it is the signer.
/// * `Shared { signature_data: Some(SignatureData { signer: Some(pk), .. }), .. }`
///   → the signature names its signer, which is the author. When `signer`
///   is `None` there is no author to name, so this returns `None` and the
///   caller treats it as "don't enforce membership here, defer to the
///   per-action signature check inside `apply_action`" — which refuses an
///   unnamed signed write outright.
/// * `Public` / `Frozen` / authorization absent → `None`; no author to
///   check (the per-action signature path verifies what's verifiable).
fn extract_author_from_leaf_authorization(
    authorization: Option<&StorageType>,
) -> Option<PublicKey> {
    match authorization? {
        StorageType::User { signature_data, .. }
        | StorageType::Shared { signature_data, .. }
        | StorageType::SharedMember { signature_data, .. } => {
            signature_data.as_ref().and_then(|sd| sd.signer)
        }
        StorageType::Public | StorageType::Frozen => None,
    }
}

/// The account a signed leaf says it was written on behalf of, if any.
///
/// Such a leaf is signed by a relay's key (its `signer`) for the account named
/// here; storage judges it against that account once the node has resolved the
/// leaf to it, and the node does so only when the signer may write for it
/// (`calimero_governance_store::on_behalf_standing`).
fn on_behalf_of_leaf(authorization: Option<&StorageType>) -> Option<calimero_account::AccountId> {
    match authorization? {
        StorageType::User { signature_data, .. }
        | StorageType::Shared { signature_data, .. }
        | StorageType::SharedMember { signature_data, .. } => {
            signature_data.as_ref().and_then(|sd| sd.on_behalf)
        }
        StorageType::Public | StorageType::Frozen => None,
    }
}

/// [`is_leaf_currently_authorized`] for a leaf written on an account's behalf.
///
/// The signer is a relay, whose role (`RelayTee`) is read-only, so the
/// author-based gate would drop every such leaf. The question is instead the
/// on-behalf rule, asked of the live rows as the rest of repair is: the signer's
/// account a `RelayTee` here, and the account written for a current member who
/// may write. A `User` leaf must then be owned by that account. A signer whose
/// binding has not folded here is dropped, like a non-member, and the leaf is
/// re-driven by the next repair round.
fn on_behalf_leaf_currently_authorized(
    store: &Store,
    context_id: &ContextId,
    leaf: &TreeLeafData,
    signer: &PublicKey,
    on_behalf: calimero_account::AccountId,
) -> bool {
    let standing =
        calimero_governance_store::get_group_for_context(store, context_id).and_then(|group| {
            // A context in no group has no namespace a relay could stand in.
            let Some(group_id) = group else {
                return Ok(None);
            };
            let Some(relay) =
                calimero_governance_store::member_account_in_namespace(store, &group_id, signer)?
            else {
                return Ok(None);
            };
            calimero_governance_store::on_behalf_standing_live(store, &group_id, relay, on_behalf)
                .map(|verdict| Some(verdict.is_ok()))
        });
    match standing {
        Ok(Some(true)) => {
            let owns = match leaf.metadata.authorization.as_ref() {
                Some(StorageType::User { owner, .. }) => *owner == on_behalf,
                _ => true,
            };
            if !owns {
                crate::node_metrics::record_hc_leaf_drop("not-entry-owner");
            }
            owns
        }
        Ok(Some(false) | None) => {
            crate::node_metrics::record_hc_leaf_drop("unauthorized");
            false
        }
        Err(err) => {
            tracing::error!(
                %context_id,
                %signer,
                error = %err,
                "is_leaf_currently_authorized: on-behalf standing lookup failed; dropping entity"
            );
            crate::node_metrics::record_hc_leaf_drop("lookup_error");
            false
        }
    }
}

/// Does a `User` leaf's author actually own it?
///
/// The ownership half of the authored-entry gate, and it lives here rather than
/// in `calimero-storage` because it is the half that needs device→account
/// bindings — which this crate can read and that one cannot. On the delta path
/// storage answers it itself, from the account the node resolved at the action's
/// causal cut. The sync repair paths (HashComparison, level-wise) carry no cut,
/// so storage defers there and this runs instead; see
/// `Interface::user_action_verdict`. Snapshot apply runs
/// [`snapshot_leaf_authorship`], which asks the same question of every binding
/// ever folded rather than only the live ones.
///
/// Without it the account flip would have been a straight downgrade on those
/// paths: `owner` used to BE the key the signature verified against, so a
/// repaired leaf was cryptographically bound to its owner even with no bindings
/// in hand. Once `owner` is an account that binding has to be re-made by
/// resolving the signer, and any context member could otherwise overwrite
/// another member's authored entry by pushing it over HC.
///
/// Non-`User` leaves pass straight through — they have no owner to check. So
/// does a leaf whose group or binding cannot be resolved: that is this node not
/// having folded the author's enrolment yet, not a claim that the author is an
/// impostor, and the membership check above has already run.
fn user_leaf_author_is_its_owner(
    store: &Store,
    context_id: &ContextId,
    leaf: &TreeLeafData,
    author: &PublicKey,
) -> bool {
    let Some(StorageType::User { owner, .. }) = leaf.metadata.authorization.as_ref() else {
        return true;
    };
    let Ok(Some(group_id)) = calimero_governance_store::get_group_for_context(store, context_id)
    else {
        return true;
    };
    match calimero_governance_store::member_account_in_namespace(store, &group_id, author) {
        Ok(Some(account)) => {
            let owns = account == *owner;
            if !owns {
                crate::node_metrics::record_hc_leaf_drop("not-entry-owner");
            }
            owns
        }
        // Unresolvable, not disproven — see the doc above.
        Ok(None) | Err(_) => true,
    }
}

/// Authorization gate for sync apply paths that don't carry a per-leaf
/// governance position on the wire (HashComparison EntityPush, level-wise).
/// Mirrors `state_delta_bridge`'s cross-DAG `membership_status_at` check,
/// coarsened to the receiver's *current* group state.
///
/// Snapshot apply does not run this. It would drop, from every cold joiner, the
/// state of anyone who has since left or lost a device, with nothing to repair
/// it; see [`snapshot_leaf_authorship`] for the gate it runs instead.
///
/// Returns `true` iff the entity should be applied:
/// * No identifiable author → applied only if the session peer may write
///   the context (see [`authorless_write_allowed`]).
/// * Author identified + currently a member of `context_id`'s owning
///   group → applied.
/// * Author identified + NOT currently a member (or lookup error) →
///   dropped. Closes the HC back door where a now-removed author's
///   entities entered storage without re-running the membership check
///   that the gossip path runs unconditionally. The trade-off (over-
///   rejection of legitimate pre-removal writes that propagate via HC)
///   is documented on
///   [`calimero_governance_store::is_currently_authorized_for_context`].
pub fn is_leaf_currently_authorized(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    leaf: &TreeLeafData,
    session_peer: Option<PublicKey>,
) -> bool {
    let author = match extract_author_from_leaf_authorization(leaf.metadata.authorization.as_ref())
    {
        Some(author) => author,
        None => {
            // Authorless PLAIN (Public) leaf — carries no signer, so the
            // author-based gate below can't apply. A removed peer's push is
            // dropped at the first hop, so the write never propagates further.
            return authorless_write_allowed(store, folded, context_id, session_peer);
        }
    };
    if let Some(on_behalf) = on_behalf_of_leaf(leaf.metadata.authorization.as_ref()) {
        return on_behalf_leaf_currently_authorized(store, context_id, leaf, &author, on_behalf);
    }
    match calimero_governance_store::is_currently_authorized_for_context(
        store, folded, context_id, &author,
    ) {
        Ok(true) => user_leaf_author_is_its_owner(store, context_id, leaf, &author),
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

/// An entry naming no author is admitted only by the session peer's current write
/// authority; a peer that cannot be attributed writes only where no group governs.
fn authorless_write_allowed(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    session_peer: Option<PublicKey>,
) -> bool {
    match session_peer {
        Some(peer) => calimero_governance_store::is_currently_authorized_for_context(
            store, folded, context_id, &peer,
        )
        .unwrap_or(false),
        None => matches!(
            calimero_governance_store::get_group_for_context(store, context_id),
            Ok(None)
        ),
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
    /// `schema_bytecode_id` is newer than the receiver's loaded reader. It will be
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
/// receiver's **loaded** reader (`loaded_bytecode_id`, i.e. `loaded_reader_bytecode_id`)
/// rather than the replicated `GroupMeta.bytecode_id` (the O3 correction):
///
/// * `leaf.metadata.schema_bytecode_id == Some(k)` with `k != loaded_bytecode_id` —
///   the receiver lacks a reader for the incoming schema. **Decline + buffer**
///   the leaf verbatim into the absorb buffer (a leaf-shaped [`AbsorbRecord`])
///   and return [`LeafOutcome::Buffered`]. The bytes are NEVER stored; the
///   drain re-applies them once the reader advances.
/// * `schema_bytecode_id == None` (legacy peer) or `== Some(loaded_bytecode_id)` —
///   apply as today and return [`LeafOutcome::Applied`].
///
/// Must be called inside a `with_runtime_env(...)` scope (it delegates to
/// [`apply_leaf_with_crdt_merge`] on the apply branch).
/// The account a repair-pushed leaf's signer speaks for, for
/// [`ApplyContext::signer_account`].
///
/// A signed leaf carries the KEY that signed it; the writer set it must be
/// checked against names ACCOUNTS. Storage cannot bridge the two — it has no
/// bindings — so the node resolves it here and passes the answer in.
///
/// Resolving live is sound for this one question, and only because a device is
/// bound to exactly one account for its whole life (`BindingRejected::
/// AccountReassignment`). Two nodes that both know a binding therefore always
/// agree, whatever their fold depth; the only variance is knowing versus not
/// knowing yet. That is unlike the writer SET, which genuinely changes over
/// time — and which storage already resolves deterministically, as of the
/// leaf's own HLC, from the anchor's rotation log.
///
/// `None` means the binding has not folded here yet. Storage refuses on `None`,
/// which is the right answer: the leaf is re-driven by the next repair round
/// once the binding lands, so both peers converge on the same verdict instead
/// of one accepting what the other rejects.
fn repair_signer_account(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    leaf: &TreeLeafData,
) -> Option<calimero_account::AccountId> {
    signer_account_for(
        store,
        folded,
        context_id,
        leaf.metadata.authorization.as_ref(),
    )
}

/// [`repair_signer_account`] over any authorization stamp, so a tombstone —
/// whose `Metadata` carries the storage type directly rather than in a leaf's
/// `authorization` — resolves its signer the same way a pushed value does.
pub(crate) fn signer_account_for(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    authorization: Option<&StorageType>,
) -> Option<calimero_account::AccountId> {
    let signer = extract_author_from_leaf_authorization(authorization)?;
    let group_id = calimero_governance_store::get_group_for_context(store, context_id)
        .ok()
        .flatten()?;
    let account = calimero_governance_store::member_account_in_namespace(store, &group_id, &signer)
        .ok()
        .flatten()?;
    // Written on an account's behalf: the leaf is that account's, provided its
    // signer may write for it, and nobody's otherwise. Never the TEE-authority
    // mapping below, which is about a TEE writing as itself.
    if let Some(on_behalf) = on_behalf_of_leaf(authorization) {
        return calimero_governance_store::on_behalf_standing_live(
            store, &group_id, account, on_behalf,
        )
        .ok()?
        .ok()
        .map(|()| on_behalf);
    }
    // The same TEE-authority mapping the delta path's resolver applies. Without
    // it, a TEE's `TeeOnly` writes reach a peer by delta but are refused when
    // they arrive by repair, so a peer that catches up by repair never gets them.
    //
    // A lookup error keeps the signer's own account rather than refusing. Only a
    // positive answer maps anyone to the TEE authority, so the fallback can never
    // grant a write; refusing instead turned a transient governance read error on
    // a joining node into a repair it could never complete.
    Some(
        calimero_governance_store::writer_account(store, folded, &group_id, &signer, account)
            .unwrap_or(account),
    )
}

/// Whether a snapshot leaf may be stored under the TEE-only rule.
///
/// Stricter than [`snapshot_leaf_authorship`], which asks only whether the
/// signer's account is in the writer set. A leaf whose writer set holds the TEE
/// authority at all is stored only when its signer resolves to the TEE
/// authority, by the same rule the merge path uses, so no other writer listed
/// beside it can author `TeeOnly` state on a cold joiner. Anything else is
/// dropped like a bad signature.
///
/// `writers` is the leaf's own set for a `Shared` anchor, and its anchor's set
/// for a `SharedMember`.
pub(crate) fn snapshot_leaf_admitted(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    writers: &std::collections::BTreeMap<
        calimero_account::AccountId,
        calimero_storage::entities::OpMask,
    >,
    storage_type: &StorageType,
) -> bool {
    tee_only_leaf_admitted(writers, || {
        signer_account_for(store, folded, context_id, Some(storage_type))
    })
}

/// [`snapshot_leaf_admitted`] with the signer resolution passed in, so the rule
/// can be tested without a governance store. The resolver runs only for a
/// TEE-only leaf.
fn tee_only_leaf_admitted<W>(
    writers: &std::collections::BTreeMap<calimero_account::AccountId, W>,
    resolve_signer: impl FnOnce() -> Option<calimero_account::AccountId>,
) -> bool {
    !writers.contains_key(&calimero_account::AccountId::TEE_AUTHORITY)
        || resolve_signer() == Some(calimero_account::AccountId::TEE_AUTHORITY)
}

/// What a snapshot leaf's signer proves about who wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotAuthorship {
    /// The signer's account is the leaf's owner or one of its writers, or the
    /// leaf names no account to check against (Public, Frozen, a rotation-log
    /// child, or a context in no group).
    Authored,
    /// The signer's account is known and is neither the owner nor a writer. The
    /// leaf is a forgery by whoever signed it; drop it.
    Forged,
    /// No certificate for the signer has been folded here, so the question
    /// cannot be answered yet. Fail the snapshot and retry; dropping would leave
    /// a hole the shipped root hides from repair.
    Unknown,
}

/// Whether a snapshot leaf was written by an account allowed to write it (#4089).
///
/// A snapshot leaf is state, not an op, so it has no causal cut to ask "was the
/// signer a writer *then*" against, and the storage-layer check verifies only
/// that the signature is genuine. That let an admitted source serve a `User`
/// entry under someone else's `owner`, or a `Shared` entry under a writer set its
/// signer is not in, signed with a key of its own.
///
/// This closes that with the part of the question that has no timing in it:
/// whose key signed the leaf. The key is resolved through every certificate
/// ever folded here ([`calimero_governance_store::signer_account_in_namespace`]),
/// not the live binding, so a member who has since left, been downgraded, or had
/// the signing device revoked keeps the state they wrote. Asking whether the
/// author may write *now*, as HashComparison does, would silently drop all of
/// that from every cold joiner, because the root check reads the shipped root
/// index and would still pass.
///
/// `anchor_writers` is the writer set of a `SharedMember`'s anchor; `None` for
/// every other storage type.
///
/// Not checked here, deliberately:
/// * `Public` / `Frozen` leaves carry no signer. The source vouches for them,
///   and it has proved it is admitted to the context
///   (`SyncManager::ensure_snapshot_server_admitted`).
/// * Whether a `Shared` signer was in the writer set when it signed rather than
///   in the set the leaf carries now. A writer who removed themselves in their
///   last write signs a leaf whose set no longer names them, so this reports
///   it `Forged`; snapshot apply then consults the anchor's rotation log
///   ([`rotation_removed_the_signer`]) before dropping it.
pub(crate) fn snapshot_leaf_authorship(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    metadata: &Metadata,
    anchor_writers: Option<
        &std::collections::BTreeMap<
            calimero_account::AccountId,
            calimero_storage::entities::OpMask,
        >,
    >,
) -> SnapshotAuthorship {
    let group_id = match calimero_governance_store::get_group_for_context(store, context_id) {
        Ok(Some(group_id)) => group_id,
        Ok(None) => return SnapshotAuthorship::Authored,
        Err(_) => return SnapshotAuthorship::Unknown,
    };
    authorship_verdict(
        &metadata.storage_type,
        anchor_writers,
        |signer| {
            calimero_governance_store::signer_account_in_namespace(store, &group_id, signer)
                .ok()
                .flatten()
        },
        // The TEE-authority mapping `signer_account_for` applies, so a TEE's
        // leaf in a TEE-only entry names the writer the set holds. A lookup
        // error keeps the signer's own account, which can only refuse.
        |signer, account| {
            calimero_governance_store::writer_account(store, folded, &group_id, signer, account)
                .unwrap_or(account)
        },
        |relay, on_behalf| snapshot_relay_may_write_for(store, &group_id, relay, on_behalf),
    )
}

/// Whether a snapshot leaf written on `on_behalf`'s behalf was signed by a party
/// that may write for it, `None` on a lookup error.
///
/// Only the signer half of the on-behalf rule decides here: its account must be
/// a `RelayTee` (live, there being no cut). Whether the account written for may
/// write *now* does not, for the reason [`snapshot_leaf_authorship`] gives for
/// any author: a member who has since left keeps the state written for them.
///
/// The relay half is live because the snapshot carries no cut and the projection
/// keeps no history of roles. So the state a relay wrote is dropped from a cold
/// joiner once that relay stops being a `RelayTee` (removed, or switched back to
/// a replica); the relay that wrote it vouched for it, and a forger signing as a
/// member could otherwise serve anyone's entries.
fn snapshot_relay_may_write_for(
    store: &Store,
    group_id: &calimero_context_config::types::ContextGroupId,
    relay: calimero_account::AccountId,
    on_behalf: calimero_account::AccountId,
) -> Option<bool> {
    use calimero_governance_store::OnBehalfRefusal;

    match calimero_governance_store::on_behalf_standing_live(store, group_id, relay, on_behalf) {
        Ok(
            Ok(()) | Err(OnBehalfRefusal::AccountNotAMember | OnBehalfRefusal::AccountIsReadOnly),
        ) => Some(true),
        Ok(Err(OnBehalfRefusal::SignerNotARelay)) => Some(false),
        Err(_) => None,
    }
}

/// Whether a `Shared` snapshot leaf that [`snapshot_leaf_authorship`] found
/// [`Forged`](SnapshotAuthorship::Forged) was in fact written by the rotation
/// that removed its signer from the writer set.
///
/// A writer may remove themselves. Their rotation is the entry's last write, so
/// the stored leaf carries the new set, which no longer names them, and their
/// signature. Checked only against the set it carries, that honest leaf reads as
/// a forgery and every cold joiner drops it. The anchor's rotation log settles
/// it: the leaf is rescued when the log's latest authenticated rotation was
/// signed by the leaf's signer, produced exactly the writer set the leaf
/// carries, and the signer's account was a writer in the set before it.
///
/// `entries` are the anchor's rotation-log entries as delivered by the same
/// snapshot. They are untrusted in transit; an entry counts only if its own
/// signature verifies ([`crate::delta_store::verify_rotation_entry`]).
pub(crate) fn rotation_removed_the_signer(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    metadata: &Metadata,
    entries: &[calimero_storage::rotation_log::RotationLogEntry],
) -> bool {
    let StorageType::Shared { writers, .. } = &metadata.storage_type else {
        return false;
    };
    let Some((signer, account, writer)) =
        snapshot_signer_accounts(store, folded, context_id, metadata)
    else {
        return false;
    };
    latest_rotation_removed(
        entries,
        writers,
        &signer,
        |prior| prior.contains_key(&account) || prior.contains_key(&writer),
        crate::delta_store::verify_rotation_entry,
    )
}

/// Whether a `SharedMember` snapshot leaf's signer was one of its anchor's
/// writers when the member was written, though no longer.
///
/// A rotation re-signs the anchor it changes, not the anchor's members. So a
/// member written by a writer whom a later rotation removed, whether they
/// removed themselves or another writer removed them, still carries that
/// writer's signature, and [`snapshot_leaf_authorship`] finds it
/// [`Forged`](SnapshotAuthorship::Forged) against the anchor's current set. The
/// anchor's rotation log answers the question it cannot: the member is kept when
/// its signer's account is in the set in effect at the member's own timestamp
/// ([`rotation_log::resolve_local_as_of`](calimero_storage::rotation_log::resolve_local_as_of),
/// the resolver HashComparison uses for the same case).
///
/// `entries` are the anchor's rotation-log entries as delivered by the same
/// snapshot, untrusted in transit; only those whose own signature verifies are
/// considered. The timestamp is the author's, so a removed writer could backdate
/// a member past their removal. The same holds on the HashComparison path, and
/// the member still has to be served by an admitted source.
pub(crate) fn member_signer_was_a_writer_then(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    metadata: &Metadata,
    entries: &[calimero_storage::rotation_log::RotationLogEntry],
) -> bool {
    if !matches!(metadata.storage_type, StorageType::SharedMember { .. }) {
        return false;
    }
    let Some((_, account, writer)) = snapshot_signer_accounts(store, folded, context_id, metadata)
    else {
        return false;
    };
    writer_when_written(
        entries,
        *metadata.updated_at,
        |set| set.contains_key(&account) || set.contains_key(&writer),
        crate::delta_store::verify_rotation_entry,
    )
}

/// A snapshot leaf's signer, the account it was certified for, and that
/// account as a writer set names it (the TEE-authority mapping). `None` when
/// the leaf names no signer, its context is in no group, or no certificate for
/// the key has been folded.
fn snapshot_signer_accounts(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    metadata: &Metadata,
) -> Option<(
    PublicKey,
    calimero_account::AccountId,
    calimero_account::AccountId,
)> {
    let signer = extract_author_from_leaf_authorization(Some(&metadata.storage_type))?;
    let group_id = calimero_governance_store::get_group_for_context(store, context_id)
        .ok()
        .flatten()?;
    let account = calimero_governance_store::signer_account_in_namespace(store, &group_id, &signer)
        .ok()
        .flatten()?;
    // Written on an account's behalf by a relay that may write for it: the leaf
    // is that account's, as `snapshot_leaf_authorship` decides it.
    if let Some(on_behalf) = on_behalf_of_leaf(Some(&metadata.storage_type)) {
        return snapshot_relay_may_write_for(store, &group_id, account, on_behalf)
            .filter(|may| *may)
            .map(|_| (signer, on_behalf, on_behalf));
    }
    let writer =
        calimero_governance_store::writer_account(store, folded, &group_id, &signer, account)
            .unwrap_or(account);
    Some((signer, account, writer))
}

/// [`member_signer_was_a_writer_then`] with the account check and the entry
/// signature check passed in, so the rule can be tested without a store.
fn writer_when_written(
    entries: &[calimero_storage::rotation_log::RotationLogEntry],
    written_at: u64,
    is_writer: impl Fn(
        &std::collections::BTreeMap<calimero_account::AccountId, calimero_storage::entities::OpMask>,
    ) -> bool,
    verify: impl Fn(&calimero_storage::rotation_log::RotationLogEntry) -> bool,
) -> bool {
    let authenticated = calimero_storage::rotation_log::RotationLog {
        snapshot: None,
        entries: entries
            .iter()
            .filter(|entry| verify(entry))
            .cloned()
            .collect(),
    };
    calimero_storage::rotation_log::resolve_local_as_of(&authenticated, written_at)
        .is_some_and(|set| is_writer(&set))
}

/// [`rotation_removed_the_signer`] with the account lookup and the entry
/// signature check passed in, so the rule can be tested without a store.
fn latest_rotation_removed(
    entries: &[calimero_storage::rotation_log::RotationLogEntry],
    leaf_writers: &std::collections::BTreeMap<
        calimero_account::AccountId,
        calimero_storage::entities::OpMask,
    >,
    signer: &PublicKey,
    signer_was_a_writer_in: impl Fn(
        &std::collections::BTreeMap<calimero_account::AccountId, calimero_storage::entities::OpMask>,
    ) -> bool,
    verify: impl Fn(&calimero_storage::rotation_log::RotationLogEntry) -> bool,
) -> bool {
    use core::cmp::Ordering;

    // The order `rotation_log::resolve_local` resolves the current set by: HLC,
    // then the smaller signer, with an unsigned entry losing ties.
    fn order(
        a: &calimero_storage::rotation_log::RotationLogEntry,
        b: &calimero_storage::rotation_log::RotationLogEntry,
    ) -> Ordering {
        a.delta_hlc
            .cmp(&b.delta_hlc)
            .then_with(|| match (&a.signer, &b.signer) {
                (Some(sa), Some(sb)) => sb.digest().cmp(sa.digest()),
                (Some(_), None) => Ordering::Greater,
                (None, Some(_)) => Ordering::Less,
                (None, None) => Ordering::Equal,
            })
    }

    let mut authenticated: Vec<_> = entries.iter().filter(|entry| verify(entry)).collect();
    authenticated.sort_by(|a, b| order(a, b));
    let [.., prior, latest] = authenticated.as_slice() else {
        // No rotation, or only the one that created the set: nothing records a
        // set the signer was in before.
        return false;
    };
    latest.signer.as_ref() == Some(signer)
        && latest.new_writers == *leaf_writers
        && signer_was_a_writer_in(&prior.new_writers)
}

/// [`snapshot_leaf_authorship`] with the governance reads passed in, so the rule
/// can be tested without a store. `signer_account` returns `None` when the key
/// has no certified account here.
///
/// `may_write_for(signer_account, on_behalf)` answers for a leaf written on an
/// account's behalf, `None` when it cannot be read; such a leaf is the account's
/// when it does, and a forgery when it does not.
fn authorship_verdict<W>(
    storage_type: &StorageType,
    anchor_writers: Option<&std::collections::BTreeMap<calimero_account::AccountId, W>>,
    signer_account: impl FnOnce(&PublicKey) -> Option<calimero_account::AccountId>,
    writer_account: impl FnOnce(&PublicKey, calimero_account::AccountId) -> calimero_account::AccountId,
    may_write_for: impl FnOnce(calimero_account::AccountId, calimero_account::AccountId) -> Option<bool>,
) -> SnapshotAuthorship {
    let Some(signer) = extract_author_from_leaf_authorization(Some(storage_type)) else {
        return SnapshotAuthorship::Authored;
    };
    let Some(account) = signer_account(&signer) else {
        return SnapshotAuthorship::Unknown;
    };
    let on_behalf = on_behalf_of_leaf(Some(storage_type));
    let author = match on_behalf {
        None => account,
        Some(on_behalf) => match may_write_for(account, on_behalf) {
            Some(true) => on_behalf,
            Some(false) => return SnapshotAuthorship::Forged,
            None => return SnapshotAuthorship::Unknown,
        },
    };
    // A direct write may also name the writer as the TEE authority; an
    // on-behalf one is its account's and nobody else's.
    let writer = || {
        on_behalf
            .is_none()
            .then(|| writer_account(&signer, account))
    };
    let authored = match storage_type {
        StorageType::User { owner, .. } => *owner == author,
        StorageType::Shared { writers, .. } => {
            writers.contains_key(&author) || writer().is_some_and(|w| writers.contains_key(&w))
        }
        StorageType::SharedMember { .. } => anchor_writers.is_some_and(|writers| {
            writers.contains_key(&author) || writer().is_some_and(|w| writers.contains_key(&w))
        }),
        // No signer, so returned above.
        StorageType::Public | StorageType::Frozen => true,
    };
    if authored {
        SnapshotAuthorship::Authored
    } else {
        SnapshotAuthorship::Forged
    }
}

/// What a receiver should do with one incoming leaf.
///
/// Both DFS walks — HashComparison and level-wise — face the same three-way
/// choice, and had it inline twice. It lives here so the two cannot drift, and
/// so the classification is testable without a transport.
#[derive(Debug, PartialEq, Eq)]
pub enum LeafDisposition {
    /// Apply it here and now; the storage layer can merge this itself.
    Apply,
    /// The app-state entry: defer for `__calimero_merge_root_state`.
    DeferRoot,
    /// A custom-typed entry: defer for `__calimero_merge_custom`, carrying the
    /// id the entry declares.
    DeferCustom(CustomTypeId),
}

/// Decide what to do with `leaf_metadata` for `entity_id`.
///
/// Neither deferral is an optimisation. A leaf whose merge rule lives in the
/// app's module cannot be merged from the DFS at all: the apply is synchronous
/// and runs inside `with_runtime_env`, so it cannot call into the runtime.
/// Applying it anyway falls through to last-write-wins, which contradicts the
/// in-WASM delta path and leaves the two replicas settling the same conflict
/// differently depending on which path delivered it.
///
/// The app-state entry defers and the root collection applies as a shell whatever
/// type the peer names, since the wire type is the peer's claim.
///
/// A custom entry defers only when `stored_locally` says this node holds a value
/// to merge it with. With nothing stored there is nothing to merge, and the
/// deferred pass skips such an entry ([`dispatch_deferred_custom_merges`]
/// leaves it "to the plain apply path"), so deferring it would deliver it
/// nowhere: a receiver that missed the entry — its delta's signed actions were
/// refused before the author's binding folded here — would never get it, and
/// the replicas would stay divergent on the same DAG heads. The plain apply
/// stores an entry it has no bytes for as it arrives, which is the only merge
/// one side admits.
///
/// [`dispatch_deferred_custom_merges`]: crate::sync::protocol_selector::dispatch_deferred_custom_merges
#[must_use]
pub fn classify_leaf(
    entity_id: Id,
    crdt_type: &CrdtType,
    stored_locally: impl FnOnce() -> bool,
) -> LeafDisposition {
    if entity_id == ROOT_ENTRY_ID {
        return LeafDisposition::DeferRoot;
    }
    if entity_id.is_root() {
        return LeafDisposition::Apply;
    }

    if let CrdtType::Custom(type_id) = crdt_type {
        if stored_locally() {
            return LeafDisposition::DeferCustom(*type_id);
        }
    }

    LeafDisposition::Apply
}

/// Whether this node stores a value for `entity_id`: the [`classify_leaf`]
/// question of whether a custom entry has anything to merge with here. Reads
/// the current runtime env's storage, so call it inside `with_runtime_env`.
pub fn stores_value(entity_id: Id) -> bool {
    <MainStorage as calimero_storage::store::StorageAdaptor>::storage_read(
        calimero_storage::store::Key::Entry(entity_id),
    )
    .is_some()
}

/// The schema `leaf` was written under, when the loaded reader is another one and
/// so cannot read it.
pub(crate) fn unreadable_schema(leaf: &TreeLeafData, loaded: [u8; 32]) -> Option<[u8; 32]> {
    leaf.metadata
        .schema_bytecode_id
        .filter(|schema| *schema != loaded)
}

pub fn apply_leaf_with_crdt_merge_gated(
    store: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: ContextId,
    leaf: &TreeLeafData,
    loaded_bytecode_id: [u8; 32],
) -> Result<LeafOutcome> {
    if let Some(schema) = unreadable_schema(leaf, loaded_bytecode_id) {
        // The receiver's loaded reader can't read this leaf — buffer it
        // verbatim instead of storing unreadable bytes. Keyed by the leaf
        // key (idempotent overwrite on re-delivery), under the *sender's*
        // schema so the drain only re-applies once this node advances to it.
        let leaf_bytes = borsh::to_vec(leaf)?;
        let record =
            calimero_governance_store::AbsorbRecord::from_leaf(leaf.key, leaf_bytes, schema);
        calimero_governance_store::AbsorbRepository::new(store).save(
            &context_id,
            schema,
            &record,
        )?;
        crate::node_metrics::record_delta_outcome("absorbed_leaf_future_schema");
        tracing::warn!(
            %context_id,
            key = %hex::encode(leaf.key),
            ?schema,
            ?loaded_bytecode_id,
            "sync-repair leaf authored under a newer schema than the loaded \
             reader — buffered into the absorb buffer instead of storing \
             unreadable bytes (will replay once the reader advances)"
        );
        return Ok(LeafOutcome::Buffered);
    }
    let signer_account = repair_signer_account(store, folded, &context_id, leaf);
    apply_leaf_with_crdt_merge_as(context_id, leaf, signer_account)?;
    Ok(LeafOutcome::Applied)
}

/// [`apply_leaf_with_crdt_merge_as`] for a caller that cannot name the signer's
/// account — no store in scope, or a test.
///
/// A signed leaf applied this way is REFUSED by storage and re-driven on the
/// next repair round. Prefer the gated entry point, which resolves it.
pub fn apply_leaf_with_crdt_merge(context_id: ContextId, leaf: &TreeLeafData) -> Result<()> {
    apply_leaf_with_crdt_merge_as(context_id, leaf, None)
}

/// Apply a repair-pushed leaf as `signer_account`.
///
/// The account is the node's half of the writer check: storage verifies the
/// signature under the key the leaf names, then checks that account against the
/// writer set resolved as of the leaf's own HLC. Passing `None` leaves storage
/// unable to name the writer, so it refuses — see [`repair_signer_account`].
pub fn apply_leaf_with_crdt_merge_as(
    context_id: ContextId,
    leaf: &TreeLeafData,
    signer_account: Option<calimero_account::AccountId>,
) -> Result<()> {
    let entity_id = Id::new(leaf.key);
    let root_id = Id::new(*context_id.as_ref());

    // The app-state entry merges in the app's module, so its callers defer it.
    if entity_id == ROOT_ENTRY_ID {
        tracing::debug!(
            %context_id,
            "HC apply: the app-state entry is merged by the deferred root dispatch"
        );
        return Ok(());
    }
    if entity_id.is_root() {
        let mut metadata = Metadata::default();
        metadata.created_at = leaf.metadata.created_at;
        metadata.updated_at = leaf.metadata.hlc_timestamp.into();
        let shell = Action::Update {
            id: entity_id,
            data: leaf.value.clone(),
            ancestors: vec![],
            metadata,
        };
        Interface::<MainStorage>::apply_remote_action(shell, &ApplyContext::empty())?;
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

    // No `CausalDelta` in scope (#2266): a repair carries state, not an op, so
    // there are no parents and `effective_writers` stays `None` — storage then
    // resolves the writer set as of this leaf's own HLC. What the node supplies
    // is the other half, the account its signer speaks for, without which the
    // writer check cannot run at all.
    let ctx = ApplyContext {
        signer_account,
        ..ApplyContext::empty()
    };
    Interface::<MainStorage>::apply_action(action, &ctx)?;
    Ok(())
}

/// Maximum entities per `EntityPush` message (shared between initiator and responder).
///
/// The initiator batches at this limit; the responder truncates messages exceeding it.
pub const MAX_ENTITIES_PER_PUSH: usize = 500;

/// Send entities to the peer as `EntityPush` batches, consuming one
/// `EntityPushAck` per batch. Returns `(applied, batches)` so the caller can
/// fold both into its own stats. A batch holds at most
/// [`MAX_ENTITIES_PER_PUSH`] entities and, past its first, no more than
/// [`MAX_RESPONSE_BYTES`] of them, so it always fits the stream's frame.
///
/// Shared by the HashComparison and LevelWise initiators: both repair a peer
/// through the same wire item, and a second copy of the batching would be a
/// second place for the request budget to drift.
pub(crate) async fn push_entities<T: SyncTransport>(
    transport: &mut T,
    context_id: ContextId,
    identity: PublicKey,
    leaves: &[TreeLeafData],
) -> Result<(u64, u64)> {
    let mut applied = 0u64;
    let mut batches = 0u64;

    for chunk in push_batches(leaves) {
        let push_msg = StreamMessage::Init {
            context_id,
            party_id: identity,
            payload: InitPayload::EntityPush {
                context_id,
                entities: chunk.to_vec(),
            },
            next_nonce: generate_nonce(),
            // Pushes are writes: each entity is authorized by its own action
            // path on apply, not by the sender's `party_id`, so no read-gating
            // proof is attached (or required by the responder) here.
            pop: None,
        };

        transport.send(&push_msg).await?;
        batches += 1;

        let ack = transport
            .recv()
            .await?
            .ok_or_else(|| eyre::eyre!("stream closed while waiting for EntityPushAck"))?;

        match ack {
            StreamMessage::Message {
                payload: MessagePayload::EntityPushAck { applied_count },
                ..
            } => applied += u64::from(applied_count),
            _ => {
                bail!("Unexpected response to EntityPush (peer may not support bidirectional sync)")
            }
        }
    }

    Ok((applied, batches))
}

/// `leaves` cut into `EntityPush` batches; see [`push_entities`].
fn push_batches(leaves: &[TreeLeafData]) -> impl Iterator<Item = &[TreeLeafData]> {
    let mut rest = leaves;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut bytes = 0usize;
        let mut len = 0;
        for leaf in rest.iter().take(MAX_ENTITIES_PER_PUSH) {
            // Measuring writes nowhere and cannot fail; `MAX` would only end
            // the batch early.
            bytes = bytes.saturating_add(borsh::object_length(leaf).unwrap_or(usize::MAX));
            if len > 0 && bytes > MAX_RESPONSE_BYTES {
                break;
            }
            len += 1;
        }
        let (batch, tail) = rest.split_at(len);
        rest = tail;
        Some(batch)
    })
}

/// Outcome of an EntityPush batch.
///
/// `applied` is the count of leaves successfully written via the host
/// CRDT apply path. `deferred_root_merges` collects app-state entry leaves
/// the host can't merge by itself (same rationale as
/// [`HashComparisonStats::deferred_root_merges`](crate::sync::hash_comparison_protocol::HashComparisonStats::deferred_root_merges)) —
/// the caller dispatches each through `ContextClient::merge_root_state`
/// after the batch returns.
#[derive(Debug, Default)]
pub struct EntityPushOutcome {
    pub applied: u32,
    /// App-state entry leaves, as in
    /// [`crate::sync::hash_comparison_protocol::HashComparisonStats::deferred_root_merges`].
    pub deferred_root_merges: Vec<TreeLeafData>,
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
/// App-state entry leaves are surfaced in `deferred_root_merges` for the
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
    let loaded_bytecode_id =
        calimero_context::hlc_fence::loaded_reader_bytecode_id(store, &context_id);
    apply_entity_push_batch(
        store,
        runtime_env,
        context_id,
        entities,
        loaded_bytecode_id,
        session_peer,
    )
}

/// Apply (or buffer) a pre-truncated, pre-resolved `EntityPush` batch.
///
/// `loaded_bytecode_id` is the resolution of the receiver's loaded reader schema:
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
    loaded_bytecode_id: Result<Option<[u8; 32]>>,
    session_peer: Option<PublicKey>,
) -> EntityPushOutcome {
    // One read of the namespace's TEE state for the whole batch, rather than
    // one op-log scan and one quote verification per leaf a TEE signed.
    let folded = calimero_governance_store::ScanOnce::default();
    let loaded_bytecode_id = match loaded_bytecode_id {
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
        let mut deferred_root_merges: Vec<TreeLeafData> = Vec::new();
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
            if !is_leaf_currently_authorized(store, &folded, &context_id, leaf, session_peer) {
                dropped_unauthorized += 1;
                tracing::warn!(
                    %context_id,
                    key = %hex::encode(leaf.key),
                    "pushed entity dropped: claimed author is not currently authorized for this context"
                );
                continue;
            }
            // The app-state entry merges in the app's module; the caller, which
            // holds the `ContextClient`, dispatches it.
            if classify_leaf(Id::new(leaf.key), &leaf.metadata.crdt_type, || false)
                == LeafDisposition::DeferRoot
            {
                deferred_root_merges.push(leaf.clone());
                continue;
            }
            let apply_result = match loaded_bytecode_id {
                Some(loaded) => apply_leaf_with_crdt_merge_gated(
                    store, &folded, context_id, leaf, loaded,
                )
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
    store: &Store,
    context_id: ContextId,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    deletions: &[EntityDeletion],
    session_peer: Option<PublicKey>,
) -> u32 {
    // One read of the namespace's TEE state for the whole batch, rather than
    // one op-log scan and one quote verification per leaf a TEE signed.
    let folded = calimero_governance_store::ScanOnce::default();
    calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
        let mut applied: u32 = 0;
        for deletion in deletions {
            let id = Id::new(deletion.id);
            if stored_as_public(id)
                && !authorless_write_allowed(store, &folded, &context_id, session_peer)
            {
                tracing::warn!(
                    %context_id,
                    id = %hex::encode(deletion.id),
                    "dropped a tombstone for a public entry from a peer that may not write"
                );
                continue;
            }
            let action = Action::DeleteRef {
                id,
                deleted_at: deletion.deleted_at,
                metadata: deletion.metadata.clone(),
            };
            // A tombstone for a signed entity is authorized like any other
            // write, so it needs the same account resolution a pushed value
            // gets — otherwise deletes of `User`/`Shared` entities stop
            // propagating on the repair paths.
            let ctx = ApplyContext {
                signer_account: signer_account_for(
                    store,
                    &folded,
                    &context_id,
                    Some(&deletion.metadata.storage_type),
                ),
                ..ApplyContext::empty()
            };
            match Interface::<MainStorage>::apply_action(action, &ctx) {
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

/// Whether storage would delete `id` without a signature: it checks a delete
/// against the stored entry, and a stored `Public` entry names no author.
fn stored_as_public(id: Id) -> bool {
    match Index::<MainStorage>::get_index(id) {
        Ok(Some(index)) => matches!(index.metadata.storage_type, StorageType::Public),
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Apply a batch of tombstones under the per-context execution lock.
///
/// Same split-brain guard as [`handle_entity_push_locked`]: a tombstone apply
/// is a read-modify-write up to the root and must not interleave with a
/// concurrent delta merge.
pub async fn handle_entity_delete_push_locked(
    context_client: Option<&ContextClient>,
    store: &Store,
    context_id: ContextId,
    runtime_env: &calimero_storage::env::RuntimeEnv,
    deletions: &[EntityDeletion],
    session_peer: Option<PublicKey>,
) -> u32 {
    let _guard = match context_client {
        Some(client) => client.acquire_lock(&context_id).await,
        None => None,
    };
    apply_entity_deletions(store, context_id, runtime_env, deletions, session_peer)
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

    fn leaf_under(schema: Option<[u8; 32]>) -> TreeLeafData {
        let mut metadata = LeafMetadata::new(CrdtType::lww_register(), 100, [0; 32]);
        if let Some(schema) = schema {
            metadata = metadata.with_schema_bytecode_id(schema);
        }
        TreeLeafData::new([1; 32], b"value".to_vec(), metadata)
    }

    #[test]
    fn a_leaf_naming_no_schema_is_read_by_any_reader() {
        assert_eq!(unreadable_schema(&leaf_under(None), [7; 32]), None);
    }

    #[test]
    fn a_leaf_under_the_loaded_schema_is_read() {
        assert_eq!(unreadable_schema(&leaf_under(Some([7; 32])), [7; 32]), None);
    }

    #[test]
    fn a_leaf_under_another_schema_is_not_read() {
        assert_eq!(
            unreadable_schema(&leaf_under(Some([8; 32])), [7; 32]),
            Some([8; 32])
        );
    }

    fn leaf(len: usize) -> TreeLeafData {
        let metadata = LeafMetadata::new(CrdtType::lww_register(), 1, [0; 32]);
        TreeLeafData::new([1; 32], vec![0; len], metadata)
    }

    /// Push batches stop at the entity cap for small leaves and at the byte
    /// budget for large ones, and a single leaf past the budget still goes out.
    #[test]
    fn push_batches_respect_count_and_bytes() {
        let small: Vec<_> = (0..1_200).map(|_| leaf(8)).collect();
        let sizes: Vec<usize> = push_batches(&small).map(<[_]>::len).collect();
        assert_eq!(sizes, [500, 500, 200]);

        let large: Vec<_> = (0..3).map(|_| leaf(3 * 1024 * 1024)).collect();
        let sizes: Vec<usize> = push_batches(&large).map(<[_]>::len).collect();
        assert_eq!(sizes, [1, 1, 1]);

        let huge = [leaf(MAX_RESPONSE_BYTES + 1)];
        assert_eq!(push_batches(&huge).count(), 1);
    }

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

    #[test]
    fn extract_author_user_returns_the_signer_not_the_owner() {
        // `owner` names the ACCOUNT allowed to change the entry; the author this
        // gate needs is the KEY that wrote it, because membership is checked per
        // key. The two are deliberately unrelated here — returning the owner
        // would hand a 32-byte account id to a membership lookup that expects a
        // signing key, and it would silently answer "not a member".
        let signer = PublicKey::from([7u8; 32]);
        let st = StorageType::User {
            rules: calimero_storage::entities::EntryRules::OWNED,
            owner: calimero_account::AccountId::from([0x7A; 32]),
            signature_data: Some(SignatureData {
                signer: Some(signer),
                signature: [0u8; 64],
                nonce: 0,
                on_behalf: None,
            }),
        };
        assert_eq!(
            extract_author_from_leaf_authorization(Some(&st)),
            Some(signer),
        );
    }

    #[test]
    fn extract_author_user_without_a_signer_returns_none() {
        // Same deferral as the `Shared` arm below: an authorization naming no
        // signer yields no author, and `apply_action`'s signature check is what
        // refuses it.
        let st = StorageType::User {
            rules: calimero_storage::entities::EntryRules::OWNED,
            owner: calimero_account::AccountId::from([0x7A; 32]),
            signature_data: None,
        };
        assert_eq!(extract_author_from_leaf_authorization(Some(&st)), None);
    }

    #[test]
    fn extract_author_shared_with_signer_hint_returns_signer() {
        let signer = PublicKey::from([9u8; 32]);
        // The writer set names an ACCOUNT; the signer hint names the KEY. This test
        // is about extracting the hint, so the two are deliberately unrelated.
        let st = StorageType::Shared {
            writers: std::collections::BTreeMap::from([(
                calimero_account::AccountId::from([0x9A; 32]),
                calimero_storage::entities::OpMask::FULL,
            )]),
            signature_data: Some(SignatureData {
                signer: Some(signer),
                signature: [0u8; 64],
                nonce: 0,
                on_behalf: None,
            }),
        };
        assert_eq!(
            extract_author_from_leaf_authorization(Some(&st)),
            Some(signer),
        );
    }

    #[test]
    fn extract_author_shared_without_signer_hint_returns_none() {
        // An authorization that names no signer yields no author — caller
        // treats `None` as "defer to per-action signature verification inside
        // apply_action", which is where it is refused.
        let st = StorageType::Shared {
            writers: std::collections::BTreeMap::from([(
                calimero_account::AccountId::from([1u8; 32]),
                calimero_storage::entities::OpMask::FULL,
            )]),
            signature_data: Some(SignatureData {
                signer: None,
                signature: [0u8; 64],
                nonce: 0,
                on_behalf: None,
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
        let mut md = LeafMetadata::new(CrdtType::lww_register(), 100, [0u8; 32]);
        if let Some(k) = schema {
            md = md.with_schema_bytecode_id(k);
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
            &store,
            context_id,
            identity,
            calimero_account::AccountId::from([0xAC; 32]),
        );

        let leaf_key = [0x42u8; 32];
        let leaf = opaque_leaf_with_schema(leaf_key, Some([2u8; 32])); // v2
        let loaded_v1 = [1u8; 32];

        let outcome = calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
            apply_leaf_with_crdt_merge_gated(
                &store,
                &calimero_governance_store::NotFolded,
                context_id,
                &leaf,
                loaded_v1,
            )
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
        let pending = calimero_governance_store::AbsorbRepository::new(&store)
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
            &store,
            context_id,
            identity,
            calimero_account::AccountId::from([0xAC; 32]),
        );

        let leaf_key = [0x43u8; 32];
        let loaded = [1u8; 32];
        let leaf = opaque_leaf_with_schema(leaf_key, Some(loaded)); // same schema

        let outcome = calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
            apply_leaf_with_crdt_merge_gated(
                &store,
                &calimero_governance_store::NotFolded,
                context_id,
                &leaf,
                loaded,
            )
        })
        .expect("gated apply must not error");

        assert!(matches!(outcome, LeafOutcome::Applied));
    }

    #[test]
    fn legacy_leaf_without_schema_marker_applies() {
        // Back-compat: an older peer's leaf carries `schema_bytecode_id = None`.
        // Treat as "no newer schema" → Apply (never buffer).
        let context_id = ContextId::from([0xCC; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = calimero_node_primitives::sync::storage_bridge::create_runtime_env(
            &store,
            context_id,
            identity,
            calimero_account::AccountId::from([0xAC; 32]),
        );

        let leaf_key = [0x44u8; 32];
        let leaf = opaque_leaf_with_schema(leaf_key, None); // legacy: no marker

        let outcome = calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
            apply_leaf_with_crdt_merge_gated(
                &store,
                &calimero_governance_store::NotFolded,
                context_id,
                &leaf,
                [1u8; 32],
            )
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
            &store,
            context_id,
            identity,
            calimero_account::AccountId::from([0xAC; 32]),
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

    /// A leaf's label is not evidence of who wrote it, so a rotation-log label
    /// does not waive its authorship check.
    #[test]
    fn a_leaf_labelled_rotation_log_still_needs_its_author() {
        use calimero_context_config::types::ContextGroupId;
        use calimero_governance_store::test_fixtures::{enrolled, test_store};

        let store = test_store();
        let group = ContextGroupId::from([0x7A; 32]);
        let context_id = ContextId::from([0x7B; 32]);
        calimero_governance_store::register_context_in_group(&store, &group, &context_id)
            .expect("register");
        let (mallory, _) = enrolled(&store, &group, 0xEE);
        let mut metadata = Metadata::new(1, 1);
        metadata.storage_type = StorageType::User {
            rules: calimero_storage::entities::EntryRules::OWNED,
            owner: calimero_account::AccountId::from([0xA1; 32]),
            signature_data: Some(SignatureData {
                signer: Some(mallory),
                signature: [0u8; 64],
                nonce: 0,
                on_behalf: None,
            }),
        };
        let verdict = |metadata: &Metadata| {
            snapshot_leaf_authorship(
                &store,
                &calimero_governance_store::NotFolded,
                &context_id,
                metadata,
                None,
            )
        };
        assert_eq!(verdict(&metadata), SnapshotAuthorship::Forged, "control");
        metadata.crdt_type = Some(CrdtType::RotationLog);
        assert_eq!(verdict(&metadata), SnapshotAuthorship::Forged);
    }

    #[test]
    fn no_gate_ok_none_still_applies_leaf() {
        // Distinct from the `Err` case: `Ok(None)` is the legitimate
        // "no group / unresolvable meta" case and MUST still apply as today.
        let context_id = ContextId::from([0xCE; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let runtime_env = calimero_node_primitives::sync::storage_bridge::create_runtime_env(
            &store,
            context_id,
            identity,
            calimero_account::AccountId::from([0xAC; 32]),
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

#[cfg(test)]
mod classify_leaf_tests {
    use calimero_primitives::crdt::{CrdtType, CustomTypeId};
    use calimero_storage::address::Id;
    use calimero_storage::collections::ROOT_ENTRY_ID;

    use super::{classify_leaf, LeafDisposition};

    /// The reachability check. A stamped entry must be recognised from the
    /// metadata that actually crosses the wire — the sender carries the
    /// entity's stored `crdt_type` verbatim, so this is the tag that arrives.
    /// If this returned `Apply`, the whole dispatch below it would be
    /// unreachable and the app's rule would silently never run.
    #[test]
    fn a_custom_entry_defers_and_carries_its_id() {
        let id = CustomTypeId::of("team::Stats");
        assert_eq!(
            classify_leaf(Id::random(), &CrdtType::Custom(id), || true),
            LeafDisposition::DeferCustom(id),
            "the id must survive classification — the dispatcher has no other \
             way to know which rule to run"
        );
    }

    /// A custom entry this node holds nothing for applies, because a merge of
    /// one side is not a merge and the deferred pass skips it. Deferring it
    /// anyway is calimero-network/core#4310: a receiver that refused the
    /// entry's delta (the author's binding had not folded yet) is offered it on
    /// every sync and drops it every time, and the replicas never converge.
    #[test]
    fn a_custom_entry_with_nothing_stored_applies() {
        assert_eq!(
            classify_leaf(
                Id::random(),
                &CrdtType::Custom(CustomTypeId::of("app::Read")),
                || false
            ),
            LeafDisposition::Apply,
            "with nothing to merge, the plain apply is the only path that stores it"
        );
    }

    /// The stored-value probe reads storage, so it runs only for a custom entry.
    #[test]
    fn only_a_custom_entry_asks_what_is_stored() {
        for (entity_id, crdt_type) in [
            (Id::random(), CrdtType::UnorderedMap),
            (Id::random(), CrdtType::lww_register()),
            (Id::root(), CrdtType::lww_register()),
        ] {
            let _ = classify_leaf(entity_id, &crdt_type, || {
                panic!("{crdt_type:?} must not probe storage")
            });
        }
    }

    /// Built-ins merge in the storage layer and must NOT be deferred; deferring
    /// them would park work the receiver could have finished immediately.
    #[test]
    fn builtin_entries_apply_directly() {
        for crdt_type in [
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::UnorderedMap,
            CrdtType::Vector,
            CrdtType::lww_register(),
        ] {
            assert_eq!(
                classify_leaf(Id::random(), &crdt_type, || true),
                LeafDisposition::Apply,
                "{crdt_type:?} merges in the storage layer"
            );
        }
    }

    /// The wire type is the peer's claim, so the app-state entry goes to the root
    /// dispatcher whatever it names, and the root collection never does.
    #[test]
    fn the_app_state_entry_defers_whatever_type_the_peer_names() {
        for crdt_type in [
            CrdtType::lww_register(),
            CrdtType::opaque_leaf(),
            CrdtType::js_root(),
            CrdtType::Custom(CustomTypeId::of("x")),
        ] {
            assert_eq!(
                classify_leaf(ROOT_ENTRY_ID, &crdt_type, || true),
                LeafDisposition::DeferRoot,
                "{crdt_type:?}"
            );
            assert_eq!(
                classify_leaf(Id::root(), &crdt_type, || true),
                LeafDisposition::Apply,
                "{crdt_type:?}"
            );
        }
    }
}

#[cfg(test)]
mod tee_only_snapshot_tests {
    use std::collections::BTreeMap;

    use calimero_account::AccountId;

    use super::tee_only_leaf_admitted;

    fn writers(accounts: &[AccountId]) -> BTreeMap<AccountId, ()> {
        accounts.iter().map(|a| (*a, ())).collect()
    }

    #[test]
    fn a_tee_only_leaf_needs_a_signer_resolved_to_the_tee_authority() {
        let tee_only = writers(&[AccountId::TEE_AUTHORITY]);
        let member = AccountId::from([0x4D; 32]);

        assert!(tee_only_leaf_admitted(&tee_only, || Some(
            AccountId::TEE_AUTHORITY
        )));
        assert!(
            !tee_only_leaf_admitted(&tee_only, || Some(member)),
            "a member's leaf claiming the TEE-only writer set must be dropped"
        );
        assert!(
            !tee_only_leaf_admitted(&tee_only, || None),
            "an unresolvable signer must be dropped, not trusted"
        );
    }

    #[test]
    fn other_leaves_are_not_resolved_at_all() {
        let shared = writers(&[AccountId::from([0x01; 32])]);
        assert!(tee_only_leaf_admitted(&shared, || {
            panic!("a leaf outside the TEE-only rule must not be resolved")
        }));
    }
}

#[cfg(test)]
mod snapshot_authorship_tests {
    use std::collections::BTreeMap;

    use calimero_account::AccountId;
    use calimero_primitives::identity::PublicKey;
    use calimero_storage::address::Id;
    use calimero_storage::entities::{SignatureData, StorageType};

    use super::{authorship_verdict, SnapshotAuthorship};

    const SIGNER: [u8; 32] = [0x51; 32];
    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    fn signed() -> Option<SignatureData> {
        Some(SignatureData {
            signature: [0x77; 64],
            nonce: 1,
            signer: Some(PublicKey::from(SIGNER)),
            on_behalf: None,
        })
    }

    fn writers(accounts: &[[u8; 32]]) -> BTreeMap<AccountId, ()> {
        accounts.iter().map(|a| (AccountId::from(*a), ())).collect()
    }

    fn verdict(
        storage_type: &StorageType,
        anchor: Option<&BTreeMap<AccountId, ()>>,
        signer_is: Option<[u8; 32]>,
    ) -> SnapshotAuthorship {
        authorship_verdict(
            storage_type,
            anchor,
            |key| {
                assert_eq!(*key, PublicKey::from(SIGNER));
                signer_is.map(AccountId::from)
            },
            |_, account| account,
            |_, _| panic!("a direct write is not asked the on-behalf rule"),
        )
    }

    #[test]
    fn a_user_entry_must_be_signed_by_its_owner() {
        let entry = StorageType::User {
            rules: calimero_storage::entities::EntryRules::OWNED,
            owner: AccountId::from(ALICE),
            signature_data: signed(),
        };
        assert_eq!(
            verdict(&entry, None, Some(ALICE)),
            SnapshotAuthorship::Authored
        );
        assert_eq!(
            verdict(&entry, None, Some(BOB)),
            SnapshotAuthorship::Forged,
            "another account's key must not be able to write alice's entry"
        );
    }

    #[test]
    fn a_shared_entry_must_be_signed_by_one_of_its_writers() {
        let entry = StorageType::Shared {
            writers: [(AccountId::from(ALICE), Default::default())]
                .into_iter()
                .collect(),
            signature_data: signed(),
        };
        assert_eq!(
            verdict(&entry, None, Some(ALICE)),
            SnapshotAuthorship::Authored
        );
        assert_eq!(verdict(&entry, None, Some(BOB)), SnapshotAuthorship::Forged);
    }

    #[test]
    fn a_member_is_checked_against_its_anchors_writers() {
        let member = StorageType::SharedMember {
            anchor: Id::new([0x0A; 32]),
            signature_data: signed(),
        };
        let anchor = writers(&[ALICE]);
        assert_eq!(
            verdict(&member, Some(&anchor), Some(ALICE)),
            SnapshotAuthorship::Authored
        );
        assert_eq!(
            verdict(&member, Some(&anchor), Some(BOB)),
            SnapshotAuthorship::Forged
        );
        assert_eq!(
            verdict(&member, None, Some(ALICE)),
            SnapshotAuthorship::Forged,
            "a member with no writer set to check against is not authored"
        );
    }

    #[test]
    fn an_uncertified_signer_is_unknown_not_forged() {
        let entry = StorageType::User {
            rules: calimero_storage::entities::EntryRules::OWNED,
            owner: AccountId::from(ALICE),
            signature_data: signed(),
        };
        assert_eq!(verdict(&entry, None, None), SnapshotAuthorship::Unknown);
    }

    #[test]
    fn the_tee_authority_mapping_names_the_writer() {
        let entry = StorageType::Shared {
            writers: [(AccountId::TEE_AUTHORITY, Default::default())]
                .into_iter()
                .collect(),
            signature_data: signed(),
        };
        let verdict = authorship_verdict::<()>(
            &entry,
            None,
            |_| Some(AccountId::from(ALICE)),
            |_, _| AccountId::TEE_AUTHORITY,
            |_, _| panic!("a direct write is not asked the on-behalf rule"),
        );
        assert_eq!(verdict, SnapshotAuthorship::Authored);
    }

    #[test]
    fn unsigned_leaves_are_not_resolved() {
        let verdict = authorship_verdict::<()>(
            &StorageType::Public,
            None,
            |_| panic!("a leaf with no signer must not be resolved"),
            |_, _| panic!("a leaf with no signer must not be resolved"),
            |_, _| panic!("a leaf with no signer must not be resolved"),
        );
        assert_eq!(verdict, SnapshotAuthorship::Authored);
    }

    const RELAY: [u8; 32] = [0x7E; 32];

    /// Signed by the relay's key, written for `on_behalf`.
    fn signed_for(on_behalf: [u8; 32]) -> Option<SignatureData> {
        signed().map(|sd| SignatureData {
            on_behalf: Some(AccountId::from(on_behalf)),
            ..sd
        })
    }

    /// `may` is the on-behalf rule's answer for the relay writing for the
    /// account; the signer always resolves to the relay's account.
    fn verdict_for(
        storage_type: &StorageType,
        anchor: Option<&BTreeMap<AccountId, ()>>,
        may: Option<bool>,
    ) -> SnapshotAuthorship {
        authorship_verdict(
            storage_type,
            anchor,
            |_| Some(AccountId::from(RELAY)),
            |_, _| AccountId::TEE_AUTHORITY,
            |relay, _| {
                assert_eq!(
                    relay,
                    AccountId::from(RELAY),
                    "asked of the signer's account"
                );
                may
            },
        )
    }

    /// A relay's leaf for alice is alice's: judged against her ownership, not
    /// the relay's, and only when the relay may write for her.
    #[test]
    fn an_on_behalf_user_entry_is_its_accounts_when_the_relay_may_write_for_it() {
        let entry = |owner| StorageType::User {
            rules: calimero_storage::entities::EntryRules::OWNED,
            owner: AccountId::from(owner),
            signature_data: signed_for(ALICE),
        };
        assert_eq!(
            verdict_for(&entry(ALICE), None, Some(true)),
            SnapshotAuthorship::Authored
        );
        assert_eq!(
            verdict_for(&entry(ALICE), None, Some(false)),
            SnapshotAuthorship::Forged,
            "a signer that may not write for alice forged her entry"
        );
        assert_eq!(
            verdict_for(&entry(BOB), None, Some(true)),
            SnapshotAuthorship::Forged,
            "an entry written for alice cannot be bob's"
        );
        assert_eq!(
            verdict_for(&entry(ALICE), None, None),
            SnapshotAuthorship::Unknown,
            "an unreadable rule fails the snapshot rather than dropping the leaf"
        );
    }

    /// Writer sets are asked about the account written for, never the relay's
    /// own account, and never the TEE authority the relay's key may map to.
    #[test]
    fn an_on_behalf_shared_entry_is_checked_against_the_account_written_for() {
        let entry = |writers: &[[u8; 32]]| StorageType::Shared {
            writers: writers
                .iter()
                .map(|a| (AccountId::from(*a), Default::default()))
                .collect(),
            signature_data: signed_for(ALICE),
        };
        assert_eq!(
            verdict_for(&entry(&[ALICE]), None, Some(true)),
            SnapshotAuthorship::Authored
        );
        assert_eq!(
            verdict_for(&entry(&[RELAY]), None, Some(true)),
            SnapshotAuthorship::Forged,
            "the relay's own place in the set does not let it write for alice"
        );
        let tee_only = StorageType::Shared {
            writers: [(AccountId::TEE_AUTHORITY, Default::default())]
                .into_iter()
                .collect(),
            signature_data: signed_for(ALICE),
        };
        assert_eq!(
            verdict_for(&tee_only, None, Some(true)),
            SnapshotAuthorship::Forged
        );

        let member = StorageType::SharedMember {
            anchor: Id::new([0x0C; 32]),
            signature_data: signed_for(ALICE),
        };
        assert_eq!(
            verdict_for(&member, Some(&writers(&[ALICE])), Some(true)),
            SnapshotAuthorship::Authored
        );
        assert_eq!(
            verdict_for(&member, Some(&writers(&[RELAY])), Some(true)),
            SnapshotAuthorship::Forged
        );
    }
}

#[cfg(test)]
mod rotation_rescue_tests {
    use std::collections::BTreeMap;

    use calimero_account::AccountId;
    use calimero_primitives::identity::PublicKey;
    use calimero_storage::entities::OpMask;
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use calimero_storage::rotation_log::RotationLogEntry;
    use core::num::NonZeroU64;

    use super::latest_rotation_removed;

    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    fn key(seed: [u8; 32]) -> PublicKey {
        PublicKey::from(seed)
    }

    fn set(accounts: &[[u8; 32]]) -> BTreeMap<AccountId, OpMask> {
        accounts
            .iter()
            .map(|a| (AccountId::from(*a), OpMask::FULL))
            .collect()
    }

    fn rotation(at: u64, by: [u8; 32], to: &[[u8; 32]]) -> RotationLogEntry {
        RotationLogEntry {
            delta_id: [at as u8; 32],
            delta_hlc: HybridTimestamp::new(Timestamp::new(
                NTP64(at),
                ID::from(NonZeroU64::new(1).unwrap()),
            )),
            signer: Some(key(by)),
            signature: Some([0x5A; 64]),
            signed_payload: Some([0x5B; 32]),
            new_writers: set(to),
            writers_nonce: at,
        }
    }

    /// Alice is a writer whenever the set names her account.
    fn verdict(entries: &[RotationLogEntry], leaf_writers: &[[u8; 32]], signer: [u8; 32]) -> bool {
        latest_rotation_removed(
            entries,
            &set(leaf_writers),
            &key(signer),
            |prior| prior.contains_key(&AccountId::from(signer)),
            |_| true,
        )
    }

    #[test]
    fn a_writer_who_removed_themselves_is_rescued() {
        let log = [
            rotation(1, ALICE, &[ALICE, BOB]),
            rotation(2, ALICE, &[BOB]),
        ];
        assert!(verdict(&log, &[BOB], ALICE));
    }

    #[test]
    fn the_leaf_must_carry_the_set_that_rotation_produced() {
        let log = [
            rotation(1, ALICE, &[ALICE, BOB]),
            rotation(2, ALICE, &[BOB]),
        ];
        assert!(!verdict(&log, &[ALICE], BOB));
        assert!(!verdict(&log, &[], ALICE));
    }

    #[test]
    fn only_the_latest_rotation_counts() {
        // Alice's removal was later superseded by Bob's own rotation, so the
        // leaf's last write is Bob's, not the one that removed Alice.
        let log = [
            rotation(1, ALICE, &[ALICE, BOB]),
            rotation(2, ALICE, &[BOB]),
            rotation(3, BOB, &[BOB]),
        ];
        assert!(!verdict(&log, &[BOB], ALICE));
    }

    #[test]
    fn the_signer_must_have_been_a_writer_before_it() {
        let log = [rotation(1, BOB, &[BOB]), rotation(2, ALICE, &[BOB])];
        assert!(!verdict(&log, &[BOB], ALICE));
    }

    #[test]
    fn a_log_with_only_the_creating_rotation_rescues_nothing() {
        assert!(!verdict(&[rotation(1, ALICE, &[BOB])], &[BOB], ALICE));
    }

    fn member_verdict(entries: &[RotationLogEntry], written_at: u64, signer: [u8; 32]) -> bool {
        super::writer_when_written(
            entries,
            written_at,
            |set| set.contains_key(&AccountId::from(signer)),
            |_| true,
        )
    }

    #[test]
    fn a_member_written_before_its_author_was_removed_is_kept() {
        // Bob removed Alice at 5; her member was written at 3.
        let log = [rotation(1, BOB, &[ALICE, BOB]), rotation(5, BOB, &[BOB])];
        assert!(member_verdict(&log, 3, ALICE));
    }

    #[test]
    fn a_member_written_after_its_author_was_removed_is_not() {
        let log = [rotation(1, BOB, &[ALICE, BOB]), rotation(5, BOB, &[BOB])];
        assert!(!member_verdict(&log, 7, ALICE));
    }

    #[test]
    fn a_member_older_than_every_rotation_has_no_set_to_check() {
        let log = [rotation(5, BOB, &[ALICE, BOB])];
        assert!(!member_verdict(&log, 3, ALICE));
    }

    #[test]
    fn a_forged_rotation_cannot_make_its_signer_a_past_writer() {
        // A fabricated entry naming Alice at 2 does not verify, so only the
        // genuine set without her counts.
        let log = [rotation(1, BOB, &[BOB]), rotation(2, ALICE, &[ALICE, BOB])];
        assert!(!super::writer_when_written(
            &log,
            3,
            |set| set.contains_key(&AccountId::from(ALICE)),
            |entry| entry.writers_nonce != 2,
        ));
    }

    #[test]
    fn an_entry_whose_signature_does_not_verify_does_not_count() {
        let log = [
            rotation(1, ALICE, &[ALICE, BOB]),
            rotation(2, ALICE, &[BOB]),
        ];
        let forged_latest = latest_rotation_removed(
            &log,
            &set(&[BOB]),
            &key(ALICE),
            |prior| prior.contains_key(&AccountId::from(ALICE)),
            |entry| entry.writers_nonce != 2,
        );
        assert!(!forged_latest);
    }
}

/// How the node resolves a leaf a relay wrote on an account's behalf, on the
/// repair and snapshot paths, against real governance rows.
#[cfg(test)]
mod on_behalf_resolution_tests {
    use calimero_account::AccountId;
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::MemberCapabilities;
    use calimero_governance_store::test_fixtures::{enrolled, sample_meta_with_admin, test_store};
    use calimero_governance_store::{
        CapabilitiesRepository, MembershipRepository, MetaRepository, NotFolded,
    };
    use calimero_node_primitives::sync::{LeafMetadata, TreeLeafData};
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::crdt::CrdtType;
    use calimero_primitives::identity::PublicKey;
    use calimero_storage::entities::{EntryRules, Metadata, SignatureData, StorageType};
    use calimero_store::Store;

    use super::{
        is_leaf_currently_authorized, signer_account_for, snapshot_leaf_authorship,
        SnapshotAuthorship,
    };

    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    /// A group holding a context, a `RelayTee` relay, alice and bob as members
    /// with no key here, and mallory, a member holding `CAN_AUTHOR_ON_BEHALF`.
    struct World {
        store: Store,
        group: ContextGroupId,
        context: ContextId,
        relay_pk: PublicKey,
        relay: AccountId,
        mallory_pk: PublicKey,
    }

    fn world() -> World {
        let store = test_store();
        let group = ContextGroupId::from([0x7A; 32]);
        let context = ContextId::from([0x7B; 32]);
        MetaRepository::new(&store)
            .save(&group, &sample_meta_with_admin(AccountId::from([0xEE; 32])))
            .expect("save meta");
        calimero_governance_store::register_context_in_group(&store, &group, &context)
            .expect("register");
        let membership = MembershipRepository::new(&store);
        let (relay_pk, relay) = enrolled(&store, &group, 0x7E);
        membership
            .add_member(&group, &relay, GroupMemberRole::RelayTee)
            .expect("seat the relay");
        let (mallory_pk, mallory) = enrolled(&store, &group, 0x3D);
        membership
            .add_member(&group, &mallory, GroupMemberRole::Member)
            .expect("seat mallory");
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &group,
                &mallory,
                MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
            )
            .expect("mallory holds the authorship bit");
        for account in [ALICE, BOB] {
            membership
                .add_member(&group, &AccountId::from(account), GroupMemberRole::Member)
                .expect("seat a member");
        }
        World {
            store,
            group,
            context,
            relay_pk,
            relay,
            mallory_pk,
        }
    }

    /// A `User` entry owned by `owner`, signed by `signer` for `on_behalf`.
    fn owned(owner: [u8; 32], signer: PublicKey, on_behalf: Option<[u8; 32]>) -> StorageType {
        StorageType::User {
            rules: EntryRules::OWNED,
            owner: AccountId::from(owner),
            signature_data: Some(SignatureData {
                signature: [0x77; 64],
                nonce: 1,
                signer: Some(signer),
                on_behalf: on_behalf.map(AccountId::from),
            }),
        }
    }

    fn leaf(storage_type: StorageType) -> TreeLeafData {
        TreeLeafData::new(
            [0x01; 32],
            vec![1],
            LeafMetadata::new(CrdtType::lww_register(), 1, [0; 32])
                .with_authorization(storage_type),
        )
    }

    fn repaired(w: &World, storage_type: StorageType) -> bool {
        is_leaf_currently_authorized(&w.store, &NotFolded, &w.context, &leaf(storage_type), None)
    }

    fn snapshot(w: &World, storage_type: StorageType) -> SnapshotAuthorship {
        let mut metadata = Metadata::new(1, 1);
        metadata.storage_type = storage_type;
        snapshot_leaf_authorship(&w.store, &NotFolded, &w.context, &metadata, None)
    }

    /// A relay's entry for alice resolves to alice, so storage judges it as
    /// hers. Signed by anyone else, or for a stranger, it resolves to nobody.
    #[test]
    fn a_relay_entry_resolves_to_the_account_it_was_written_for() {
        let w = world();
        let resolve =
            |st: StorageType| signer_account_for(&w.store, &NotFolded, &w.context, Some(&st));

        assert_eq!(
            resolve(owned(ALICE, w.relay_pk, Some(ALICE))),
            Some(AccountId::from(ALICE))
        );
        assert_eq!(
            resolve(owned(ALICE, w.mallory_pk, Some(ALICE))),
            None,
            "a member holding the authorship bit is no relay"
        );
        assert_eq!(
            resolve(owned([0x51; 32], w.relay_pk, Some([0x51; 32]))),
            None,
            "a relay writes for members only"
        );
        assert_eq!(
            resolve(owned(ALICE, w.relay_pk, None)),
            Some(w.relay),
            "control: a direct write still resolves to its signer's account"
        );
    }

    /// The repair gate asks the on-behalf rule instead of the signer's own
    /// standing: the relay is read-only, so the signer gate alone drops every
    /// entry it wrote for someone.
    #[test]
    fn repair_admits_a_relay_entry_for_its_owner_and_nothing_else() {
        let w = world();
        assert!(repaired(&w, owned(ALICE, w.relay_pk, Some(ALICE))));
        assert!(
            !repaired(&w, owned(ALICE, w.relay_pk, None)),
            "control: the relay may not write as itself"
        );
        assert!(
            !repaired(&w, owned(BOB, w.relay_pk, Some(ALICE))),
            "an entry written for alice cannot overwrite bob's"
        );
        assert!(!repaired(&w, owned(ALICE, w.mallory_pk, Some(ALICE))));

        MembershipRepository::new(&w.store)
            .set_role(&w.group, &w.relay, GroupMemberRole::ReadOnlyTee)
            .expect("the relay is switched back to a replica");
        assert!(
            !repaired(&w, owned(ALICE, w.relay_pk, Some(ALICE))),
            "repair reads the relay's standing now"
        );
    }

    /// A snapshot keeps what a relay wrote for a member who has since left, as
    /// it keeps any departed author's state, and drops what a non-relay signed.
    #[test]
    fn a_snapshot_keeps_a_relay_entry_and_drops_a_forgery() {
        let w = world();
        assert_eq!(
            snapshot(&w, owned(ALICE, w.relay_pk, Some(ALICE))),
            SnapshotAuthorship::Authored
        );
        assert_eq!(
            snapshot(&w, owned(ALICE, w.mallory_pk, Some(ALICE))),
            SnapshotAuthorship::Forged
        );

        MembershipRepository::new(&w.store)
            .remove_member(&w.group, &AccountId::from(ALICE))
            .expect("alice leaves");
        assert_eq!(
            snapshot(&w, owned(ALICE, w.relay_pk, Some(ALICE))),
            SnapshotAuthorship::Authored,
            "a cold joiner keeps the state written for a member who left"
        );
        assert!(
            !repaired(&w, owned(ALICE, w.relay_pk, Some(ALICE))),
            "while repair, which asks about now, drops it"
        );
    }
}
