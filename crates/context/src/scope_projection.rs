//! Substrate for the unified causal log: convert the live apply stream into
//! [`Op`]s and maintain one [`ScopeState`] projection per scope.
//!
//! This is **additive** — nothing routes a decision or a convergence check
//! through it yet. It is the building block the apply paths feed while the
//! unified op-log is brought up alongside the separate data / governance /
//! rotation stores: maintain one projection per scope, and (later) derive its
//! convergence root by folding the projection's ACL + governance hashes onto
//! the storage layer's existing Merkle entities root, so a hash-neutral
//! writer/membership rotation moves the root.
//!
//! What feeds it, and why: the projection's value is the **ACL + governance**
//! planes (the data plane's entities come from the storage Merkle, so feeding
//! `Put`/`Delete` here would only duplicate state in memory). This module
//! starts with the **governance** plane — the cleartext namespace ops, whose
//! `signer` is a deterministic cross-node author and whose target scope is
//! resolvable from the op — applied at the namespace governance handler.

use std::collections::{HashMap, HashSet};

use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    CapabilitiesRepository, MembershipRepository, MetaRepository, NamespaceDagService,
    NamespaceOpLogService, NamespaceRepository,
};
use calimero_governance_types::{GroupOp, NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_op::{Op, OpPayload, ScopeId};
use calimero_op_adapter::{payload_from_group_op, payload_from_root_op, set_writers_payload};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_projection::ScopeState;
use calimero_storage::address::Id;
use calimero_storage::collections::decode_rotation_log_entry_child;
use calimero_storage::index::EntityIndex;
use calimero_storage::interface::Interface;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_storage::rotation_log::{RotationLog, RotationLogEntry};
use calimero_storage::store::{Key as StorageKey, MainStorage};
use calimero_store::key::ContextState;
use calimero_store::Store;

use crate::governance_dag::signed_namespace_op_to_delta;

/// Hard cap on ops collected in one backfill DAG walk ([`collect_namespace_ops`]).
/// Bounds memory/CPU against a pathologically deep or corrupted persisted
/// governance DAG (the `visited` set already prevents cycles, but not the
/// allocation): hitting the cap leaves the namespace partially backfilled with a
/// warning rather than OOM-ing the node. Set well above any real namespace's
/// governance history — comfortably over the in-memory prune threshold (8192).
const MAX_BACKFILL_OPS: usize = 100_000;

/// Assemble an [`Op`] that **mirrors a source-DAG op**: its `id` and `parents`
/// are the source delta's own id/parents, *not* a fresh [`Op::compute_id`]. This
/// is deliberate — it makes the projection's op graph share an id space with the
/// source DAGs, so a live decision's cut (e.g. a delta's `governance_dag_heads`,
/// which are governance-op ids) maps directly onto the projection and
/// [`ScopeProjections::acl_view_at`] resolves the same ancestry the source DAG
/// would. The source ids are themselves content-addressed + identical on every
/// node, so the projection's `(hlc, op_id)` LWW stays deterministic.
fn build_op(
    id: [u8; 32],
    scope: ScopeId,
    author: PublicKey,
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
    payload: OpPayload,
) -> Op {
    Op {
        id,
        scope,
        parents: parents.to_vec(),
        author,
        hlc,
        payload,
        expected_scope_root: [0u8; 32],
        signature: [0u8; 64],
    }
}

/// Convert a writer-set rotation ([`RotationLogEntry`]) into the unified
/// `SetWriters` [`Op`] for `object` in `scope`, or `None` for an unsigned
/// bootstrap entry (no author to attribute it to — those are skipped exactly as
/// the rotation-log append path skips them).
///
/// The op `id` is the rotation's `delta_id` (mirroring the source), the author
/// is its `signer` (deterministic across nodes), and the hlc is its `delta_hlc`.
/// Parents are left empty: the rotation log is a per-object sequence resolved by
/// `(hlc, signer)` today, and the projection's per-object `(hlc, op_id)` LWW
/// reproduces that ordering without needing the causal edges (the equivalence is
/// covered by `op-adapter::acl_plane_matches_resolve_local_*`).
///
/// This is the ACL-plane **conversion**; feeding it from the live apply stream
/// is a later step — the raw rotation entries are produced in the storage
/// layer, below the projection, so the independent feed needs storage to
/// surface applied rotations rather than re-deriving them from the resolver.
#[must_use]
pub fn op_from_rotation_entry(object: Id, scope: ScopeId, entry: &RotationLogEntry) -> Option<Op> {
    let author = entry.signer?;
    let payload = set_writers_payload(object, entry);
    Some(build_op(
        entry.delta_id,
        scope,
        author,
        entry.delta_hlc,
        &[],
        payload,
    ))
}

/// Read a Shared anchor's rotation log directly from the datastore (no WASM
/// storage env), by walking its hashed-collection children. `None` if the
/// anchor has no rotation collection yet (never rotated / not a Shared anchor).
///
/// This is the post-apply read the live ACL feed uses to recover the **raw**
/// rotation entries (with their signer), the independent source the projection
/// folds — as opposed to the resolver's already-merged output.
///
/// TODO(consolidate): mirrors `load_rotation_log_direct` in
/// `crates/node/src/delta_store.rs`; both should share one Store-backed reader
/// in `calimero-storage` before this becomes authoritative at cutover.
#[must_use]
pub fn load_rotation_log_direct(
    client: &ContextClient,
    context_id: ContextId,
    anchor: Id,
) -> Option<RotationLog> {
    let map_id = Interface::<MainStorage>::rotation_log_child_id(anchor);
    let handle = client.datastore_handle();
    // Read a state value. A store error is surfaced as a warning (distinct from
    // a legitimately-absent key) so a transient I/O fault doesn't silently make
    // the ACL shadow feed skip an anchor's rotations.
    let read = |key: StorageKey| -> Option<Vec<u8>> {
        let state_key = ContextState::new(context_id, key.to_bytes());
        match handle.get(&state_key) {
            Ok(state) => state.map(|s| s.value.into_boxed().into_vec()),
            Err(err) => {
                tracing::warn!(
                    %context_id, anchor = ?anchor, %err,
                    "rotation-log read failed; ACL shadow feed skips this anchor"
                );
                None
            }
        }
    };

    let index_bytes = read(StorageKey::Index(map_id))?;
    let index = match borsh::from_slice::<EntityIndex>(&index_bytes) {
        Ok(index) => index,
        Err(err) => {
            tracing::warn!(
                %context_id, anchor = ?anchor, %err,
                "rotation-log index failed to decode; ACL shadow feed skips this anchor"
            );
            return None;
        }
    };
    let mut entries = Vec::new();
    if let Some(children) = index.children() {
        for child in children {
            let Some(bytes) = read(StorageKey::Entry(child.id())) else {
                // The child is listed in the index but its value is unreadable
                // (store error — already warned in `read` — or an absent value,
                // a write-skew). Either way the log is partial; flag it.
                tracing::warn!(
                    %context_id, anchor = ?anchor, child = ?child.id(),
                    "rotation-log child listed but unreadable; ACL shadow feed may be incomplete"
                );
                continue;
            };
            match decode_rotation_log_entry_child(&bytes) {
                Some(entry) => entries.push(entry),
                // A child that doesn't decode yields a partial log; warn rather
                // than skip in silence (matches the node-crate reader).
                None => tracing::warn!(
                    %context_id, anchor = ?anchor, child = ?child.id(),
                    "rotation-log child failed to decode; ACL shadow feed may be incomplete"
                ),
            }
        }
    }
    // Canonical order; the caller filters to this delta's id, so ordering is not
    // load-bearing here (it mirrors the node-crate reader's shape).
    entries.sort_by(|a, b| a.delta_id.cmp(&b.delta_id));
    Some(RotationLog {
        snapshot: None,
        entries,
    })
}

/// Convert a namespace governance op into the unified [`Op`] graph node it
/// occupies — **always** a node, never `None`: membership ops carry their
/// payload, and every other op (non-membership Root op, encrypted/undecryptable
/// Group op, key transport) folds to [`OpPayload::Noop`]. The node MUST still
/// exist so an ancestry walk can traverse *through* it; dropping it would
/// truncate the walk and orphan every membership op behind it.
///
/// Governance ops are keyed under the **namespace** scope, not per-group. The
/// live system keeps ONE governance DAG per namespace and a data write cites
/// namespace-wide `governance_dag_heads`, so membership has to resolve over the
/// whole namespace ancestry (a per-group log truncates the walk at the first
/// cross-scope node — that was the bug). Membership for a specific group is read
/// out of the folded view's `groups[group]`; the per-scope-DAG split is a
/// post-cutover concern.
///
/// `id`/`hlc`/`parents` are the governance **delta's own** id, hlc, and parents
/// (its `parent_op_hashes`) so the projection mirrors the governance DAG and the
/// cut maps onto it (see [`build_op`]). `decrypted_group_op` is the cleartext
/// `GroupOp` for a `NamespaceOp::Group` (via
/// `calimero_governance_store::decrypt_group_op`), or `None` when it couldn't be
/// decrypted — in which case the node is still recorded as `Noop`.
#[must_use]
pub fn op_from_namespace_op(
    signed: &SignedNamespaceOp,
    decrypted_group_op: Option<&GroupOp>,
    id: [u8; 32],
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
) -> Op {
    let payload = match &signed.op {
        // `MemberJoinedOpen` is an open-subgroup inheritance-join PROOF, not a
        // direct membership: live's apply requires `check_path == Inherited` and
        // writes NO persistent `GroupMember` row, re-deriving the membership from
        // the anchor each time (so it is revoked when the anchor's membership is
        // removed, and restored on rejoin). Folding it as a direct `MemberAdded`
        // would make it permanent and survive anchor removal (the over-grant). Fold
        // it as a `Noop` graph node; the inheritance walk in
        // `AclView::is_member_at_cut` derives the membership from the foldable
        // anchor membership + visibility + cap (default cap via base fact), so it
        // tracks the anchor both ways.
        NamespaceOp::Root(RootOp::MemberJoinedOpen { .. }) => OpPayload::Noop,
        NamespaceOp::Root(root) => {
            payload_from_root_op(root, signed.signer).unwrap_or(OpPayload::Noop)
        }
        NamespaceOp::Group { group_id, .. } => decrypted_group_op
            .and_then(|g| payload_from_group_op(ContextGroupId::from(*group_id), g))
            .unwrap_or(OpPayload::Noop),
    };
    build_op(
        id,
        ScopeId::from(signed.namespace_id),
        signed.signer,
        hlc,
        parents,
        payload,
    )
}

/// In-memory registry of unified-op [`ScopeState`] projections, keyed by
/// [`ScopeId`].
///
/// Keyed by **scope**. For the ACL plane a scope is a single object's
/// writer-set sequence. For the **governance** plane the scope is the
/// **namespace** (`ScopeId::from(namespace_id)`): the live system keeps one
/// governance DAG per namespace and a data write cites namespace-wide
/// `governance_dag_heads`, so all of a namespace's governance ops fold into one
/// log and membership for any group within it is read from the folded view's
/// `groups[group]`. (Keying governance per-group instead would truncate the
/// causal-cut walk at the first cross-scope node; the per-scope-DAG split is a
/// post-cutover concern.)
///
/// Unbounded for now: only governance/ACL ops feed it today, so growth tracks
/// the (small) number of live scopes and their op history; eviction (gated like
/// the other per-context caches) and persistence come with the broader wiring.
#[derive(Debug, Default)]
pub struct ScopeProjections {
    /// Folded current state per scope — the fast path for current-view reads.
    states: HashMap<ScopeId, ScopeState>,
    /// Retained op-log per scope, enabling causal-cut resolution
    /// ([`Self::acl_view_at`]) — the **causal-honor** view the cutover's
    /// `authorize` decides against (the state as of an op's own parents, never
    /// the receiver's current state). Grows with governance/ACL ops; bounding
    /// lands with the cutover.
    logs: HashMap<ScopeId, Vec<Op>>,
    /// Op ids already retained per scope — gives `ingest_op` O(1) dedup of a
    /// replayed delta instead of an O(n) scan of the log.
    seen: HashMap<ScopeId, HashSet<[u8; 32]>>,
    /// Namespaces already replayed from persisted state, so `backfill_namespace`
    /// walks each governance DAG at most once (the live feed maintains it after).
    backfilled: HashSet<[u8; 32]>,
}

impl ScopeProjections {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a single op into its own scope's projection and op-log.
    ///
    /// A duplicate op (same `id`, e.g. a delta replayed on restart) is skipped
    /// from the retained log: the fold is already idempotent (LWW by
    /// `(hlc, op_id)`) and `acl_view_at` dedups by id, but skipping keeps the
    /// log from accreting duplicates.
    pub fn ingest_op(&mut self, op: &Op) {
        self.states.entry(op.scope).or_default().apply(op);
        // O(1) dedup: `insert` is true only for a not-yet-seen id.
        if self.seen.entry(op.scope).or_default().insert(op.id) {
            self.logs.entry(op.scope).or_default().push(op.clone());
        }
    }

    /// The **causal-honor** authorization view of `scope` at the cut named by
    /// `parents`: fold only the ops in the ancestry of `parents` (never ops
    /// after the cut), over the retained op-log. `None` if `scope` hasn't been
    /// fed. This is the view the cutover's `authorize` decides against — the
    /// projection's answer to "was this author permitted *as of its own causal
    /// position*", independent of what the receiver has since applied.
    ///
    /// TODO(cutover): this folds the full retained log (O(n) per call) and the
    /// log is unbounded until eviction lands. Replace with an indexed ancestry
    /// lookup + bound/persist the log before making this authoritative.
    #[must_use]
    pub fn acl_view_at(
        &self,
        scope: &ScopeId,
        parents: &[[u8; 32]],
    ) -> Option<calimero_authz::AclView> {
        let log = self.logs.get(scope)?;
        Some(ScopeState::acl_view_at(log, parents))
    }

    /// Is the full causal ancestry of `parents` folded in `scope`'s log (no
    /// truncation)? `false` if the scope is unfed or any ancestor is missing.
    /// The authoritative-grant gate (see [`member_at_cut_authoritative`]).
    ///
    /// [`member_at_cut_authoritative`]: Self::member_at_cut_authoritative
    #[must_use]
    pub fn cut_ancestry_complete(&self, scope: &ScopeId, parents: &[[u8; 32]]) -> bool {
        self.logs
            .get(scope)
            .is_some_and(|log| ScopeState::cut_ancestry_complete(log, parents))
    }

    /// Rebuild this namespace's governance scopes from **persisted** source
    /// state — the startup/backfill path, so a just-restarted node's projection
    /// isn't empty (an empty projection can't be authoritative). The projection
    /// is a *derived* view, so we replay the authoritative persisted governance
    /// op-log rather than persisting a parallel copy (which could diverge).
    ///
    /// Walks the namespace governance DAG from its heads via `parent_op_hashes`,
    /// re-deriving each op's delta coordinates (`signed_namespace_op_to_delta`)
    /// so the ingested ops carry the same ids/parents as the live feed (see
    /// [`op_from_signed_namespace_op`]). Idempotent — `ingest_op` dedups by id,
    /// so a backfill after some live feed (or a repeated backfill) is a no-op
    /// for already-seen ops. ACL (rotation) scopes are backfilled separately via
    /// [`op_from_rotation_entry`] + [`load_rotation_log_direct`] at the call site
    /// (the anchors are known there).
    pub fn backfill_namespace(&mut self, store: &Store, namespace_id: [u8; 32]) {
        if self.backfilled.contains(&namespace_id) {
            return;
        }
        if let Some(ops) = Self::collect_namespace_ops(store, namespace_id) {
            self.apply_backfill(namespace_id, ops);
        }
        // A `None` (governance head unreadable) leaves the namespace UN-backfilled
        // so a later call retries; see `collect_namespace_ops`.
    }

    /// Has this namespace's governance history already been replayed into the
    /// projection? The hot-path gate so the (lock-free) [`collect_namespace_ops`]
    /// walk runs at most once per namespace.
    ///
    /// [`collect_namespace_ops`]: Self::collect_namespace_ops
    #[must_use]
    pub fn is_namespace_backfilled(&self, namespace_id: [u8; 32]) -> bool {
        self.backfilled.contains(&namespace_id)
    }

    /// Resolve `group`'s namespace and report it **iff** the projection needs a
    /// (re)walk before resolving the cut at `heads`: either it was never
    /// backfilled, OR the cut cites a head this node hasn't folded yet. The
    /// latter is the self-heal for the **originator** case — a node that emits a
    /// governance op applies it locally (bypassing the namespace apply feed) and
    /// then cites the new head; backfill ran once already, so the head is missing
    /// from the log until we re-walk. Cheap (point lookup + set/contains checks,
    /// no DAG walk); the expensive walk ([`collect_namespace_ops`]) runs WITHOUT
    /// the projection lock held. `None` ⇒ unresolvable, or the cut is already
    /// fully folded (steady state — no re-walk).
    ///
    /// [`collect_namespace_ops`]: Self::collect_namespace_ops
    #[must_use]
    pub fn namespace_to_refresh(
        &self,
        store: &Store,
        group: ContextGroupId,
        heads: &[[u8; 32]],
    ) -> Option<[u8; 32]> {
        let namespace_id = NamespaceRepository::new(store)
            .resolve(&group)
            .ok()?
            .to_bytes();
        if !self.backfilled.contains(&namespace_id) {
            return Some(namespace_id);
        }
        // Already walked once — only re-walk if the cut references a head we
        // haven't folded (e.g. an op this node just authored locally).
        let scope = ScopeId::from(namespace_id);
        let folded = self.seen.get(&scope);
        let cut_incomplete = heads
            .iter()
            .any(|h| folded.is_none_or(|ids| !ids.contains(h)));
        cut_incomplete.then_some(namespace_id)
    }

    /// Walk a namespace's **persisted** governance DAG from its heads and return
    /// the [`Op`]s to ingest — the backfill's expensive half, deliberately an
    /// associated fn taking no `&self` so it can run **outside** the projection
    /// lock (the apply path shares that lock; holding it across a RocksDB DAG
    /// walk would stall the actor's ingest). Pair with [`apply_backfill`].
    ///
    /// Replays the authoritative persisted op-log rather than persisting a
    /// parallel copy (which could diverge), re-deriving each op's delta
    /// coordinates (`signed_namespace_op_to_delta`) so the ingested ops carry the
    /// same ids/parents as the live feed (see [`op_from_namespace_op`]). EVERY
    /// op becomes a node (membership ops with their payload, the rest as `Noop`)
    /// so the ancestry stays unbroken; encrypted `NamespaceOp::Group` ops are
    /// decrypted best-effort (this node holds the key for groups it belongs to),
    /// folding membership when decryptable and a `Noop` node otherwise.
    ///
    /// `None` when the governance head itself is unreadable — the signal to leave
    /// the namespace un-backfilled so a transient store fault retries on the next
    /// call rather than permanently marking it done. A missing *parent* op is a
    /// normal partial frontier (collect what's present), not a `None`.
    ///
    /// [`apply_backfill`]: Self::apply_backfill
    #[must_use]
    pub fn collect_namespace_ops(store: &Store, namespace_id: [u8; 32]) -> Option<Vec<Op>> {
        let dag = NamespaceDagService::new(store, namespace_id);
        let heads = match dag.read_head_record() {
            Ok(head) => head.parent_hashes,
            Err(err) => {
                tracing::warn!(namespace = ?namespace_id, %err, "projection backfill: governance head unreadable");
                return None;
            }
        };

        let op_log = NamespaceOpLogService::new(store, namespace_id);
        let mut visited: HashSet<[u8; 32]> = HashSet::new();
        let mut queue: std::collections::VecDeque<[u8; 32]> = heads.into_iter().collect();
        let mut ops = Vec::new();
        while let Some(id) = queue.pop_front() {
            if !visited.insert(id) {
                continue;
            }
            // Bound the walk: a corrupted/adversarial persisted DAG (or one far
            // deeper than any real history) must not OOM the node. Stop with a
            // warning and return what we have — a partial backfill is safe (the
            // live feed keeps maintaining the projection; the authoritative-grant
            // path independently checks `cut_ancestry_complete`).
            if visited.len() > MAX_BACKFILL_OPS {
                tracing::warn!(
                    namespace = ?namespace_id,
                    cap = MAX_BACKFILL_OPS,
                    "projection backfill: op cap hit; returning partial walk"
                );
                break;
            }
            let signed = match op_log.get_signed_op(id) {
                Ok(Some(signed)) => signed,
                // A referenced parent not present locally is a normal partial
                // frontier (backfill what we have); only a store error is noise.
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(namespace = ?namespace_id, op = ?id, %err, "projection backfill: op read failed");
                    continue;
                }
            };
            for parent in &signed.parent_op_hashes {
                queue.push_back(*parent);
            }
            let Ok(delta) = signed_namespace_op_to_delta(&signed) else {
                continue;
            };
            // Decrypt an encrypted group op so its membership change folds; a
            // failure (no key for this group) leaves it a `Noop` node — still
            // recorded so the walk can pass through it.
            let decrypted = match &signed.op {
                calimero_governance_types::NamespaceOp::Group {
                    group_id,
                    key_id,
                    encrypted,
                    ..
                } => calimero_governance_store::decrypt_group_op(
                    store,
                    namespace_id,
                    ContextGroupId::from(*group_id),
                    key_id,
                    encrypted,
                )
                .ok()
                .flatten(),
                calimero_governance_types::NamespaceOp::Root(_) => None,
            };
            ops.push(op_from_namespace_op(
                &signed,
                decrypted.as_ref(),
                delta.id,
                delta.hlc,
                &delta.parents,
            ));
        }
        Some(ops)
    }

    /// Ingest the ops [`collect_namespace_ops`] gathered and mark the namespace
    /// backfilled — the cheap, lock-held half. Always ingests (no early-out on an
    /// already-backfilled namespace) so a *refresh* re-walk — triggered when the
    /// cut cites a head this node authored after the first backfill — actually
    /// folds the new ops; `ingest_op` dedups by id, so re-ingesting the rest is a
    /// cheap no-op. Ingestion order is irrelevant: [`ScopeState::apply`] is
    /// per-slot LWW, so the folded state converges regardless of walk order.
    ///
    /// [`collect_namespace_ops`]: Self::collect_namespace_ops
    pub fn apply_backfill(&mut self, namespace_id: [u8; 32], ops: Vec<Op>) {
        let _ = self.backfilled.insert(namespace_id);
        for op in &ops {
            self.ingest_op(op);
        }
    }

    /// Is `author` a member of `group` at the governance cut named by `heads`,
    /// per the projection? A **pure read** — the caller must have already
    /// backfilled the group's namespace (via [`namespace_to_refresh`] +
    /// [`collect_namespace_ops`] + [`apply_backfill`]) so this can take `&self`
    /// and hold the projection lock only briefly. Returns the type-free `bool` so
    /// the node side needs no `authz`/`op` deps.
    ///
    /// [`namespace_to_refresh`]: Self::namespace_to_refresh
    /// [`collect_namespace_ops`]: Self::collect_namespace_ops
    /// [`apply_backfill`]: Self::apply_backfill
    #[must_use]
    pub fn member_at_cut(
        &self,
        store: &Store,
        group: ContextGroupId,
        author: &PublicKey,
        heads: &[[u8; 32]],
    ) -> Option<bool> {
        // Governance is keyed by namespace (see [`op_from_namespace_op`]): the cut
        // `heads` are namespace-DAG nodes, so resolve over the whole namespace
        // ancestry, then read out membership for THIS group.
        let namespace_id = NamespaceRepository::new(store)
            .resolve(&group)
            .ok()?
            .to_bytes();
        let scope = ScopeId::from(namespace_id);

        // Resolve membership entirely from the AT-CUT folded view: direct, group
        // admin (subgroup creator / `Admin` role), or inherited through an open-
        // subgroup chain — a faithful port of the live `check_path` +
        // `acl_view_at` carve-outs, but over the cut's state, so a membership the
        // cut revoked (e.g. remove-from-root) is NOT granted. The ONLY live read
        // is the immutable namespace-root genesis admin (no governance op carries
        // it); every mutable input — memberships, caps, visibility, the subgroup
        // tree, subgroup-creator admin — comes from the fold.
        let view = self.acl_view_at(&scope, heads);
        let root_group = ContextGroupId::from(namespace_id);
        let root = MetaRepository::new(store)
            .load(&root_group)
            .ok()
            .flatten()
            .map(|meta| (root_group, meta.admin_identity));
        // The namespace root's default member cap (CAN_JOIN_OPEN_SUBGROUPS is set
        // here at creation as a store write, not an op) — base fallback for the
        // inheritance walk's cap check. Immutable-base like the genesis admin.
        let default_cap_base = CapabilitiesRepository::new(store)
            .default_capabilities(&root_group)
            .ok()
            .flatten()
            .unwrap_or(0);
        if view
            .as_ref()
            .is_some_and(|v| v.is_member_at_cut(group, author, root, default_cap_base))
        {
            return Some(true);
        }

        // Materialized-only fallback — the decision group is ENTIRELY absent from
        // the fold (no member folded for it at all), yet the live store has the
        // author as a direct member. This is a restricted subgroup whose
        // membership reached this node as materialized `GroupMember` rows via
        // governance sync (or whose member ops this node can't decrypt), not as
        // foldable ops — so the op-log legitimately can't carry it, exactly like
        // the live resolver's `heads_equal` materialized fast-path. This is the
        // one remaining current-state read (deny direction only — the
        // authoritative grant path never uses it).
        //
        // The `wholly_unfolded` gate prevents masking ONLY for a group with zero
        // folded members: a PARTIALLY-folded group (e.g. some members via
        // cleartext Root ops, others via an encrypted channel this node can't
        // decrypt) has `contains_key == true`, so the fallback is skipped and the
        // un-folded members correctly surface as divergence. It does NOT mask a
        // partial op-fold drop. (Caveat: `apply` drops an emptied group, so a
        // group whose members were all removed via ops also reads as "unfolded" —
        // acceptable here because this is the conservative deny-direction path,
        // which errs toward member.)
        let group_wholly_unfolded = view.as_ref().is_none_or(|v| !v.groups.contains_key(&group));
        if group_wholly_unfolded
            && MembershipRepository::new(store)
                .role_of(&group, author)
                .ok()
                .flatten()
                .is_some()
        {
            return Some(true);
        }

        Some(false)
    }

    /// Authoritative membership verdict for the **grant** direction (overriding
    /// live's reject) — `Some(true)` ONLY when the projection can decide membership
    /// from its own fold, deterministically:
    ///   1. EVERY cited head is folded (complete cut ancestry), so the at-cut walk
    ///      sees the whole picture — including any removal in the cut; and
    ///   2. the at-cut walk ([`AclView::is_member_at_cut`]) confirms membership.
    ///
    /// Returns `None` when the cut isn't fully folded (the projection can't
    /// authoritatively decide — defer to live's reject) and `Some(false)` when the
    /// fold says not-a-member. Unlike [`member_at_cut`], it uses **neither** the
    /// materialized `role_of` fallback **nor** the immutable-root carve-out as a
    /// grant basis: the materialized fallback reads *current* live state, which
    /// races a still-propagating cascade removal and caused a non-deterministic
    /// over-grant in `group-remove-from-root-revokes-inherited`. The grant
    /// direction must rest only on fully-folded, at-cut evidence so it can never
    /// out-run live into authorizing a write live rejected.
    ///
    /// [`member_at_cut`]: Self::member_at_cut
    #[must_use]
    pub fn member_at_cut_authoritative(
        &self,
        store: &Store,
        group: ContextGroupId,
        author: &PublicKey,
        heads: &[[u8; 32]],
    ) -> Option<bool> {
        let namespace_id = NamespaceRepository::new(store)
            .resolve(&group)
            .ok()?
            .to_bytes();
        let scope = ScopeId::from(namespace_id);

        // Require the COMPLETE cited ANCESTRY to be folded — not merely the heads.
        // `acl_view_at` silently truncates at a missing mid-ancestry op, which
        // would leave a since-removed member still folded as present (the
        // over-grant in group-remove-from-root-revokes-inherited: the root-removal
        // op was absent from the log, so the inherited walk still saw the member
        // in the root). If the ancestry isn't whole, abstain (`None`) and defer to
        // live's reject rather than grant on a truncated, possibly-stale view.
        if !self.cut_ancestry_complete(&scope, heads) {
            return None;
        }

        // The genesis root admin + the root's default cap are immutable base state
        // (no governance op carries them), correct at any cut — safe to consult in
        // the authoritative grant path.
        let root_group = ContextGroupId::from(namespace_id);
        let root = MetaRepository::new(store)
            .load(&root_group)
            .ok()
            .flatten()
            .map(|meta| (root_group, meta.admin_identity));
        let default_cap_base = CapabilitiesRepository::new(store)
            .default_capabilities(&root_group)
            .ok()
            .flatten()
            .unwrap_or(0);
        Some(
            self.acl_view_at(&scope, heads)
                .is_some_and(|v| v.is_member_at_cut(group, author, root, default_cap_base)),
        )
    }

    /// Diagnostics for a divergence at `heads` — why does the projection NOT see
    /// `author` in `group`? Distinguishes the failure modes:
    /// - `namespace_log_len == 0` → the projection is empty for this namespace
    ///   (feed/backfill never populated it);
    /// - `heads_in_log == 0` with a non-empty log → the cited cut heads aren't in
    ///   the projection's id-space (cut misalignment / not-yet-fed heads);
    /// - both non-zero but `author_in_any_group == false` → the ops are present
    ///   but the membership fold doesn't place `author` in any group.
    ///
    /// Cheap read-only inspection for the shadow's warning; not on any hot path.
    #[must_use]
    pub fn cut_diagnostics(
        &self,
        store: &Store,
        group: ContextGroupId,
        author: &PublicKey,
        heads: &[[u8; 32]],
    ) -> (bool, bool, usize, usize, bool, bool, usize) {
        let resolved = NamespaceRepository::new(store).resolve(&group).ok();
        let Some(ns) = resolved else {
            return (false, false, 0, 0, false, false, 0);
        };
        let ns_bytes = ns.to_bytes();
        let backfilled = self.backfilled.contains(&ns_bytes);
        let scope = ScopeId::from(ns_bytes);
        let log = self.logs.get(&scope);
        let log_len = log.map_or(0, Vec::len);
        let heads_in_log = match log {
            Some(l) => {
                let ids: HashSet<[u8; 32]> = l.iter().map(|o| o.id).collect();
                heads.iter().filter(|h| ids.contains(*h)).count()
            }
            None => 0,
        };
        let view = self.acl_view_at(&scope, heads);
        let author_in_any = view.as_ref().is_some_and(|v| v.is_scope_member(author));
        // Is the DECISION group folded at all, and how big? Distinguishes
        // "subgroup membership never folded" (group absent / size 0 — a
        // decrypt/sync gap) from "folded but author absent" (re-add / deny-list).
        let decision_group_size = view
            .as_ref()
            .and_then(|v| v.groups.get(&group))
            .map_or(0, std::collections::BTreeMap::len);
        let decision_group_in_view = view.as_ref().is_some_and(|v| v.groups.contains_key(&group));
        (
            backfilled,
            true,
            log_len,
            heads_in_log,
            author_in_any,
            decision_group_in_view,
            decision_group_size,
        )
    }

    /// The role the projection resolves for `member` in `group` at the causal
    /// cut named by `heads`, or `None` if absent. Unlike [`Self::role_of`] (the
    /// `states` snapshot), this folds the cut's ancestry with causal generations,
    /// so an add → remove → re-add chain resolves to the causally-latest state —
    /// the correct answer for the apply-time membership shadow, which resolves at
    /// the just-applied op's own cut.
    #[must_use]
    pub fn role_at_cut(
        &self,
        scope: &ScopeId,
        group: &ContextGroupId,
        member: &PublicKey,
        heads: &[[u8; 32]],
    ) -> Option<GroupMemberRole> {
        self.acl_view_at(scope, heads)?
            .groups
            .get(group)?
            .get(member)
            .cloned()
    }

    /// The role the projection records for `member` in `group` within `scope`,
    /// or `None` if absent (member not present, or the scope hasn't been fed).
    /// The `states` fast-path snapshot — order-converged but NOT causal for
    /// equal-`hlc` governance ops (use [`Self::role_at_cut`] when causal
    /// resolution matters).
    #[must_use]
    pub fn role_of(
        &self,
        scope: &ScopeId,
        group: &ContextGroupId,
        member: &PublicKey,
    ) -> Option<GroupMemberRole> {
        self.states
            .get(scope)?
            .acl_view()
            .groups
            .get(group)?
            .get(member)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU128;

    use calimero_context_config::types::{
        ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation,
    };
    use calimero_op::OpPayload;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_storage::entities::OpMask;
    use calimero_storage::logical_clock::{Timestamp, ID, NTP64};

    use super::*;

    fn hlc(ns: u64) -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(
            NTP64(ns),
            ID::from(NonZeroU128::new(1).unwrap()),
        ))
    }

    fn signed_root(namespace_id: [u8; 32], signer: PublicKey, op: RootOp) -> SignedNamespaceOp {
        SignedNamespaceOp {
            version: 1,
            namespace_id,
            parent_op_hashes: Vec::new(),
            state_hash: [0u8; 32],
            signer,
            nonce: 0,
            op: NamespaceOp::Root(op),
            signature: [0u8; 64],
        }
    }

    /// An invitation-based `MemberJoined` for `group` with `Member` role — a
    /// DIRECT membership op (folds as `MemberAdded`), unlike the open-subgroup
    /// `MemberJoinedOpen` inheritance proof.
    fn member_joined(group: ContextGroupId, member: PublicKey) -> RootOp {
        RootOp::MemberJoined {
            member,
            signed_invitation: SignedGroupOpenInvitation {
                invitation: GroupInvitationFromAdmin {
                    inviter_identity: [0xA1; 32].into(),
                    group_id: group,
                    expiration_timestamp: 1_700_000_000,
                    secret_salt: [0x33; 32],
                    invited_role: 1, // Member
                },
                inviter_signature: "deadbeef".to_string(),
                application_id: None,
                app_key: None,
            },
        }
    }

    #[test]
    fn open_subgroup_join_folds_as_noop_inheritance_proof() {
        // `MemberJoinedOpen` is an open-subgroup inheritance-join PROOF — live
        // writes no persistent direct row and re-derives membership from the
        // anchor — so it folds as `Noop`, not a direct `MemberAdded`. The
        // inheritance walk in `AclView::is_member_at_cut` derives the membership
        // from the (foldable) anchor membership + visibility + cap, so it is
        // revoked on anchor removal and restored on rejoin. Cross-validated
        // against the live resolver in
        // tests/projection_membership_equivalence.rs. The node still occupies its
        // DAG place so an ancestry walk can pass through it.
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let group = [0x33; 32];

        let op = op_from_namespace_op(
            &signed_root(
                ns,
                signer,
                RootOp::MemberJoinedOpen {
                    member,
                    group_id: group,
                },
            ),
            None,
            [0x99; 32],
            hlc(10),
            &[[0x88; 32]],
        );

        assert_eq!(op.id, [0x99; 32], "op still occupies its DAG node");
        assert_eq!(op.scope, ScopeId::from(ns));
        assert_eq!(op.parents, vec![[0x88; 32]], "with its real parents");
        assert_eq!(
            op.payload,
            OpPayload::Noop,
            "open-subgroup inheritance join is a Noop (derived by the walk)"
        );
    }

    #[test]
    fn invitation_join_decodes_group_and_role() {
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let group = ContextGroupId::from([0x44; 32]);
        let signed_invitation = SignedGroupOpenInvitation {
            invitation: GroupInvitationFromAdmin {
                inviter_identity: [0xA1; 32].into(),
                group_id: group,
                expiration_timestamp: 1_700_000_000,
                secret_salt: [0x33; 32],
                invited_role: 0, // Admin
            },
            inviter_signature: "deadbeef".to_string(),
            application_id: None,
            app_key: None,
        };

        let op = op_from_namespace_op(
            &signed_root(
                ns,
                signer,
                RootOp::MemberJoined {
                    member,
                    signed_invitation,
                },
            ),
            None,
            [0x99; 32],
            hlc(10),
            &[],
        );

        assert_eq!(op.scope, ScopeId::from(ns));
        assert_eq!(
            op.payload,
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Admin,
            }
        );
    }

    #[test]
    fn admin_change_folds_under_the_namespace_scope() {
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let op = op_from_namespace_op(
            &signed_root(ns, signer, RootOp::AdminChanged { new_admin: signer }),
            None,
            [0x99; 32],
            hlc(10),
            &[],
        );
        assert_eq!(op.scope, ScopeId::from(ns));
        assert_eq!(op.payload, OpPayload::AdminChanged { new_admin: signer });
    }

    #[test]
    fn undecryptable_group_op_folds_as_a_noop_graph_node() {
        // A group op we can't decrypt (no `decrypted` supplied) carries no
        // membership change we can read, but it MUST still become a node so an
        // ancestry walk can pass through it to the ops behind it (dropping it
        // would orphan them — the bug this guards against).
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let group = ContextGroupId::from([0x33; 32]);
        let op = op_from_namespace_op(
            &signed_group(ns, signer, group),
            None,
            [0x99; 32],
            hlc(10),
            &[[0x88; 32]],
        );
        assert_eq!(
            op.payload,
            OpPayload::Noop,
            "undecryptable group op is a Noop node"
        );
        assert_eq!(op.id, [0x99; 32], "but it still occupies its DAG node");
        assert_eq!(op.parents, vec![[0x88; 32]], "with its real parents");
    }

    #[test]
    fn rotation_entry_maps_to_set_writers_op() {
        let scope = ScopeId::from([0u8; 32]);
        let object = Id::new([0xB0; 32]);
        let signer = PublicKey::from([1u8; 32]);
        let writer = PublicKey::from([9u8; 32]);
        let writers: std::collections::BTreeMap<PublicKey, OpMask> =
            [(writer, OpMask::FULL)].into_iter().collect();

        let entry = RotationLogEntry {
            delta_id: [7u8; 32],
            delta_hlc: hlc(5),
            signer: Some(signer),
            signature: None,
            signed_payload: None,
            new_writers: writers.clone(),
            writers_nonce: 1,
        };

        let op = op_from_rotation_entry(object, scope, &entry).expect("signed rotation maps");
        assert_eq!(op.author, signer, "author is the rotation signer");
        assert_eq!(op.hlc, hlc(5));
        assert_eq!(op.payload, OpPayload::SetWriters { object, writers });

        // Unsigned bootstrap entries have no author and are skipped.
        let unsigned = RotationLogEntry {
            signer: None,
            ..entry
        };
        assert!(op_from_rotation_entry(object, scope, &unsigned).is_none());
    }

    #[test]
    fn acl_view_at_honors_the_causal_cut_over_the_retained_log() {
        let scope = ScopeId::from([0u8; 32]);
        let group = ContextGroupId::from([3u8; 32]);
        let admin = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);

        let build = |ns: u64, parents: Vec<[u8; 32]>, payload: OpPayload| -> Op {
            let h = hlc(ns);
            let id = Op::compute_id(scope, &parents, &admin, &h, &payload);
            Op {
                id,
                scope,
                parents,
                author: admin,
                hlc: h,
                payload,
                expected_scope_root: [0u8; 32],
                signature: [0u8; 64],
            }
        };

        let add = build(
            10,
            vec![],
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Member,
            },
        );
        let remove = build(20, vec![add.id], OpPayload::MemberRemoved { group, member });

        let mut reg = ScopeProjections::new();
        reg.ingest_op(&add);
        reg.ingest_op(&remove);

        // Cut at the add (pre-removal ancestry): the member is present — a write
        // authored here stays authorized even though we've since seen the remove.
        let pre = reg.acl_view_at(&scope, &[add.id]).expect("scope fed");
        assert_eq!(
            pre.groups.get(&group).and_then(|m| m.get(&member)),
            Some(&GroupMemberRole::Member),
        );

        // Cut at the remove (its ancestry includes both): the member is gone.
        let post = reg.acl_view_at(&scope, &[remove.id]).expect("scope fed");
        assert_eq!(post.groups.get(&group).and_then(|m| m.get(&member)), None);

        // Unknown scope ⇒ no view.
        assert!(reg
            .acl_view_at(&ScopeId::from([0xEE; 32]), &[add.id])
            .is_none());
    }

    #[test]
    fn registry_records_membership_under_the_namespace_scope() {
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let group = ContextGroupId::from([0x33; 32]);
        let ns_scope = ScopeId::from(ns);

        let join = op_from_namespace_op(
            &signed_root(ns, signer, member_joined(group, member)),
            None,
            [0x99; 32],
            hlc(10),
            &[],
        );

        let mut reg = ScopeProjections::new();
        // Before the join: nothing recorded.
        assert_eq!(reg.role_of(&ns_scope, &group, &member), None);
        reg.ingest_op(&join);
        // After: the member is recorded for the group, under the namespace scope.
        assert_eq!(
            reg.role_of(&ns_scope, &group, &member),
            Some(GroupMemberRole::Member),
        );
        // A different namespace's scope is unaffected (isolation across namespaces).
        assert_eq!(
            reg.role_of(&ScopeId::from([0xEE; 32]), &group, &member),
            None,
        );
    }

    #[test]
    fn group_op_membership_folds_under_the_namespace_scope() {
        use calimero_governance_types::GroupOp;

        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x77; 32]);
        let group = ContextGroupId::from([0x33; 32]);
        let ns_scope = ScopeId::from(ns);

        // Admin-push add (decrypted GroupOp::MemberAdded) folds into groups[group].
        let add = op_from_namespace_op(
            &signed_group(ns, signer, group),
            Some(&GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Admin,
            }),
            [0xAB; 32],
            hlc(5),
            &[],
        );
        assert_eq!(
            add.scope, ns_scope,
            "group ops key under the namespace scope"
        );
        assert_eq!(add.id, [0xAB; 32], "op carries the namespace delta id");

        let mut reg = ScopeProjections::new();
        reg.ingest_op(&add);
        assert_eq!(
            reg.role_of(&ns_scope, &group, &member),
            Some(GroupMemberRole::Admin),
        );

        // A later removal at a higher hlc wins → member gone.
        let remove = op_from_namespace_op(
            &signed_group(ns, signer, group),
            Some(&GroupOp::MemberRemoved {
                member,
                expected_group_state_hash: [0u8; 32],
                expected_context_state_hashes: Vec::new(),
            }),
            [0xCD; 32],
            hlc(9),
            &[[0xAB; 32]],
        );
        reg.ingest_op(&remove);
        assert_eq!(reg.role_of(&ns_scope, &group, &member), None);

        // A truly out-of-model group op folds as a Noop node (still recorded).
        let other = op_from_namespace_op(
            &signed_group(ns, signer, group),
            Some(&GroupOp::Noop),
            [0xEF; 32],
            hlc(11),
            &[],
        );
        assert_eq!(other.payload, OpPayload::Noop);
    }

    /// A namespace DAG interleaves a group-membership op with a namespace-root op
    /// on ONE parent chain. Keying governance under the namespace scope keeps the
    /// ancestry walk whole, so a cut at the namespace head still sees the member
    /// (the per-group-scope keying that truncated this walk was the bug).
    #[test]
    fn namespace_head_cut_sees_a_member_joined_earlier_on_the_chain() {
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let group = ContextGroupId::from([0x33; 32]);

        //   MemberJoinedOpen(group, member)  <--  AdminChanged(namespace)  [head]
        let join = op_from_namespace_op(
            &signed_root(ns, signer, member_joined(group, member)),
            None,
            [0x99; 32],
            hlc(10),
            &[],
        );
        let admin = op_from_namespace_op(
            &signed_root(ns, signer, RootOp::AdminChanged { new_admin: signer }),
            None,
            [0xAA; 32],
            hlc(20),
            &[[0x99; 32]], // child of the join
        );

        let mut reg = ScopeProjections::new();
        reg.ingest_op(&join);
        reg.ingest_op(&admin);

        // Cut = the namespace head (AdminChanged). The member joined causally
        // before it, so the projection must still see them at that cut.
        let ns_scope = ScopeId::from(ns);
        let view = reg
            .acl_view_at(&ns_scope, &[[0xAA; 32]])
            .expect("scope fed");
        assert!(
            view.groups
                .get(&group)
                .is_some_and(|m| m.contains_key(&member)),
            "member must be visible at the namespace-head cut"
        );
    }

    /// Build a `NamespaceOp::Group` envelope for the namespace; the cleartext op
    /// is supplied separately to [`op_from_namespace_op`] as `decrypted`.
    fn signed_group(
        namespace_id: [u8; 32],
        signer: PublicKey,
        group: ContextGroupId,
    ) -> SignedNamespaceOp {
        SignedNamespaceOp {
            version: 1,
            namespace_id,
            parent_op_hashes: Vec::new(),
            state_hash: [0u8; 32],
            signer,
            nonce: 0,
            op: NamespaceOp::Group {
                group_id: group.to_bytes(),
                key_id: [0u8; 32],
                encrypted: calimero_governance_types::EncryptedGroupOp {
                    nonce: [0u8; 12],
                    ciphertext: Vec::new(),
                },
                key_rotation: None,
            },
            signature: [0u8; 64],
        }
    }
}
