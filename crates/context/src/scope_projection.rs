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
    CapabilitiesRepository, DenyListRepository, MembershipRepository, MetaRepository,
    NamespaceDagService, NamespaceOpLogService, NamespaceRepository,
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

/// The resolved at-cut context for an apply-auth read: the folded `AclView`, the
/// genesis root tuple `(root_group, genesis_admin)`, and the namespace default-cap
/// base. Produced by `ScopeProjections::auth_cut_context`.
type AuthCutContext = (
    calimero_authz::AclView,
    Option<(ContextGroupId, PublicKey)>,
    u32,
);

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

    /// The owning namespace's CURRENT governance heads for `group` — the cut that
    /// represents "now" for a current-state membership read (resolve the group to
    /// its namespace, then read that DAG's head record). `None` if the group can't
    /// be resolved or the head record is unreadable. Pair with
    /// [`member_at_cut`](Self::member_at_cut) (via the node's refreshing wrapper) to
    /// answer "is X a member right now" off the projection instead of the live
    /// materialized rows.
    #[must_use]
    pub fn namespace_current_heads(store: &Store, group: ContextGroupId) -> Option<Vec<[u8; 32]>> {
        let namespace_id = NamespaceRepository::new(store)
            .resolve(&group)
            .ok()?
            .to_bytes();
        NamespaceDagService::new(store, namespace_id)
            .read_head_record()
            .ok()
            .map(|head| head.parent_hashes)
    }

    /// Current-state membership via a projection built FRESH from the store — for
    /// the context-layer query handlers, which (unlike the node) hold no cached
    /// `ScopeProjections`. Resolves the namespace, folds its governance DAG, and
    /// reads `member_at_cut` at the current heads. `None` when the namespace/heads
    /// can't be resolved or the cut isn't decidable.
    ///
    /// Per-query rebuild: acceptable for low-frequency read endpoints (a real
    /// namespace's governance history is small); hot paths use the node's cached
    /// projection via `projection_member_at_cut` instead.
    #[must_use]
    pub fn member_now_ephemeral(
        store: &Store,
        group: &ContextGroupId,
        member: &PublicKey,
    ) -> Option<bool> {
        let namespace_id = NamespaceRepository::new(store)
            .resolve(group)
            .ok()?
            .to_bytes();
        let mut proj = Self::new();
        let Some(ops) = Self::collect_namespace_ops(store, namespace_id) else {
            // Governance head unreadable (store fault). Don't silently read an empty
            // projection — surface it and let the caller fall back to live.
            tracing::warn!(
                group_id = ?group,
                "member_now_ephemeral: governance head unreadable; falling back to live"
            );
            return None;
        };
        proj.apply_backfill(namespace_id, ops);
        // Read the cut heads AFTER folding so the cut never names ops the fold is
        // missing: the fold covers the head observed by `collect_namespace_ops`, and
        // these heads are read no earlier than that, so if a concurrent governance
        // op advanced the head mid-rebuild, the new head's ancestry isn't fully
        // folded and `member_at_cut`'s completeness guard returns `None` (defer to
        // live) rather than deciding against a stale cut that predates a revocation.
        let heads = NamespaceDagService::new(store, namespace_id)
            .read_head_record()
            .ok()?
            .parent_hashes;
        proj.member_at_cut(store, *group, member, &heads)
    }

    /// The membership verdict a query gate should ACT on: the projection's
    /// current-state answer ([`member_now_ephemeral`](Self::member_now_ephemeral)),
    /// falling back to live only when the projection can't decide (`None`). Logs
    /// `unified_projection_divergence` (plane `membership-query`, caught by the e2e
    /// gate) when the two definitely disagree — keeping live as the gated
    /// cross-check until the resolver is deleted in F5.
    pub fn member_now_checked(
        store: &Store,
        group: &ContextGroupId,
        member: &PublicKey,
    ) -> eyre::Result<bool> {
        if let Some(p) = Self::member_now_ephemeral(store, group, member) {
            // The projection is authoritative for the query gate (validated
            // divergence-free across the e2e `membership-query` plane); act on it.
            return Ok(p);
        }
        // Projection couldn't decide (cold/partial fold or store fault) — fall back
        // to live, whose error legitimately propagates. (Live retires in #29b.)
        MembershipRepository::new(store).is_member(group, member)
    }

    /// The effective member-identity union across `groups`, folding the namespace
    /// projection ONCE and reading ONE cut — so every group is evaluated at the
    /// SAME governance position. A per-group rebuild would re-fold and re-read heads
    /// for each group,
    /// which both multiplies the cost by the subtree size and (under concurrent
    /// governance) evaluates groups at DIFFERENT cuts, producing a synthetic
    /// cohort mismatch. `groups` must all resolve to `namespace_root`'s namespace.
    #[must_use]
    pub fn member_identities_subtree_ephemeral(
        store: &Store,
        namespace_root: &ContextGroupId,
        groups: &[ContextGroupId],
    ) -> Option<std::collections::BTreeSet<PublicKey>> {
        let namespace_id = NamespaceRepository::new(store)
            .resolve(namespace_root)
            .ok()?
            .to_bytes();
        let (proj, heads) = Self::ephemeral_fold(store, namespace_id)?;
        let scope = ScopeId::from(namespace_id);
        // Defer to live on a partial fold (governance-backfill race) rather than
        // returning an under-counted cohort — same guard as the membership gate.
        if !proj.cut_ancestry_complete(&scope, &heads) {
            return None;
        }
        let view = proj.acl_view_at(&scope, &heads)?;
        let mut out = std::collections::BTreeSet::new();
        for group in groups {
            out.extend(Self::member_identities_in_view(
                &view,
                store,
                namespace_id,
                group,
            ));
        }
        Some(out)
    }

    /// Build the shared ephemeral projection for `group`'s namespace ONCE — the
    /// expensive part (`collect_namespace_ops` RocksDB DAG walk + fold) plus the
    /// current heads. A handler that needs BOTH the membership gate and the
    /// effective-member enumeration folds once via this and reuses the result for
    /// [`member_now_checked_with`](Self::member_now_checked_with) and
    /// [`member_entries_with`](Self::member_entries_with), instead of two
    /// independent folds. `None` (with a warn) on a store fault — the caller falls
    /// back to live.
    #[must_use]
    pub fn ephemeral_projection(
        store: &Store,
        group: &ContextGroupId,
    ) -> Option<(Self, [u8; 32], Vec<[u8; 32]>)> {
        let namespace_id = NamespaceRepository::new(store)
            .resolve(group)
            .ok()?
            .to_bytes();
        let (proj, heads) = Self::ephemeral_fold(store, namespace_id)?;
        Some((proj, namespace_id, heads))
    }

    /// The effective member-identity SET of `group` from an ALREADY-built
    /// projection. The authoritative read for the identity-set enumeration consumers
    /// (member count, migration cohort) now that the set is validated divergence-free
    /// across the e2e `membership-enum` plane; the role-bearing `list_group_members`
    /// consumer uses [`member_entries_with`](Self::member_entries_with), which builds
    /// on this same set. `None` when the scope wasn't fed (an empty namespace) OR the
    /// cited ancestry isn't fully folded (a governance-backfill race) — caller falls
    /// back to live, exactly as the membership gate's `member_at_cut` defers on an
    /// incomplete cut. Without this guard a partial fold would silently UNDER-count.
    #[must_use]
    pub fn member_identities_with(
        &self,
        store: &Store,
        namespace_id: [u8; 32],
        group: &ContextGroupId,
        heads: &[[u8; 32]],
    ) -> Option<std::collections::BTreeSet<PublicKey>> {
        let scope = ScopeId::from(namespace_id);
        if !self.cut_ancestry_complete(&scope, heads) {
            return None;
        }
        let view = self.acl_view_at(&scope, heads)?;
        Some(Self::member_identities_in_view(
            &view,
            store,
            namespace_id,
            group,
        ))
    }

    /// The full effective-member ENUMERATION with roles at the cut — the
    /// projection's answer for `list_group_members`. For a FOLDED group it returns
    /// `(identity, role)` for every effective member, with the identity SET EQUAL to
    /// [`member_identities_with`](Self::member_identities_with) (validated
    /// divergence-free on the `membership-enum` plane) and each role resolved by the
    /// same `member_path_at_cut` the `membership-role` plane validated.
    ///
    /// `None` (defer to live) when the cited ancestry isn't fully folded — enforced
    /// by `auth_cut_context`, which gates on `cut_ancestry_complete` and returns
    /// `None` on a partial fold — OR the target group's direct membership isn't
    /// folded at all (`!view.groups.contains_key`).
    ///
    /// NOTE on the unfolded-group case: unlike `member_identities_with`, which
    /// INJECTS live `list ∪ enumerate_inherited` rows for an unfolded group (its
    /// materialized fallback), this returns `None` there — so the identity-set
    /// equality above holds for FOLDED groups only. The end result is the same: on
    /// `None` the handler falls back to the live `list ∪ enumerate_inherited` union,
    /// yielding those same rows (with roles). Deferring via `None` keeps the live
    /// `list` read out of this projection method.
    ///
    /// PARTIAL FOLD: the materialized fallback is all-or-nothing — a group with even
    /// one folded direct member is treated as fully folded, so a materialized
    /// `GroupMember` row that never entered the fold would be missing. This is a
    /// property of the shared `member_identities_in_view` candidate universe (the
    /// merged count/cohort consumers share it), not new here. It is bounded by sync
    /// delivering a group's membership atomically — fully folded or fully
    /// materialized, not partial — which is why the e2e `membership-enum` plane held
    /// 0 divergences; the durable close is folding-completeness in P6 (#17).
    ///
    /// If an id from the validated set resolves to `MemberPathAtCut::None` (a fold
    /// inconsistency — the membership walk and the candidate filter disagreeing), the
    /// whole enumeration abandons the projection and defers to live rather than
    /// returning a silently truncated list.
    pub fn member_entries_with(
        &self,
        store: &Store,
        namespace_id: [u8; 32],
        group: &ContextGroupId,
        heads: &[[u8; 32]],
    ) -> Option<Vec<(PublicKey, GroupMemberRole)>> {
        // Fold the view ONCE: `auth_cut_context` gates `cut_ancestry_complete` and
        // builds the `acl_view_at` + root + cap. Both the validated id set and the
        // per-member role derive from THIS view — `member_identities_in_view` is the
        // pre-folded variant, so the ancestry walk runs once per request, not twice
        // (the gate `member_identities_with` would otherwise fold a second time).
        let (view, root, default_cap_base) = self.auth_cut_context(store, *group, heads)?;
        if !view.groups.contains_key(group) {
            return None;
        }
        // Reuse the SAME `root`/`default_cap_base` for the candidate filter that the
        // role walk below uses, so they can't disagree across two store reads (and
        // `Meta`/`Capabilities` are read once per request, not twice).
        let ids = Self::member_identities_in_view_with_ctx(
            &view,
            store,
            namespace_id,
            group,
            root,
            default_cap_base,
        );
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            let role = match view.member_path_at_cut(*group, &id, root, default_cap_base) {
                calimero_authz::MemberPathAtCut::None => {
                    // The validated id set placed `id` in `group`, but the role walk
                    // rejects it — the candidate filter and `member_path_at_cut`
                    // disagreeing. Never silently drop the member (a truncated list
                    // has no signal); abandon the projection enumeration and let the
                    // caller fall back to live's complete union.
                    tracing::warn!(
                        marker = "unified_projection_divergence",
                        plane = "membership-enum",
                        group_id = ?group,
                        %id,
                        "member_entries: validated-set id has no at-cut path; deferring whole enumeration to live"
                    );
                    return None;
                }
                calimero_authz::MemberPathAtCut::Direct { role } => role,
                calimero_authz::MemberPathAtCut::Inherited {
                    via_admin: true, ..
                } => GroupMemberRole::Admin,
                calimero_authz::MemberPathAtCut::Inherited {
                    anchor,
                    via_admin: false,
                } => {
                    // `member_path_at_cut` only emits this arm when the anchor row is
                    // present (the walk sets it inside `groups[anchor].contains(id)`),
                    // read from the SAME view, so the lookup can't miss. On the
                    // authoritative path don't silently guess `Member` if it somehow
                    // does — that's a fold inconsistency; bail to live like the
                    // `None` arm, rather than misreport a role with no signal.
                    let Some(role) = view.groups.get(&anchor).and_then(|m| m.get(&id)).cloned()
                    else {
                        // A member in the validated identity set whose anchor row is
                        // absent from the SAME view is a structural inconsistency in
                        // the membership graph, not a role-only mismatch — so this is
                        // an enum-plane divergence, matching the `None` arm above.
                        tracing::warn!(
                            marker = "unified_projection_divergence",
                            plane = "membership-enum",
                            group_id = ?group,
                            %id,
                            ?anchor,
                            "member_entries: inherited anchor row absent; deferring whole enumeration to live"
                        );
                        return None;
                    };
                    role
                }
            };
            entries.push((id, role));
        }
        Some(entries)
    }

    /// The shared fold primitive for both [`ephemeral_projection`](Self::ephemeral_projection)
    /// and [`ephemeral_view`](Self::ephemeral_view): collect `namespace_id`'s
    /// persisted governance DAG into a fresh projection and read its current heads
    /// (AFTER the fold — see `member_now_ephemeral`). `None` (with a warn) on a
    /// store fault so the caller falls back to live rather than reading an empty
    /// projection.
    fn ephemeral_fold(store: &Store, namespace_id: [u8; 32]) -> Option<(Self, Vec<[u8; 32]>)> {
        let mut proj = Self::new();
        let Some(ops) = Self::collect_namespace_ops(store, namespace_id) else {
            tracing::warn!(
                namespace = ?namespace_id,
                "ephemeral_fold: governance head unreadable; caller falls back to live"
            );
            return None;
        };
        proj.apply_backfill(namespace_id, ops);
        let heads = NamespaceDagService::new(store, namespace_id)
            .read_head_record()
            .ok()?
            .parent_hashes;
        Some((proj, heads))
    }

    /// The gate verdict over an ALREADY-built ephemeral projection — same contract
    /// as [`member_now_checked`](Self::member_now_checked) (act on the projection,
    /// live on `None`), but reusing a shared fold.
    pub fn member_now_checked_with(
        &self,
        store: &Store,
        group: &ContextGroupId,
        member: &PublicKey,
        heads: &[[u8; 32]],
    ) -> eyre::Result<bool> {
        if let Some(p) = self.member_at_cut(store, *group, member, heads) {
            return Ok(p);
        }
        MembershipRepository::new(store).is_member(group, member)
    }

    /// The effective member-identity set of `group` from an already-folded `view`
    /// of its namespace. Candidate universe = every direct member of any group in
    /// the view plus the group/root admins (a superset the walk narrows). Mirrors
    /// three live behaviours:
    /// * the at-cut inheritance walk (`is_member_at_cut`), so the set is consistent
    ///   with the boolean reads;
    /// * the enumeration DENY ASYMMETRY — `is_member`/`check_path` keeps a denied
    ///   member (still an `Inherited` path) but `enumerate_inherited` EXCLUDES a
    ///   denied INHERITED member (direct members are never deny-filtered);
    /// * the namespace-leave CASCADE — a subgroup member must also be a namespace
    ///   ROOT member (the single `MemberLeft` op the projection folds doesn't carry
    ///   the local cascade that removes descendant rows).
    ///
    /// MATERIALIZED FALLBACK: for a group with NO direct member folded (a Restricted
    /// subgroup whose membership reached this node as materialized `GroupMember`
    /// rows, or whose member ops this node can't decrypt), the fold carries nothing
    /// — defer to live's full `list ∪ enumerate_inherited` for that group, so the
    /// set is neither a spurious subset (missing materialized direct rows) nor an
    /// under-count (missing inherited members the unfolded structure can't derive).
    #[must_use]
    pub fn member_identities_in_view(
        view: &calimero_authz::AclView,
        store: &Store,
        namespace_id: [u8; 32],
        group: &ContextGroupId,
    ) -> std::collections::BTreeSet<PublicKey> {
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
        Self::member_identities_in_view_with_ctx(
            view,
            store,
            namespace_id,
            group,
            root,
            default_cap_base,
        )
    }

    /// [`member_identities_in_view`](Self::member_identities_in_view) with the
    /// genesis `root` tuple + `default_cap_base` supplied by the caller instead of
    /// re-read here. `member_entries_with` passes the SAME values its role walk
    /// (`member_path_at_cut`) uses, so the candidate filter and the role resolution
    /// can't disagree on the admin/cap base across two store reads, and the
    /// `MetaRepository`/`CapabilitiesRepository` reads happen once per request.
    #[must_use]
    pub fn member_identities_in_view_with_ctx(
        view: &calimero_authz::AclView,
        store: &Store,
        namespace_id: [u8; 32],
        group: &ContextGroupId,
        root: Option<(ContextGroupId, PublicKey)>,
        default_cap_base: u32,
    ) -> std::collections::BTreeSet<PublicKey> {
        let root_group = ContextGroupId::from(namespace_id);

        // Candidate universe — provably COMPLETE w.r.t. `is_member_at_cut`, which
        // accepts an identity only as: a direct member of `group` or an ancestor
        // (every group's direct members are in `view.groups.values()`), a folded
        // group/subgroup admin of `group` or an ancestor (all in
        // `view.group_admin.values()` — one genesis admin per group, plus Admin-role
        // holders already counted as direct members), or the genesis root admin
        // (`root`). So no accepted identity lies outside this set.
        let mut candidates: std::collections::BTreeSet<PublicKey> =
            std::collections::BTreeSet::new();
        for members in view.groups.values() {
            candidates.extend(members.keys().copied());
        }
        candidates.extend(view.group_admin.values().copied());
        if let Some((_, admin)) = root {
            let _ = candidates.insert(admin);
        }

        let deny = DenyListRepository::new(store);
        let mut result: std::collections::BTreeSet<PublicKey> = candidates
            .into_iter()
            .filter(|c| view.is_member_at_cut(*group, c, root, default_cap_base))
            // Namespace-leave cascade: every (sub)group member must also be a
            // namespace-ROOT member (live has no subgroup member who isn't one; the
            // folded single `MemberLeft` doesn't carry the descendant-row cascade).
            // For `group == root_group` this filter is a no-op on purpose — the
            // FIRST filter above (`is_member_at_cut(*group, …)` with `*group ==
            // root_group`) already decides root membership directly, and the root's
            // `MemberLeft` IS folded, so there's no un-cascaded stale row to drop.
            .filter(|c| {
                *group == root_group
                    || view.is_member_at_cut(root_group, c, root, default_cap_base)
            })
            // Deny asymmetry: drop a denied INHERITED member; never deny-filter a
            // direct member (live's `list` doesn't consult the deny-list).
            .filter(|c| {
                let is_direct = view
                    .groups
                    .get(group)
                    .is_some_and(|members| members.contains_key(c));
                is_direct || !deny.is_denied(group, c).unwrap_or(false)
            })
            .collect();

        // Materialized fallback for a wholly-unfolded group (no direct member folded
        // — a Restricted subgroup whose membership reached this node as materialized
        // rows, or whose member ops it can't decrypt). The fold has NO opinion for
        // such a group, so defer entirely to live's `list ∪ enumerate_inherited`
        // rather than the (empty/partial) folded candidate set. These live rows are
        // already cascade- and removal-consistent; re-filtering them through this
        // node's INCOMPLETE fold (the reason we're falling back) would wrongly drop
        // valid members, so they bypass the fold-based filters above by design.
        if !view.groups.contains_key(group) {
            let live = MembershipRepository::new(store);
            // Defer fully to live for an unfolded group: add BOTH its materialized
            // direct rows (`list`) AND its inherited members (`enumerate_inherited`).
            // The fold has no opinion here, so anything less would under-include the
            // inherited side the unfolded structure can't derive.
            if let Ok(rows) = live.list(group, 0, usize::MAX) {
                result.extend(rows.into_iter().map(|(pk, _)| pk));
            }
            if let Ok(inherited) = live.enumerate_inherited(group) {
                result.extend(inherited.into_iter().map(|(pk, _)| pk));
            }
        }
        result
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

        // The fold is only authoritative enough to DENY when the FULL cited
        // ancestry is present. A proactive governance backfill races incoming
        // state deltas: the write can arrive before this node has folded the
        // author's membership chain, leaving the cut's ancestry truncated. An
        // INHERITED open-subgroup membership is especially exposed — deriving it
        // needs the whole chain folded (anchor membership + the subgroup edge +
        // its visibility + the join cap), far more state than a direct membership
        // fold did, so a partial fold spuriously reads not-a-member. Rejecting on
        // that partial view would be a false deny of a real member. Defer to live
        // (`None`) until the ancestry is whole; the at-cut walk above then decides
        // correctly. Symmetric with `member_at_cut_authoritative`, which gates
        // grants on this same completeness predicate.
        if !self.cut_ancestry_complete(&scope, heads) {
            return None;
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

    /// Is `author` an ADMIN of `group` at the cut named by `heads`, authoritatively
    /// — `Some(true)`/`Some(false)` only when the COMPLETE cited ancestry is folded,
    /// `None` otherwise (defer to live). The apply-auth analogue of
    /// [`member_at_cut_authoritative`](Self::member_at_cut_authoritative): admin =
    /// a folded group admin (subgroup creator / `Admin`-role holder, via
    /// `is_group_admin`) OR the immutable genesis root admin for the root group.
    #[must_use]
    pub fn is_admin_at_cut(
        &self,
        store: &Store,
        group: ContextGroupId,
        author: &PublicKey,
        heads: &[[u8; 32]],
    ) -> Option<bool> {
        let (view, root, _) = self.auth_cut_context(store, group, heads)?;
        Some(view.is_authorized_admin(group, author, root))
    }

    /// Is `author` an admin of `group` OR a holder of any bit in `capability` at
    /// the cut — the apply-auth analogue of live's `is_authorized_with_capability`.
    /// Same authoritative `None`-on-incomplete-ancestry contract as
    /// [`is_admin_at_cut`](Self::is_admin_at_cut). The capability is the member's
    /// folded cap, falling back to the namespace default-cap base (a store-written
    /// genesis fact, as in [`member_at_cut`](Self::member_at_cut)).
    #[must_use]
    pub fn is_admin_or_capability_at_cut(
        &self,
        store: &Store,
        group: ContextGroupId,
        author: &PublicKey,
        capability: u32,
        heads: &[[u8; 32]],
    ) -> Option<bool> {
        let (view, root, default_cap_base) = self.auth_cut_context(store, group, heads)?;
        if view.is_authorized_admin(group, author, root) {
            return Some(true);
        }
        let folded = view.capability(&group, author);
        let effective = if folded != 0 {
            folded
        } else {
            default_cap_base
        };
        Some(effective & capability != 0)
    }

    /// Would removing/demoting `member` orphan `group`'s admins at the cut — is
    /// `member` an admin of `group` AND the only Admin-role member there? Backs the
    /// circular last-admin invariants, resolved at the op's PARENT cut. Same
    /// authoritative `None`-on-incomplete-ancestry contract as the other at-cut
    /// reads.
    ///
    /// Mirrors live exactly (`GroupMembershipView::is_admin` + `has_another_admin`):
    /// `member` counts as an admin via a direct `Admin`-role row OR the genesis group
    /// admin (`group_admin` / namespace-root); but "another admin" counts only
    /// another `Admin`-role ROW (`groups[group]`) — the genesis admin alone does NOT
    /// satisfy it, matching live's row-based `has_another_admin`.
    #[must_use]
    pub fn is_last_admin_at_cut(
        &self,
        store: &Store,
        group: ContextGroupId,
        member: &PublicKey,
        heads: &[[u8; 32]],
    ) -> Option<bool> {
        let (view, root, _) = self.auth_cut_context(store, group, heads)?;
        let rows = view.groups.get(&group);
        // `member` is an admin: a direct `Admin` row, or the genesis admin (the
        // folded subgroup creator, or the namespace-root genesis admin).
        let member_is_admin = rows
            .and_then(|m| m.get(member))
            .is_some_and(|r| *r == GroupMemberRole::Admin)
            || view.group_admin.get(&group) == Some(member)
            || root.is_some_and(|(root_g, root_admin)| root_g == group && root_admin == *member);
        if !member_is_admin {
            return Some(false);
        }
        // Another admin exists only via a distinct `Admin`-role ROW — matching live's
        // `has_another_admin`, which scans stored rows and ignores the genesis admin.
        let has_other = rows.is_some_and(|m| {
            m.iter()
                .any(|(k, r)| *r == GroupMemberRole::Admin && k != member)
        });
        Some(!has_other)
    }

    /// Shared setup for the at-cut admin/capability reads: resolve the namespace,
    /// gate on COMPLETE cited ancestry (so the verdict is authoritative — `None`
    /// otherwise, defer to live), build the folded view + the genesis root tuple
    /// (`AclView::is_authorized_admin` prefers the folded root admin, tracking
    /// `AdminChanged`; this is the un-folded base) + the namespace default cap.
    fn auth_cut_context(
        &self,
        store: &Store,
        group: ContextGroupId,
        heads: &[[u8; 32]],
    ) -> Option<AuthCutContext> {
        let namespace_id = NamespaceRepository::new(store)
            .resolve(&group)
            .ok()?
            .to_bytes();
        let scope = ScopeId::from(namespace_id);
        if !self.cut_ancestry_complete(&scope, heads) {
            return None;
        }
        let view = self.acl_view_at(&scope, heads)?;
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
        Some((view, root, default_cap_base))
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
