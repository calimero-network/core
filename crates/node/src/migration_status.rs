//! In-memory TTL cache of per-(namespace, peer) migration heartbeats.
//!
//! PR-6c Task 6c.8. Modeled directly on [`crate::readiness::ReadinessCache`]:
//! a `Mutex<BTreeMap<(namespace, peer), CacheEntry>>` with TTL fresh-peers
//! filtering and opportunistic eviction, populated by verified-on-receive
//! heartbeats and never persisted.
//!
//! A [`SignedMigrationHeartbeat`] is purely observability telemetry — signed
//! TTL gossip a node publishes on the namespace topic to advertise its loaded
//! `schema_version` and remaining unconverted residue. It is NOT replicated
//! governance state and must never be treated as a migration gate; the
//! `get_migration_status` rollup (Task 6c.9) reads this cache to report
//! per-cohort completion, and a member with no fresh heartbeat is `unknown`
//! (never silently "migrated").
//!
//! **Verification contract**: [`MigrationStatusCache::insert`] assumes the
//! heartbeat has already been verified for signature AND namespace membership
//! by the caller. The single legitimate caller is the receiver-side
//! `network_event::namespace` dispatch, which calls
//! `calimero_context::governance_broadcast::verify_migration_heartbeat`
//! (sig + member-set check) BEFORE invoking `insert`. Putting verification
//! inside `insert` would couple the cache to `&Store`, drag
//! namespace-membership state into this module, and duplicate work since the
//! receiver gate runs first — exactly the split the readiness cache uses.
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use actix::{Actor, AsyncContext, Context, Handler, Message};
use calimero_context::group_store::NamespaceRepository;
use calimero_context_client::group::MigrationFailureKind;
use calimero_context_client::local_governance::{NamespaceTopicMsg, SignedMigrationHeartbeat};
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::messages::MigrationStatusReport;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use zeroize::Zeroize;

/// Default freshness window for a cached heartbeat. A node beats on-change
/// and on a low-frequency periodic tick; 60s matches the readiness cache's
/// TTL so a member that stopped reporting drops to `unknown` rather than
/// pinning a stale "migrated" claim.
pub const DEFAULT_HEARTBEAT_TTL: Duration = Duration::from_secs(60);

/// Maximum tolerated drift between a heartbeat's `ts_millis` and local
/// wall-clock. Mirrors [`crate::readiness::MAX_BEACON_CLOCK_DRIFT_MS`]:
/// heartbeats claiming a wall-clock more than this far in the future are
/// rejected to close the cache-poisoning vector documented on
/// [`MigrationStatusCache::insert`].
pub const MAX_HEARTBEAT_CLOCK_DRIFT_MS: u64 = 60_000;

/// Per-(namespace, peer) snapshot of the most recent fresh heartbeat we
/// have received from that peer.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// Schema/binary version the peer has loaded.
    pub schema_version: u32,
    /// Unconverted Convergent ("auto") contexts the peer still has pending.
    pub residue_auto: u64,
    /// Unconverted identity-gated entries the peer still has pending
    /// (the peer's local-derived residue scan from Task 6c.6).
    pub residue_identity: u64,
    /// Governance HLC the peer has synced/applied through.
    pub synced_up_to_hlc: u64,
    /// Peer's self-reported pending-authored count (sum across its namespace
    /// contexts). Surfaced in the rollup as `membersPendingSignature` (6f).
    pub authored_remaining: u64,
    /// Peer's self-reported migration-failure discriminant (`0` = none, `1` =
    /// migration-check aborted, `2` = apply errored). Raw `u8` mirroring the
    /// wire; the server maps it back to a typed kind for the rollup.
    pub migration_failed: u8,
    /// Peer-signed millis-since-epoch from the heartbeat itself.
    /// Authoritative per-peer ordering signal — used by `insert` to drop
    /// stale heartbeats that gossipsub may re-deliver out-of-order on mesh
    /// churn / peer reconnect.
    pub ts_millis: u64,
    /// Local receive instant — the TTL freshness reference.
    pub received_at: Instant,
}

/// Project a cached heartbeat into the transport-neutral
/// [`MigrationStatusReport`] DTO the admin route threads into the
/// `get_migration_status` rollup (Task 6c.9). Pure 1:1 field map; the peer's
/// `ts_millis` becomes the report's `reported_at`. The cache-local
/// `received_at` instant is intentionally dropped — the rollup pins on the
/// signed `synced_up_to_hlc`, never on local receive time.
#[must_use]
pub fn cache_entry_to_report(entry: &CacheEntry) -> MigrationStatusReport {
    MigrationStatusReport {
        schema_version: entry.schema_version,
        residue_auto: entry.residue_auto,
        residue_identity: entry.residue_identity,
        synced_up_to_hlc: entry.synced_up_to_hlc,
        reported_at: entry.ts_millis,
        authored_remaining: entry.authored_remaining,
        migration_failed: entry.migration_failed,
    }
}

/// Per-namespace, per-peer migration-heartbeat cache.
///
/// Uses `BTreeMap` (not `HashMap`) because
/// `calimero_primitives::identity::PublicKey` derives `Ord` but not `Hash`
/// — identical rationale to [`crate::readiness::ReadinessCache`]. Lookups
/// are O(log n) on a per-namespace map holding at most one entry per peer
/// (the practical n is the namespace member count).
#[derive(Debug, Default)]
pub struct MigrationStatusCache {
    entries: Mutex<BTreeMap<([u8; 32], PublicKey), CacheEntry>>,
}

impl MigrationStatusCache {
    /// Acquire the entries map, recovering from a poisoned mutex.
    ///
    /// A `PoisonError` only happens if a previous holder panicked while the
    /// guard was live; the BTreeMap's invariants are not at risk here, so
    /// continuing with the inner guard via `into_inner()` is strictly
    /// preferable to permanently DoSing the migration-status subsystem on
    /// the first transient panic. Mirrors `ReadinessCache::entries_lock`.
    fn entries_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<([u8; 32], PublicKey), CacheEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Insert a heartbeat into the cache.
    ///
    /// **Verification contract**: this method assumes the heartbeat has
    /// already been verified for signature AND namespace membership by the
    /// caller (see the module docs). The receiver-side dispatch runs
    /// `verify_migration_heartbeat` before calling this.
    ///
    /// Insert iff the incoming heartbeat is *newer* than any cached entry
    /// from the same peer (by `ts_millis`, with `synced_up_to_hlc` as a
    /// tiebreaker on clock equality). Gossipsub does not guarantee delivery
    /// order — without this filter an older re-delivered heartbeat could
    /// overwrite a fresher one, regressing a peer's reported residue.
    ///
    /// Also rejects heartbeats with `ts_millis` more than
    /// [`MAX_HEARTBEAT_CLOCK_DRIFT_MS`] ahead of local wall-clock. Without
    /// this bound a malicious or clock-skewed member could sign a heartbeat
    /// with `ts_millis = year 2100`, poisoning their cache entry: every
    /// subsequent legitimate heartbeat would be dropped by the
    /// `older-than-existing` filter, freezing the reported residue at
    /// attacker-chosen values indefinitely.
    ///
    /// Opportunistically evicts entries past `2 × MAX_HEARTBEAT_CLOCK_DRIFT_MS`
    /// for *this namespace* on every insert — keeps long-lived nodes from
    /// accumulating entries from peers that left the namespace.
    /// Stale-but-within-eviction-window entries are still filtered out of
    /// `fresh_peers` by the per-call `ttl` check.
    pub fn insert(&self, hb: &SignedMigrationHeartbeat) {
        // Wall-clock sanity bound — reject far-future ts_millis.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if hb.ts_millis > now_ms.saturating_add(MAX_HEARTBEAT_CLOCK_DRIFT_MS) {
            return;
        }

        let now = Instant::now();
        let mut g = self.entries_lock();
        let key = (hb.namespace_id, hb.peer_pubkey);
        if let Some(existing) = g.get(&key) {
            // Drop the heartbeat if it's older or equal-clock-but-not-fresher.
            if hb.ts_millis < existing.ts_millis
                || (hb.ts_millis == existing.ts_millis
                    && hb.synced_up_to_hlc <= existing.synced_up_to_hlc)
            {
                return;
            }
        }

        // Opportunistic eviction for the same namespace — keep the BTreeMap
        // from accumulating entries from peers that left. Eviction window
        // (`2 × MAX_HEARTBEAT_CLOCK_DRIFT_MS`) is intentionally wider than
        // typical TTLs so reads can still see "stale-but-recent" entries
        // (filtered by per-call `ttl`) without competing against this prune.
        let evict_window = Duration::from_millis(MAX_HEARTBEAT_CLOCK_DRIFT_MS.saturating_mul(2));
        g.retain(|(ns, _), entry| {
            *ns != hb.namespace_id || now.duration_since(entry.received_at) <= evict_window
        });

        let _ = g.insert(
            key,
            CacheEntry {
                schema_version: hb.schema_version,
                residue_auto: hb.residue_auto,
                residue_identity: hb.residue_identity,
                synced_up_to_hlc: hb.synced_up_to_hlc,
                authored_remaining: hb.authored_remaining,
                migration_failed: hb.migration_failed,
                ts_millis: hb.ts_millis,
                received_at: now,
            },
        );
    }

    /// All peers in `ns` whose most recent heartbeat is fresh within `ttl`.
    ///
    /// Stale entries (past `ttl`) are filtered out — the rollup treats a
    /// member with no fresh entry here as `unknown`, never as migrated.
    pub fn fresh_peers(&self, ns: [u8; 32], ttl: Duration) -> Vec<(PublicKey, CacheEntry)> {
        let g = self.entries_lock();
        let now = Instant::now();
        g.iter()
            .filter(|((nns, _), e)| *nns == ns && now.duration_since(e.received_at) <= ttl)
            .map(|((_, pk), e)| (*pk, e.clone()))
            .collect()
    }

    /// Snapshot the freshest in-TTL heartbeats for `ns` into the
    /// `BTreeMap<PublicKey, MigrationStatusReport>` the rollup consumes.
    ///
    /// This is the projection the admin route (Task 6c.10) threads into
    /// `GetMigrationStatusRequest::member_reports`: each fresh [`CacheEntry`]
    /// maps 1:1 to a [`MigrationStatusReport`] DTO (the peer's `ts_millis`
    /// becomes `reported_at`). A member absent from this map resolves to
    /// `unknown` in the rollup, never a false green. Stale entries past `ttl`
    /// are filtered out by [`fresh_peers`](Self::fresh_peers).
    pub fn migration_status_reports(
        &self,
        ns: [u8; 32],
        ttl: Duration,
    ) -> BTreeMap<PublicKey, MigrationStatusReport> {
        self.fresh_peers(ns, ttl)
            .into_iter()
            .map(|(pk, e)| (pk, cache_entry_to_report(&e)))
            .collect()
    }

    /// The freshest heartbeat for a specific `(ns, peer)`, or `None` if the
    /// peer has no entry or its entry is stale past `ttl`.
    ///
    /// Used by the rollup (Task 6c.9) to look up each pinned-cohort member's
    /// reported status. A `None` here maps to `unknown` and keeps
    /// `all_migrated == false`.
    pub fn peer_entry(&self, ns: [u8; 32], peer: PublicKey, ttl: Duration) -> Option<CacheEntry> {
        let g = self.entries_lock();
        let now = Instant::now();
        g.get(&(ns, peer))
            .filter(|e| now.duration_since(e.received_at) <= ttl)
            .cloned()
    }
}

/// The per-namespace migration facts this node advertises in its own
/// heartbeat. Computed locally at emit time and signed into a
/// [`SignedMigrationHeartbeat`] body.
///
/// `residue_identity` is this node's local-derived count of unconverted
/// identity-gated entries (Task 6c.6); `residue_auto` is the matching count
/// for Convergent contexts (the 6a marker). A node reporting both at 0 with
/// `schema_version >= target` is what a rollup reads as "this member migrated".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationFacts {
    pub schema_version: u32,
    pub residue_auto: u64,
    pub residue_identity: u64,
    pub synced_up_to_hlc: u64,
    /// Sum across this node's namespace contexts of each context's owner's
    /// identity-gated entries still below target (the node-local count the
    /// context handler persists into `ContextMeta.authored_remaining`). u64
    /// like the residue fields (a per-namespace sum of per-context u32 counts).
    /// Best-effort self-report — distinct from `residue_identity` (still 0).
    pub authored_remaining: u64,
    /// Set when one of this namespace's contexts has a migration-failure marker
    /// persisted (migration-check aborted or apply errored) AND that context is
    /// still below target — so a recovered context never reports stale failure.
    /// `None` = no failure on record. Self-report; categorized, not a gate.
    pub migration_failed: Option<MigrationFailureKind>,
}

/// Decide whether the local node should emit an *on-change* heartbeat for a
/// namespace, given the facts carried by the last heartbeat it emitted (if
/// any) and the freshly-computed facts.
///
/// Mirrors the readiness "edge-trigger on tier transition" pattern: a peer
/// should re-advertise immediately when its *reported state* changes —
/// here when `schema_version`, `residue_auto`, `residue_identity`,
/// `authored_remaining`, or `migration_failed` flips — rather than waiting up
/// to a full periodic interval. `synced_up_to_hlc`
/// is intentionally NOT an edge trigger: it advances on every applied op and
/// would defeat the purpose of debouncing to the periodic tick; the periodic
/// beat carries its latest value.
#[must_use]
pub fn should_emit_on_change(last: Option<MigrationFacts>, current: MigrationFacts) -> bool {
    match last {
        None => true,
        Some(prev) => {
            prev.schema_version != current.schema_version
                || prev.residue_auto != current.residue_auto
                || prev.residue_identity != current.residue_identity
                || prev.authored_remaining != current.authored_remaining
                || prev.migration_failed != current.migration_failed
        }
    }
}

/// Fold a freshly-computed `facts` for `namespace_id` into the per-namespace
/// `last_emitted` carry-forward map and report whether this is an *edge* that
/// warrants an immediate (out-of-cycle) emit.
///
/// Pure (no store / network): the actor handler calls this AFTER overlaying the
/// real `synced_up_to_hlc` (`refresh_hlc`) and BEFORE publishing. It always
/// records the latest facts — seeding the namespace into `last_emitted` on
/// first sight — so the periodic keep-alive tick has a non-empty working set
/// once the node has signalled at least one namespace (the seam that makes the
/// emitter live in production; before this the map could only ever stay empty).
/// The returned bool is the [`should_emit_on_change`] edge decision computed
/// against the *prior* recorded facts.
pub fn record_facts_update(
    last_emitted: &mut HashMap<[u8; 32], MigrationFacts>,
    namespace_id: [u8; 32],
    facts: MigrationFacts,
) -> bool {
    let prior = last_emitted.get(&namespace_id).copied();
    let emit = should_emit_on_change(prior, facts);
    let _ = last_emitted.insert(namespace_id, facts);
    emit
}

/// Parse the leading integer of a semver string ("2", "2.0.0") into the `u32`
/// schema-version a heartbeat reports. Best-effort: a non-numeric leading
/// component yields `None`. Mirrors `get_migration_status::parse_schema_version`.
fn parse_major_version(version: &str) -> Option<u32> {
    version
        .split('.')
        .next()
        .and_then(|major| major.trim().parse::<u32>().ok())
}

/// Derive the migration TARGET version for `group_id` from the local upgrade
/// record. `None` record ⇒ baseline `0` (nothing in flight). A record whose
/// `to_version` does not parse pins to [`u32::MAX`] so no real loaded version
/// can satisfy it (no false green) — the same rule the `get_migration_status`
/// rollup applies via `derive_target_version`.
fn derive_target_version(
    datastore: &Store,
    group_id: &calimero_context_config::types::ContextGroupId,
) -> u32 {
    match calimero_context::group_store::UpgradesRepository::new(datastore)
        .load(group_id)
        .ok()
        .flatten()
    {
        None => 0,
        Some(record) => parse_major_version(&record.to_version).unwrap_or(u32::MAX),
    }
}

/// Resolve a single context's LOADED reader version: the major version of the
/// `ApplicationMeta` its `ContextMeta.application` points at. This is the schema
/// the node can actually read *right now* (the binary it has swapped to), NOT
/// the replicated migration target. `None` when the context/application row is
/// missing or its version string does not parse.
fn loaded_context_version(
    datastore: &Store,
    context_id: &calimero_primitives::context::ContextId,
) -> Option<u32> {
    let handle = datastore.handle();
    let ctx_meta = handle
        .get(&calimero_store::key::ContextMeta::new(*context_id))
        .ok()
        .flatten()?;
    let app_meta = handle.get(&ctx_meta.application).ok().flatten()?;
    parse_major_version(&app_meta.version)
}

/// Enumerate every context under a namespace's full subgroup tree: the
/// namespace-root group's direct contexts UNIONed with the contexts of every
/// descendant subgroup. `NamespaceRepository::collect_descendants` does the
/// depth-bounded, cycle-guarded DOWN-direction walk over the `GroupChildIndex`
/// (the down-counterpart of `enumerate_inherited`'s up-walk), so a context a
/// node joined via an Open subgroup is no longer invisible to the facts builder.
/// Read-only and best-effort: a per-group enumeration error degrades to skipping
/// that group rather than dropping the whole namespace's facts.
fn enumerate_namespace_tree_contexts(
    datastore: &Store,
    group_id: &calimero_context_config::types::ContextGroupId,
) -> Vec<calimero_primitives::context::ContextId> {
    let mut groups = vec![*group_id];
    groups.extend(
        calimero_context::group_store::NamespaceRepository::new(datastore)
            .collect_descendants(group_id)
            .unwrap_or_default(),
    );

    let mut contexts = Vec::new();
    for gid in &groups {
        contexts.extend(
            calimero_context::group_store::enumerate_group_contexts(datastore, gid, 0, usize::MAX)
                .unwrap_or_default(),
        );
    }
    contexts
}

/// Compute this node's locally-advertised migration facts for `namespace_id`.
///
/// `schema_version` is the node's actually-LOADED reader version — the lowest
/// loaded `ApplicationMeta` major version across the namespace's contexts (the
/// most-behind context governs whether this node has fully swapped its binary).
/// This is deliberately NOT the migration TARGET (`UpgradesRepository.to_version`):
/// under LazyOnAccess the governance target advances ahead of the locally-loaded
/// binary, so reporting the target would let `all_migrated` flip green before the
/// node could read the new schema (the cursor-bot bug this fixes). With no
/// resolvable context we fall back to the target — the honest "no loaded state to
/// contradict the record" path (covered by the no-record baseline `0`).
///
/// `synced_up_to_hlc` is left at `0` here; the emitter overlays the live
/// `NamespaceGovHead.sequence` ([`MigrationEmitter::refresh_hlc`]) at publish
/// time so the periodic and on-change beats always carry the freshest position.
///
/// `residue_auto` is the count of the namespace's contexts whose loaded version
/// still trails the target — each pending whole-root (Convergent/Replayable)
/// rebuild. A context is atomically v1-or-v2 (the PR-6a/6b whole-root path), so
/// "loaded < target" is exactly its outstanding auto-residue.
///
/// `residue_identity` is computed by INVOKING the 6c.6 residue scan
/// ([`residue_identity_count`] → [`count_unconverted_identity_gated`]). At this
/// production seam the scan runs over [`CommittedStateScan`], an honest
/// empty-keyspace [`IterableStorage`] binding: real context state lives in the
/// wasm `MainStorage`, whose host exposes no committed-state key-iteration (the
/// `MainStorage` `IterableStorage` impl is intentionally absent — see
/// `calimero_storage::store`), so the scan completes over zero keys and reports
/// the conservative `0`. That `0` is safe because any not-yet-swapped context is
/// already surfaced by `schema_version < target` + `residue_auto`, which keep
/// `all_migrated` false (the cohort's `unknown`-safety covers silent members).
/// The scan path is exercised end-to-end against an iterable adaptor by
/// `residue_identity_count_invokes_the_scan`, so swapping `CommittedStateScan`
/// for a key-iterating committed-state adaptor is the only change needed to
/// begin reporting true per-context residue.
///
/// [`IterableStorage`]: calimero_storage::store::IterableStorage
/// [`count_unconverted_identity_gated`]: calimero_storage::index::Index::count_unconverted_identity_gated
#[must_use]
pub fn compute_namespace_migration_facts(
    datastore: &Store,
    namespace_id: [u8; 32],
) -> MigrationFacts {
    let group_id = calimero_context_config::types::ContextGroupId::from(namespace_id);
    let target_version = derive_target_version(datastore, &group_id);

    // Every context under the namespace, INCLUDING those in descendant
    // subgroups (a context joined via an Open subgroup is not a direct child of
    // the namespace-root group, so `enumerate_group_contexts(group_id)` alone
    // would miss it — leaving a stranded subgroup context's failure/loaded
    // state silently unreported). A context whose loaded reader version trails
    // the target is an unconverted whole-root (residue_auto); the lowest loaded
    // version across them is the node's honest advertised schema_version.
    let contexts = enumerate_namespace_tree_contexts(datastore, &group_id);

    let mut min_loaded: Option<u32> = None;
    let mut residue_auto: u64 = 0;
    // Sum the per-context authored_remaining the context handler persisted to
    // the dedicated node-local key (6f.8). A plain store read — no wasm, no
    // committed-state iteration. Absent row ⇒ 0.
    let mut authored_remaining: u64 = 0;
    // Most-severe persisted migration-failure across the namespace's contexts,
    // honored only for contexts still below target (self-healing on recovery).
    let mut migration_failed: Option<MigrationFailureKind> = None;
    for context_id in &contexts {
        if let Ok(Some(entry)) =
            datastore
                .handle()
                .get(&calimero_store::key::ContextAuthoredRemaining::new(
                    *context_id,
                ))
        {
            authored_remaining = authored_remaining.saturating_add(u64::from(entry.count));
        }

        let loaded = loaded_context_version(datastore, context_id);

        // A persisted failure marker is honored only while the context has NOT
        // reached target. A context that has since converged (via a later
        // migrate, lazy access, or cascade) is migrated — a stale marker must
        // never force a false `failed`. An unresolvable version (None) is
        // conservatively treated as below-target so a genuine failure surfaces.
        let below_target = loaded.is_none_or(|l| l < target_version);
        if below_target {
            if let Ok(Some(marker)) =
                datastore
                    .handle()
                    .get(&calimero_store::key::ContextMigrationFailed::new(
                        *context_id,
                    ))
            {
                if let Some(kind) = MigrationFailureKind::from_u8(marker.kind) {
                    migration_failed = Some(more_severe_failure(migration_failed, kind));
                }
            }
        }

        let Some(loaded) = loaded else {
            continue;
        };
        min_loaded = Some(min_loaded.map_or(loaded, |m| m.min(loaded)));
        if loaded < target_version {
            residue_auto += 1;
        }
    }

    // No resolvable loaded context ⇒ fall back to the target (the no-loaded-state
    // path; the no-record baseline already resolves the target to `0`). But never
    // advertise the unparseable-target sentinel (`u32::MAX`) as a LOADED reader
    // version — it is an all_migrated-gate marker ("no real version satisfies an
    // unparseable target"), not a version this node loaded. Report the honest
    // unknown `0` instead; the gate stays conservative (0 < MAX ⇒ not migrated).
    let schema_version = match min_loaded {
        Some(loaded) => loaded,
        None if target_version == u32::MAX => 0,
        None => target_version,
    };

    // Invoke the 6c.6 residue scan over the iterable adaptor bound at this seam.
    // The production binding is `CommittedStateScan` — an honest empty-keyspace
    // adaptor — because the wasm host exposes no committed-state key-iteration
    // yet, so the scan resolves to the conservative `0`. The scan call is real
    // (exercised against `MockedStorage` in tests) and ready to count true
    // residue the moment the node binds a key-iterating committed-state adaptor.
    let residue_identity = residue_identity_count::<CommittedStateScan>(target_version);

    MigrationFacts {
        schema_version,
        residue_auto,
        residue_identity,
        synced_up_to_hlc: 0,
        authored_remaining,
        migration_failed,
    }
}

/// Pick the more severe of an accumulated failure and a freshly-read one:
/// `ApplyFailed` outranks `CheckAborted` (higher discriminant wins). Used to
/// fold a namespace's per-context failure markers into one reported reason.
fn more_severe_failure(
    acc: Option<MigrationFailureKind>,
    next: MigrationFailureKind,
) -> MigrationFailureKind {
    match acc {
        Some(prev) if prev.to_u8() >= next.to_u8() => prev,
        _ => next,
    }
}

/// Invoke the 6c.6 residue scan ([`count_unconverted_identity_gated`]) over the
/// iterable context-storage adaptor `S` and return the count of identity-gated
/// entries still trailing `target_version` (the node's local-derived
/// `residue_identity` telemetry). A scan error degrades to `0` — residue is
/// observability only and must never block on a transient read failure; the
/// `schema_version < target` + `residue_auto` signals keep the rollup
/// conservative regardless.
///
/// This is the single seam through which the heartbeat facts reach the residue
/// scan. [`compute_namespace_migration_facts`] calls it with the production
/// [`CommittedStateScan`] adaptor (empty keyspace ⇒ `0` until the host exposes
/// committed-state iteration), while the unit tests drive it with
/// `MockedStorage` to prove the scan path is wired end-to-end.
///
/// [`count_unconverted_identity_gated`]: calimero_storage::index::Index::count_unconverted_identity_gated
#[must_use]
fn residue_identity_count<S>(target_version: u32) -> u64
where
    S: calimero_storage::store::IterableStorage,
{
    calimero_storage::index::Index::<S>::count_unconverted_identity_gated(target_version)
        .map_or(0, |count| count as u64)
}

/// The production [`IterableStorage`] binding for [`residue_identity_count`].
///
/// The node has no committed-state key-iteration at the heartbeat-facts seam:
/// real context state lives in the wasm `MainStorage`, whose host exposes no
/// `storage_iter_keys` (see `calimero_storage::store` — the `MainStorage`
/// `IterableStorage` impl is intentionally absent). Rather than special-casing
/// the facts builder to skip the scan, we bind this honest empty-keyspace
/// adaptor: it implements [`IterableStorage`] with zero keys, so the 6c.6 scan
/// runs to completion and reports the conservative `0`. When a key-iterating
/// committed-state adaptor lands, swap this binding for it and the facts begin
/// reporting true per-context `residue_identity` with no other change.
///
/// [`IterableStorage`]: calimero_storage::store::IterableStorage
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CommittedStateScan;

impl calimero_storage::store::StorageAdaptor for CommittedStateScan {
    fn storage_read(_key: calimero_storage::store::Key) -> Option<Vec<u8>> {
        None
    }

    fn storage_remove(_key: calimero_storage::store::Key) -> bool {
        false
    }

    fn storage_write(_key: calimero_storage::store::Key, _value: &[u8]) -> bool {
        false
    }
}

impl calimero_storage::store::IterableStorage for CommittedStateScan {
    fn storage_iter_keys() -> Vec<calimero_storage::store::Key> {
        Vec::new()
    }
}

/// Build and sign a [`SignedMigrationHeartbeat`] for `namespace_id` over the
/// canonical `MIGRATION_HEARTBEAT_SIGN_DOMAIN || borsh(body)` payload.
///
/// Pure: takes the signing identity and the locally-computed facts and
/// returns the signed wire message. The actor's publish path calls this then
/// hands the bytes to gossipsub; receivers verify via
/// `SignedMigrationHeartbeat::verify_signature` + namespace membership
/// (`verify_migration_heartbeat`). `ts_millis` is the signer's wall-clock at
/// build time — the per-peer ordering signal the cache uses to drop
/// out-of-order re-deliveries.
pub fn build_signed_heartbeat(
    signer_sk: &calimero_primitives::identity::PrivateKey,
    namespace_id: [u8; 32],
    facts: MigrationFacts,
    ts_millis: u64,
) -> Result<SignedMigrationHeartbeat, calimero_context_client::local_governance::GovernanceError> {
    let peer_pubkey = signer_sk.public_key();
    let mut hb = SignedMigrationHeartbeat {
        namespace_id,
        peer_pubkey,
        schema_version: facts.schema_version,
        residue_auto: facts.residue_auto,
        residue_identity: facts.residue_identity,
        synced_up_to_hlc: facts.synced_up_to_hlc,
        authored_remaining: facts.authored_remaining,
        migration_failed: facts
            .migration_failed
            .map_or(0, MigrationFailureKind::to_u8),
        ts_millis,
        signature: [0u8; 64],
    };
    let signable = hb.signable_bytes()?;
    let signature = signer_sk.sign(&signable)?.to_bytes();
    hb.signature = signature;
    Ok(hb)
}

/// Default low-frequency interval at which the emitter re-publishes each
/// namespace's heartbeat even when nothing changed, so a member that joined
/// the topic late still learns our status within one period. Edge-trigger
/// emits (residue / schema change) fire out-of-band; this is the keep-alive
/// floor. 30s — well under the 60s [`DEFAULT_HEARTBEAT_TTL`] so a steady
/// peer never lapses to `unknown` between beats.
pub const DEFAULT_EMIT_INTERVAL: Duration = Duration::from_secs(30);

/// Per-namespace migration-heartbeat emitter.
///
/// The publish twin of the ingest [`MigrationStatusCache`]: signs and
/// publishes this node's own [`SignedMigrationHeartbeat`] on the namespace
/// topic, both periodically (keep-alive) and on-change (residue / schema
/// flips). Modeled on [`crate::readiness::ReadinessManager`]'s beacon
/// emission — best-effort gossip, never blocking, never a gate.
///
/// Facts are computed at emit time from local state. `synced_up_to_hlc`
/// reads the namespace governance head sequence (the same source readiness
/// uses for `applied_through`); `schema_version`, `residue_auto`, and
/// `residue_identity` are supplied by the node when it triggers an emit and
/// are otherwise carried forward from the last emit (see
/// [`MigrationFactsUpdate`]). A node that has not yet computed residue
/// reports `0` — the honest "nothing pending locally" telemetry.
pub struct MigrationEmitter {
    pub node_client: NodeClient,
    pub datastore: Store,
    pub interval: Duration,
    /// Last facts we emitted per namespace — the on-change reference for
    /// [`should_emit_on_change`] and the carry-forward source for the
    /// periodic tick.
    pub last_emitted: HashMap<[u8; 32], MigrationFacts>,
}

impl Actor for MigrationEmitter {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // Periodic keep-alive re-publish for every namespace we have facts for.
        // RECOMPUTE facts from the store each tick (not a replay of the cached
        // last_emitted), so node-local fact changes that don't fire a
        // MigrationFactsUpdate — notably authored_remaining persisted after
        // migrate_my_entries / a lazy migrate (6f) — self-heal within one
        // interval instead of staying stale until an unrelated governance op.
        // Edge-trigger emits via MigrationFactsUpdate still give immediate
        // updates on governance applies.
        ctx.run_interval(self.interval, |this, _ctx| {
            let ns_ids: Vec<[u8; 32]> = this.last_emitted.keys().copied().collect();
            for ns_id in ns_ids {
                let facts = compute_namespace_migration_facts(&this.datastore, ns_id);
                let facts = this.refresh_hlc(ns_id, facts);
                this.last_emitted.insert(ns_id, facts);
                this.publish_heartbeat(ns_id, facts);
            }
        });
    }
}

/// Tells the emitter the node's freshly-computed migration facts for a
/// namespace. Emits immediately iff [`should_emit_on_change`] reports the
/// reported state changed (residue / schema flip); otherwise the periodic
/// tick carries the value. Sent by the node whenever a governance apply or
/// owner-driven convert may have changed local residue.
#[derive(Message)]
#[rtype(result = "()")]
pub struct MigrationFactsUpdate {
    pub namespace_id: [u8; 32],
    pub facts: MigrationFacts,
}

impl Handler<MigrationFactsUpdate> for MigrationEmitter {
    type Result = ();

    fn handle(&mut self, msg: MigrationFactsUpdate, _ctx: &mut Self::Context) {
        let facts = self.refresh_hlc(msg.namespace_id, msg.facts);
        let last = self.last_emitted.get(&msg.namespace_id).copied();
        if should_emit_on_change(last, facts) {
            self.last_emitted.insert(msg.namespace_id, facts);
            self.publish_heartbeat(msg.namespace_id, facts);
        } else {
            // No edge — still record the carry-forward value so the next
            // periodic beat advertises the latest HLC.
            self.last_emitted.insert(msg.namespace_id, facts);
        }
    }
}

impl MigrationEmitter {
    /// Overlay the namespace governance head's current sequence onto `facts`
    /// as `synced_up_to_hlc`. Read-only; defaults to the incoming value on a
    /// missing head or store error so a transient read never regresses the
    /// reported HLC.
    fn refresh_hlc(&self, ns_id: [u8; 32], mut facts: MigrationFacts) -> MigrationFacts {
        let handle = self.datastore.handle();
        let key = calimero_store::key::NamespaceGovHead::new(ns_id);
        if let Ok(Some(head)) = handle.get(&key) {
            facts.synced_up_to_hlc = head.sequence;
        }
        facts
    }

    /// Sign and publish a [`SignedMigrationHeartbeat`] on the namespace topic.
    ///
    /// Best-effort, mirroring [`crate::readiness::ReadinessManager::publish_beacon`]:
    /// any error (no identity yet, signing failure, no peers subscribed) logs
    /// at debug and returns. The periodic tick retries; an edge-trigger fires
    /// on the next residue/schema change.
    fn publish_heartbeat(&self, ns_id: [u8; 32], facts: MigrationFacts) {
        let group_id = calimero_context_config::types::ContextGroupId::from(ns_id);
        let identity = match NamespaceRepository::new(&self.datastore).identity(&group_id) {
            Ok(Some(id)) => id,
            Ok(None) => return, // No identity for this namespace yet — skip.
            Err(err) => {
                tracing::debug!(?err, ?ns_id, "MigrationHeartbeat: identity load failed");
                return;
            }
        };
        let (_peer_pubkey, mut sk_bytes, mut sender_key) = identity;
        // `sender_key` is unused on this path — zeroize immediately. `sk_bytes`
        // is consumed into `PrivateKey::from(...)`; because `[u8; 32]: Copy`
        // that move leaves a stack copy we zeroize after signing.
        sender_key.zeroize();

        let ts_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let signing_key = calimero_primitives::identity::PrivateKey::from(sk_bytes);
        sk_bytes.zeroize();
        let heartbeat = match build_signed_heartbeat(&signing_key, ns_id, facts, ts_millis) {
            Ok(hb) => hb,
            Err(err) => {
                tracing::debug!(?err, "MigrationHeartbeat: sign failed");
                return;
            }
        };

        let topic = calimero_context::governance_broadcast::ns_topic(ns_id);
        // Wrap in the `BroadcastMessage::NamespaceGovernanceDelta` envelope
        // the receiver decodes on `ns/<id>` topics, then borsh the inner
        // `NamespaceTopicMsg::MigrationHeartbeat`. delta_id/parent_ids are
        // zero/empty — heartbeats are not DAG content.
        let inner = match borsh::to_vec(&NamespaceTopicMsg::MigrationHeartbeat(heartbeat)) {
            Ok(b) => b,
            Err(err) => {
                tracing::debug!(?err, "MigrationHeartbeat: borsh encode (inner) failed");
                return;
            }
        };
        let envelope = BroadcastMessage::NamespaceGovernanceDelta {
            namespace_id: ns_id,
            delta_id: [0u8; 32],
            parent_ids: Vec::new(),
            payload: inner,
        };
        let bytes = match borsh::to_vec(&envelope) {
            Ok(b) => b,
            Err(err) => {
                tracing::debug!(?err, "MigrationHeartbeat: borsh encode (envelope) failed");
                return;
            }
        };

        // Detached publish — bypasses the 10s mesh-wait gate of
        // `publish_on_namespace`; heartbeat emission must not block.
        let net = self.node_client.network_client().clone();
        let log_ns = ns_id;
        let log_schema = facts.schema_version;
        let log_residue = facts.residue_identity;
        let log_authored = facts.authored_remaining;
        actix::spawn(async move {
            match net.publish(topic, bytes).await {
                Ok(_) => tracing::debug!(
                    namespace_id = %hex::encode(log_ns),
                    schema_version = log_schema,
                    residue_identity = log_residue,
                    authored_remaining = log_authored,
                    "migration heartbeat emitted"
                ),
                Err(err) => {
                    tracing::debug!(?err, "MigrationHeartbeat publish failed (non-fatal)");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::wire::{
        SignableMigrationHeartbeat, MIGRATION_HEARTBEAT_SIGN_DOMAIN,
    };
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    const NS: [u8; 32] = [42u8; 32];

    /// Build a heartbeat signed by `sk` over the canonical signable bytes.
    fn signed_hb(
        sk: &PrivateKey,
        ns: [u8; 32],
        schema_version: u32,
        residue_auto: u64,
        residue_identity: u64,
        ts_millis: u64,
    ) -> SignedMigrationHeartbeat {
        let peer_pubkey = sk.public_key();
        let body = SignableMigrationHeartbeat {
            namespace_id: ns,
            peer_pubkey,
            schema_version,
            residue_auto,
            residue_identity,
            synced_up_to_hlc: 0,
            ts_millis,
        };
        let mut signable = Vec::new();
        signable.extend_from_slice(MIGRATION_HEARTBEAT_SIGN_DOMAIN);
        signable.extend_from_slice(&borsh::to_vec(&body).unwrap());
        let signature = sk.sign(&signable).unwrap().to_bytes();
        SignedMigrationHeartbeat {
            namespace_id: ns,
            peer_pubkey,
            schema_version,
            residue_auto,
            residue_identity,
            synced_up_to_hlc: 0,
            ts_millis,
            signature,
            authored_remaining: 0,
            migration_failed: 0,
        }
    }

    #[test]
    fn verified_heartbeat_is_cached_and_readable() {
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let hb = signed_hb(&sk, NS, 2, 0, 0, 0);
        // Caller verifies first; a well-formed heartbeat verifies.
        assert!(hb.verify_signature().is_ok());
        cache.insert(&hb);

        let fresh = cache.fresh_peers(NS, DEFAULT_HEARTBEAT_TTL);
        assert_eq!(fresh.len(), 1, "verified heartbeat must be cached");
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .expect("entry readable by (ns, peer)");
        assert_eq!(entry.schema_version, 2);
        assert_eq!(entry.residue_identity, 0);
    }

    #[test]
    fn migration_status_reports_projects_fresh_entries() {
        // The admin route (Task 6c.10) snapshots the cache into the
        // `BTreeMap<PublicKey, MemberMigrationReport>` the rollup consumes.
        // Each fresh entry projects 1:1; `ts_millis` becomes `reported_at`.
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let hb = signed_hb(&sk, NS, 2, 1, 3, 7);
        cache.insert(&hb);

        let reports = cache.migration_status_reports(NS, DEFAULT_HEARTBEAT_TTL);
        assert_eq!(reports.len(), 1, "fresh entry must project into a report");
        let report = reports
            .get(&sk.public_key())
            .copied()
            .expect("report keyed by peer pubkey");
        assert_eq!(report.schema_version, 2);
        assert_eq!(report.residue_auto, 1);
        assert_eq!(report.residue_identity, 3);
        assert_eq!(report.reported_at, 7, "ts_millis projects to reported_at");
    }

    #[test]
    fn migration_status_reports_excludes_other_namespaces() {
        // Only the requested namespace's peers project — a heartbeat from a
        // different namespace must not leak into the rollup snapshot.
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let other_ns = [7u8; 32];
        cache.insert(&signed_hb(&sk, other_ns, 2, 0, 0, 0));

        let reports = cache.migration_status_reports(NS, DEFAULT_HEARTBEAT_TTL);
        assert!(
            reports.is_empty(),
            "an other-namespace heartbeat must not appear in this namespace's reports"
        );
    }

    #[test]
    fn wire_verify_signature_rejects_field_substitution() {
        // Documents the WIRE-TYPE contract `insert`'s verification-precondition
        // depends on: `SignedMigrationHeartbeat::verify_signature` covers every
        // signed field, so flipping `residue_identity` after signing breaks it.
        //
        // NOTE: this is NOT the ingest gate. The actual receiver gate is
        // `calimero_context::governance_broadcast::verify_migration_heartbeat`
        // (signature + cohort membership), exercised end-to-end in that crate's
        // `verify_migration_heartbeat_rejects_bad_signature` test — it needs a
        // `&Store` and namespace member-set, neither of which this pure cache
        // unit owns. Here we only pin that the wire type's own signature check
        // (the primitive the gate is built on) catches a mutated field.
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut hb = signed_hb(&sk, NS, 2, 0, 5, 0);
        assert!(hb.verify_signature().is_ok());
        hb.residue_identity = 0; // tampered after signing
        assert!(
            hb.verify_signature().is_err(),
            "verify_signature must reject a mutated residue_identity"
        );
    }

    #[test]
    fn stale_entry_is_filtered_after_ttl() {
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        cache.insert(&signed_hb(&sk, NS, 2, 0, 0, 0));
        // Drive the TTL via a very small per-call window — the same seam the
        // readiness tests use (`pick_sync_partner_excludes_stale_entries`).
        std::thread::sleep(Duration::from_millis(10));
        assert!(
            cache.fresh_peers(NS, Duration::from_millis(5)).is_empty(),
            "entry past TTL must not be fresh"
        );
        assert!(
            cache
                .peer_entry(NS, sk.public_key(), Duration::from_millis(5))
                .is_none(),
            "stale (ns, peer) lookup must report unknown (None)"
        );
    }

    #[test]
    fn insert_drops_stale_heartbeat_from_same_peer() {
        // Gossipsub out-of-order delivery must not stale-overwrite a fresher
        // entry: the fresher heartbeat's fields remain after the older one
        // arrives second.
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut fresh = signed_hb(&sk, NS, 2, 0, 0, 2000);
        fresh.residue_identity = 0;
        let mut stale = signed_hb(&sk, NS, 1, 0, 9, 1000);
        stale.residue_identity = 9;
        cache.insert(&fresh);
        cache.insert(&stale); // arrives second but is older — must be dropped
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .unwrap();
        assert_eq!(
            entry.schema_version, 2,
            "stale heartbeat must not overwrite fresher entry from same peer"
        );
        assert_eq!(entry.residue_identity, 0);
    }

    #[test]
    fn insert_accepts_newer_heartbeat_from_same_peer() {
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let older = signed_hb(&sk, NS, 1, 0, 9, 1000);
        let newer = signed_hb(&sk, NS, 2, 0, 0, 2000);
        cache.insert(&older);
        cache.insert(&newer);
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .unwrap();
        assert_eq!(
            entry.schema_version, 2,
            "newer heartbeat must replace older"
        );
        assert_eq!(entry.residue_identity, 0);
    }

    #[test]
    fn insert_rejects_far_future_ts_millis() {
        // Cache-poisoning regression: a far-future ts_millis would otherwise
        // freeze the entry, dropping every later legitimate heartbeat as
        // "older".
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let poison = signed_hb(&sk, NS, 9, 0, 0, now_ms + 600_000);
        cache.insert(&poison);
        assert!(
            cache.fresh_peers(NS, DEFAULT_HEARTBEAT_TTL).is_empty(),
            "far-future heartbeat must be rejected to prevent cache poisoning"
        );
        let legit = signed_hb(&sk, NS, 2, 0, 0, now_ms);
        cache.insert(&legit);
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .unwrap();
        assert_eq!(entry.schema_version, 2);
    }

    #[test]
    fn on_change_emit_fires_when_residue_changes() {
        let base = MigrationFacts {
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 4,
            synced_up_to_hlc: 10,
            authored_remaining: 0,
            migration_failed: None,
        };
        // First-ever emit (no prior) always fires.
        assert!(should_emit_on_change(None, base));
        // residue_identity drops 4 -> 0: on-change must fire.
        let drained = MigrationFacts {
            residue_identity: 0,
            ..base
        };
        assert!(
            should_emit_on_change(Some(base), drained),
            "residue_identity change must edge-trigger an emit"
        );
        // residue_auto change must also fire.
        let auto_changed = MigrationFacts {
            residue_auto: 1,
            ..base
        };
        assert!(should_emit_on_change(Some(base), auto_changed));
        // schema_version change must fire.
        let bumped = MigrationFacts {
            schema_version: 3,
            ..base
        };
        assert!(should_emit_on_change(Some(base), bumped));
        // A migration failure appearing must fire so the failed state propagates
        // immediately rather than waiting a full periodic interval.
        let failed = MigrationFacts {
            migration_failed: Some(MigrationFailureKind::CheckAborted),
            ..base
        };
        assert!(
            should_emit_on_change(Some(base), failed),
            "a migration failure appearing must edge-trigger an emit"
        );
    }

    #[test]
    fn on_change_emit_suppressed_when_only_hlc_advances() {
        // synced_up_to_hlc advances on every applied op; it must NOT edge-
        // trigger (the periodic beat carries its latest value), otherwise
        // the debounce-to-periodic intent is defeated.
        let prev = MigrationFacts {
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 0,
            synced_up_to_hlc: 10,
            authored_remaining: 0,
            migration_failed: None,
        };
        let advanced = MigrationFacts {
            synced_up_to_hlc: 99,
            ..prev
        };
        assert!(
            !should_emit_on_change(Some(prev), advanced),
            "an HLC-only advance must not edge-trigger an emit"
        );
    }

    #[test]
    fn record_facts_update_seeds_namespace_and_reports_edge() {
        // The production sender (governance-apply path) feeds the emitter via
        // `MigrationFactsUpdate`, whose handler calls `record_facts_update`.
        // Before any update the carry-forward map is empty, so the periodic
        // keep-alive tick has nothing to publish; the first update MUST seed it
        // (and report an edge, since there is no prior) so the tick goes live.
        let mut last_emitted: HashMap<[u8; 32], MigrationFacts> = HashMap::new();
        let facts = MigrationFacts {
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 3,
            synced_up_to_hlc: 10,
            authored_remaining: 0,
            migration_failed: None,
        };

        let emit = record_facts_update(&mut last_emitted, NS, facts);
        assert!(emit, "first-ever facts for a namespace must edge-trigger");
        assert_eq!(
            last_emitted.get(&NS).copied(),
            Some(facts),
            "first update must seed last_emitted so the periodic tick has work \
             (the dead-empty-map regression)"
        );

        // An HLC-only advance records the new value but does NOT edge-trigger;
        // the periodic tick carries it.
        let advanced = MigrationFacts {
            synced_up_to_hlc: 99,
            ..facts
        };
        let emit = record_facts_update(&mut last_emitted, NS, advanced);
        assert!(!emit, "HLC-only advance must not edge-trigger");
        assert_eq!(
            last_emitted.get(&NS).copied(),
            Some(advanced),
            "carry-forward value must still update on a non-edge"
        );

        // A residue drop is an edge.
        let drained = MigrationFacts {
            residue_identity: 0,
            ..advanced
        };
        let emit = record_facts_update(&mut last_emitted, NS, drained);
        assert!(emit, "residue drop must edge-trigger");
    }

    #[test]
    fn facts_for_namespace_reads_target_from_upgrade_record() {
        // The on-change driver computes the node's advertised facts from local
        // governance state. With no upgrade record the group is at the baseline
        // (`schema_version == 0`); once a record targets v2 the facts advertise
        // schema_version 2 — the value a rollup compares each member against.
        use calimero_context::group_store::UpgradesRepository;
        use calimero_context_config::types::ContextGroupId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
        use std::sync::Arc;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = [0x55u8; 32];

        // No record yet -> baseline target.
        let facts = compute_namespace_migration_facts(&store, ns);
        assert_eq!(
            facts.schema_version, 0,
            "no upgrade record => baseline schema version 0"
        );
        assert_eq!(facts.residue_auto, 0);
        assert_eq!(facts.residue_identity, 0);

        // Record targeting v2 -> facts advertise 2.
        UpgradesRepository::new(&store)
            .save(
                &ContextGroupId::from(ns),
                &GroupUpgradeValue {
                    from_version: "1".to_owned(),
                    to_version: "2".to_owned(),
                    migration: None,
                    initiated_at: 0,
                    initiated_by: PrivateKey::random(&mut rand::thread_rng()).public_key(),
                    status: GroupUpgradeStatus::Completed { completed_at: None },
                    cascade_hlc: None,
                    cascade_seq: None,
                },
            )
            .unwrap();
        let facts = compute_namespace_migration_facts(&store, ns);
        assert_eq!(
            facts.schema_version, 2,
            "an upgrade record targeting v2 must advertise schema_version 2"
        );
    }

    /// Register a context under `ns` whose locally-loaded `ApplicationMeta`
    /// declares `version`. This is the binary the node has actually swapped to
    /// — the value the facts must report, distinct from the migration target.
    fn install_loaded_context(store: &Store, ns: [u8; 32], ctx: [u8; 32], version: &str) {
        install_loaded_context_in_group(
            store,
            &calimero_context_config::types::ContextGroupId::from(ns),
            ctx,
            version,
        );
    }

    /// Like [`install_loaded_context`] but registers the context under an
    /// arbitrary `group_id` rather than the namespace-root group — used to put
    /// a context in a SUBGROUP so the descendant-tree enumeration is exercised.
    fn install_loaded_context_in_group(
        store: &Store,
        group_id: &calimero_context_config::types::ContextGroupId,
        ctx: [u8; 32],
        version: &str,
    ) {
        use calimero_primitives::application::ApplicationId;
        use calimero_store::key::{
            ApplicationMeta as ApplicationMetaKey, ContextMeta as ContextMetaKey,
        };
        use calimero_store::types::{ApplicationMeta, ContextMeta};

        let app_id = ApplicationId::from(ctx); // distinct per context fixture
        let blob = calimero_store::key::BlobMeta::new(calimero_primitives::blobs::BlobId::from(
            [0x9Au8; 32],
        ));
        let app_meta = ApplicationMeta::new(
            blob,
            1,
            "test://loaded".to_owned().into_boxed_str(),
            Box::new([]),
            blob,
            "loaded-test-pkg".to_owned().into_boxed_str(),
            version.to_owned().into_boxed_str(),
            "loaded-test-signer".to_owned().into_boxed_str(),
        );
        let mut handle = store.handle();
        handle
            .put(&ApplicationMetaKey::new(app_id), &app_meta)
            .expect("put ApplicationMeta");
        handle
            .put(
                &ContextMetaKey::new(ctx.into()),
                &ContextMeta::new(
                    ApplicationMetaKey::new(app_id),
                    [0x01; 32],
                    Vec::new(),
                    None,
                ),
            )
            .expect("put ContextMeta");
        calimero_context::group_store::register_context_in_group(store, group_id, &ctx.into())
            .expect("register context in group");
    }

    /// A node whose LOADED binary still reads schema v1, while the group's
    /// migration record targets v2, MUST report `schema_version == 1` (its
    /// loaded reader version) — NOT the target. Reporting the target before the
    /// binary swaps is the cursor-bot bug: it lets `all_migrated` flip green
    /// while the node still cannot read v2. The not-yet-swapped context also
    /// counts toward `residue_auto` (its whole-root rebuild is still pending).
    #[test]
    fn facts_report_loaded_version_not_target_when_binary_behind() {
        use calimero_context::group_store::UpgradesRepository;
        use calimero_context_config::types::ContextGroupId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
        use std::sync::Arc;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = [0x77u8; 32];

        // The group is migrating to v2.
        UpgradesRepository::new(&store)
            .save(
                &ContextGroupId::from(ns),
                &GroupUpgradeValue {
                    from_version: "1".to_owned(),
                    to_version: "2".to_owned(),
                    migration: None,
                    initiated_at: 0,
                    initiated_by: PrivateKey::random(&mut rand::thread_rng()).public_key(),
                    status: GroupUpgradeStatus::InProgress {
                        total: 1,
                        completed: 0,
                        failed: 0,
                    },
                    cascade_hlc: None,
                    cascade_seq: None,
                },
            )
            .unwrap();

        // ...but this node's loaded binary still reads v1.
        install_loaded_context(&store, ns, [0xC1u8; 32], "1.0.0");

        let facts = compute_namespace_migration_facts(&store, ns);
        assert_eq!(
            facts.schema_version, 1,
            "a node whose loaded binary is behind must report the LOADED (v1) \
             version, not the migration target (v2)"
        );
        assert_eq!(
            facts.residue_auto, 1,
            "a context whose loaded version trails the target is unconverted \
             (residue_auto), keeping all_migrated false"
        );
    }

    /// Once the node's loaded binary has swapped to v2, the facts report v2 and
    /// the previously-pending context drops out of `residue_auto` — the honest
    /// "this member migrated" signal the rollup needs.
    #[test]
    fn facts_report_target_and_zero_residue_once_binary_swapped() {
        use calimero_context::group_store::UpgradesRepository;
        use calimero_context_config::types::ContextGroupId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
        use std::sync::Arc;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = [0x78u8; 32];

        UpgradesRepository::new(&store)
            .save(
                &ContextGroupId::from(ns),
                &GroupUpgradeValue {
                    from_version: "1".to_owned(),
                    to_version: "2".to_owned(),
                    migration: None,
                    initiated_at: 0,
                    initiated_by: PrivateKey::random(&mut rand::thread_rng()).public_key(),
                    status: GroupUpgradeStatus::Completed { completed_at: None },
                    cascade_hlc: None,
                    cascade_seq: None,
                },
            )
            .unwrap();

        // Loaded binary IS at v2.
        install_loaded_context(&store, ns, [0xC2u8; 32], "2.0.0");

        let facts = compute_namespace_migration_facts(&store, ns);
        assert_eq!(facts.schema_version, 2, "loaded binary at v2 reports v2");
        assert_eq!(
            facts.residue_auto, 0,
            "a context at the target version contributes no residue_auto"
        );
    }

    /// A persisted migration-failure marker is reported as `migration_failed`
    /// ONLY while the context is still below the target. A context that has
    /// since reached the target must not surface a stale marker — the facts are
    /// self-healing so a recovered member never produces a false `failed`.
    #[test]
    fn facts_report_failed_marker_only_while_context_below_target() {
        use calimero_context::group_store::UpgradesRepository;
        use calimero_context_config::types::ContextGroupId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
        use std::sync::Arc;

        let save_v2_target = |store: &Store, ns: [u8; 32]| {
            UpgradesRepository::new(store)
                .save(
                    &ContextGroupId::from(ns),
                    &GroupUpgradeValue {
                        from_version: "1".to_owned(),
                        to_version: "2".to_owned(),
                        migration: None,
                        initiated_at: 0,
                        initiated_by: PrivateKey::random(&mut rand::thread_rng()).public_key(),
                        status: GroupUpgradeStatus::InProgress {
                            total: 1,
                            completed: 0,
                            failed: 0,
                        },
                        cascade_hlc: None,
                        cascade_seq: None,
                    },
                )
                .unwrap();
        };
        let set_marker = |store: &Store, ctx: [u8; 32], kind: MigrationFailureKind| {
            let mut handle = store.handle();
            handle
                .put(
                    &calimero_store::key::ContextMigrationFailed::new(ctx.into()),
                    &calimero_store::types::ContextMigrationFailed { kind: kind.to_u8() },
                )
                .expect("put marker");
        };

        // Context still on v1 (below the v2 target) with a check-aborted marker:
        // the facts surface the failure.
        let below = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = [0x79u8; 32];
        save_v2_target(&below, ns);
        install_loaded_context(&below, ns, [0xD1u8; 32], "1.0.0");
        set_marker(&below, [0xD1u8; 32], MigrationFailureKind::CheckAborted);
        assert_eq!(
            compute_namespace_migration_facts(&below, ns).migration_failed,
            Some(MigrationFailureKind::CheckAborted),
            "a failure marker on a still-below-target context must surface as failed"
        );

        // Same marker, but the context has since reached v2: the stale marker is
        // NOT honored (self-healing — a recovered context never reports failed).
        let healed = Store::new(Arc::new(InMemoryDB::owned()));
        save_v2_target(&healed, ns);
        install_loaded_context(&healed, ns, [0xD2u8; 32], "2.0.0");
        set_marker(&healed, [0xD2u8; 32], MigrationFailureKind::CheckAborted);
        assert_eq!(
            compute_namespace_migration_facts(&healed, ns).migration_failed,
            None,
            "a context that reached target must not report a stale failure marker"
        );
    }

    #[test]
    fn built_heartbeat_verifies_and_carries_facts() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let facts = MigrationFacts {
            schema_version: 2,
            residue_auto: 3,
            residue_identity: 1,
            synced_up_to_hlc: 77,
            authored_remaining: 0,
            migration_failed: None,
        };
        let hb = build_signed_heartbeat(&sk, NS, facts, 1234).expect("sign");
        // The receiver's signature gate accepts it, and the cache then reads
        // back exactly the facts we signed.
        assert!(hb.verify_signature().is_ok());
        assert_eq!(hb.peer_pubkey, sk.public_key());
        assert_eq!(hb.schema_version, 2);
        assert_eq!(hb.residue_auto, 3);
        assert_eq!(hb.residue_identity, 1);
        assert_eq!(hb.synced_up_to_hlc, 77);

        let cache = MigrationStatusCache::default();
        cache.insert(&hb);
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .expect("built heartbeat is cacheable");
        assert_eq!(entry.residue_identity, 1);
    }

    /// The facts builder's `residue_identity` is computed by INVOKING the 6c.6
    /// residue scan (`Index::count_unconverted_identity_gated`), not hardcoded.
    /// Driven here over an `IterableStorage` adaptor (`MockedStorage`) so the
    /// wiring is exercised end-to-end: seed two stale identity-gated entries +
    /// one already-converted + one Convergent, and the helper reports exactly
    /// the two stale identity-gated entries. (Production binds the empty-keyspace
    /// `CommittedStateScan`, which yields the documented conservative 0; this
    /// proves the scan is wired and ready for a key-iterating committed-state
    /// adaptor.)
    #[test]
    fn residue_identity_count_invokes_the_scan() {
        use calimero_storage::address::Id;
        use calimero_storage::entities::{ChildInfo, Metadata, StorageType};
        use calimero_storage::index::Index;
        use calimero_storage::store::MockedStorage;

        type S = MockedStorage<7200>;

        let owner = PublicKey::from([0xAAu8; 32]);
        let seed_user = |id: Id, schema: Option<u32>| {
            let mut md = Metadata::new(1, 1);
            md.storage_type = StorageType::User {
                owner,
                signature_data: None,
            };
            md.schema_version = schema;
            <Index<S>>::add_root(ChildInfo::new(id, [0u8; 32], md)).expect("seed user entry");
        };

        // Two stale identity-gated entries (residue), one already at target, one
        // Convergent (Public) that must never count.
        seed_user(Id::new([1; 32]), None);
        seed_user(Id::new([2; 32]), Some(1));
        seed_user(Id::new([3; 32]), Some(2));
        let mut public_md = Metadata::new(1, 1);
        public_md.storage_type = StorageType::Public;
        <Index<S>>::add_root(ChildInfo::new(Id::new([4; 32]), [0u8; 32], public_md))
            .expect("seed public entry");

        assert_eq!(
            residue_identity_count::<S>(2),
            2,
            "residue_identity must INVOKE the scan and count only stale \
             identity-gated entries"
        );
    }

    /// A context that lives in a SUBGROUP under the namespace (not a direct
    /// child of the namespace-root group — the stranded-resync e2e topology
    /// where a node joins via `join_subgroup_inheritance`) MUST be enumerated
    /// by the facts builder: its stranded-at-v1 state surfaces as `residue_auto`
    /// and its persisted `NoMigrationPath` marker as `migration_failed`. Before
    /// the descendant-tree enumeration, `enumerate_group_contexts(namespace)`
    /// alone skipped subgroup contexts, so the failure was silently dropped
    /// (the heartbeat reported `unknown` instead of `failed`).
    #[test]
    fn facts_enumerate_context_in_subgroup_and_surface_failure() {
        use calimero_context::group_store::{NamespaceRepository, UpgradesRepository};
        use calimero_context_config::types::ContextGroupId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
        use std::sync::Arc;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = [0x7Au8; 32];
        let subgroup = ContextGroupId::from([0x7Bu8; 32]);

        // The namespace is migrating to v2.
        UpgradesRepository::new(&store)
            .save(
                &ContextGroupId::from(ns),
                &GroupUpgradeValue {
                    from_version: "1".to_owned(),
                    to_version: "2".to_owned(),
                    migration: None,
                    initiated_at: 0,
                    initiated_by: PrivateKey::random(&mut rand::thread_rng()).public_key(),
                    status: GroupUpgradeStatus::InProgress {
                        total: 1,
                        completed: 0,
                        failed: 0,
                    },
                    cascade_hlc: None,
                    cascade_seq: None,
                },
            )
            .unwrap();

        // Nest the subgroup under the namespace-root group, then register a
        // stranded-at-v1 context in the SUBGROUP (NOT the namespace-root group).
        NamespaceRepository::new(&store)
            .nest(&ContextGroupId::from(ns), &subgroup)
            .unwrap();
        let ctx = [0xE1u8; 32];
        install_loaded_context_in_group(&store, &subgroup, ctx, "1.0.0");

        // The node stranded at v1 (NoMigrationPath) — the marker is persisted on
        // the subgroup context.
        store
            .handle()
            .put(
                &calimero_store::key::ContextMigrationFailed::new(ctx.into()),
                &calimero_store::types::ContextMigrationFailed {
                    kind: MigrationFailureKind::NoMigrationPath.to_u8(),
                },
            )
            .expect("put marker");

        let facts = compute_namespace_migration_facts(&store, ns);
        assert_eq!(
            facts.schema_version, 1,
            "the subgroup context's loaded v1 must govern the advertised version"
        );
        assert_eq!(
            facts.residue_auto, 1,
            "the subgroup context trails the target and counts as residue_auto"
        );
        assert_eq!(
            facts.migration_failed,
            Some(MigrationFailureKind::NoMigrationPath),
            "a stranded subgroup context's failure marker must surface as failed \
             (was silently lost when only namespace-root contexts were enumerated)"
        );
    }
}
