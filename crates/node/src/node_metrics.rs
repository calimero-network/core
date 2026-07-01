//! Centralised metric families for node-level observability.
//!
//! Holds the families that don't naturally belong to a single submodule (blob
//! cache, sync sessions, governance-pending buffer, delta-store apply rate,
//! HTTP request volume, build info, process resources) and the periodic
//! gauge-snapshot tick that polls the [`NodeState`] DashMaps so operators
//! can chart memory-leak shapes without instrumenting every mutation site.
//!
//! All series here are registered once at startup via [`NodeMetrics::new`].
//! Recording is via owned handles (counters / families / gauges) cloned out
//! of [`NodeMetrics`]; clones share the underlying atomic so updates from
//! any thread land on the same series.
//!
//! ## Cardinality discipline
//!
//! Per-context labels are deliberately avoided on hot-path counters. A
//! merod node can host hundreds of contexts; multiplying that by the
//! per-counter Prometheus storage (~1 KB resident) blows the scraper's
//! budget. Where per-context detail is genuinely useful (e.g.
//! `governance_pending_queue_depth`) we expose the *sum* over contexts as
//! a single gauge and rely on logs for per-context drill-down. Histograms
//! with high-cardinality labels (`http_request_duration_seconds{path}`)
//! use coarse path templates rather than raw URIs.

use std::time::Duration;

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;
use tracing::trace;

/// Build-info beacon labels.
///
/// `version` is `calimero-node`'s `CARGO_PKG_VERSION` at compile time;
/// `peer_id` is the node's libp2p PeerId. The beacon is a constant `1`
/// gauge — operators querying `merod_build_info` confirm both that the
/// scrape pipeline is alive and which build / identity is responding,
/// independent of whether the node has yet produced any other metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct BuildInfoLabels {
    pub(crate) version: String,
    pub(crate) peer_id: String,
}

/// Outcome label for delta-store events.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct DeltaApplyLabels {
    /// One of: `applied`, `pending`, `cascaded`, `duplicate`, `error`.
    pub(crate) outcome: String,
}

/// Outcome label for governance-pending drain events.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct GovernanceDrainLabels {
    /// One of: `applied`, `removed`, `never_member`, `rebuffered`,
    /// `dropped_max_attempts`, `lookup_error`.
    pub(crate) outcome: String,
}

/// Outcome label for HC / LevelWise / EntityPush per-leaf drops in
/// `sync::helpers::is_leaf_currently_authorized`. Separates "author is
/// not currently a member" (an expected, common outcome under churn)
/// from "the store lookup itself failed" (rare, indicates I/O trouble
/// — should never approach the rate of the former).
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct LeafDropLabels {
    /// One of: `unauthorized` (membership check returned `false`),
    /// `lookup_error` (storage layer raised).
    pub(crate) reason: String,
}

/// Snapshot of all node-level metric handles.
///
/// Clone-friendly: the families and counters are wrapped in `Arc` by
/// `prometheus-client`, so a cheap clone hands a thread its own recording
/// surface without re-registering anything.
#[derive(Clone, Debug)]
pub(crate) struct NodeMetrics {
    // Build-info beacon — constant 1 gauge.
    pub(crate) build_info: Family<BuildInfoLabels, Gauge>,

    // NodeState DashMap sizes — polled periodically; never recorded on a
    // hot path.
    pub(crate) blob_cache_entries: Gauge,
    pub(crate) blob_cache_size_bytes: Gauge,
    pub(crate) delta_stores_count: Gauge,
    pub(crate) sync_sessions_active: Gauge,
    pub(crate) governance_pending_contexts: Gauge,
    pub(crate) governance_pending_queue_depth: Gauge,
    pub(crate) specialized_node_pending_invites: Gauge,

    // Blob cache eviction counters — recorded on the eviction hot path
    // (state.rs::evict_old_blobs).
    pub(crate) blob_cache_evictions_age_total: Counter,
    pub(crate) blob_cache_evictions_count_total: Counter,
    pub(crate) blob_cache_evictions_memory_total: Counter,

    // Delta-store outcomes — high-volume but bounded label cardinality.
    pub(crate) delta_outcomes_total: Family<DeltaApplyLabels, Counter>,
    pub(crate) delta_cascade_size: Histogram,
    pub(crate) delta_missing_parents_total: Counter,
    pub(crate) dag_heads_count: Histogram,

    // DAG compaction (#2026).
    pub(crate) dag_compaction_deltas_pruned_total: Counter,

    // HC / LevelWise / EntityPush per-leaf authorization drops.
    pub(crate) hc_leaf_drops_total: Family<LeafDropLabels, Counter>,

    // Governance-pending drain outcomes (B2 buffer-on-unknown lifecycle).
    pub(crate) governance_drain_outcomes_total: Family<GovernanceDrainLabels, Counter>,

    // Process resource gauges — polled periodically on linux via
    // /proc/self/status + /proc/self/fd. Only present (and only registered)
    // on linux: on other platforms there is no /proc to read, and exposing
    // permanently-zero gauges misleads dashboards into reading a real 0.
    #[cfg(target_os = "linux")]
    pub(crate) process_resident_memory_bytes: Gauge,
    #[cfg(target_os = "linux")]
    pub(crate) process_virtual_memory_bytes: Gauge,
    #[cfg(target_os = "linux")]
    pub(crate) process_threads: Gauge,
    #[cfg(target_os = "linux")]
    pub(crate) process_open_fds: Gauge,
}

impl NodeMetrics {
    /// Register every family on `registry` and return the recording handles.
    /// Called once from `run::start` before the metrics service is mounted.
    pub(crate) fn new(registry: &mut Registry) -> Self {
        let build_info: Family<BuildInfoLabels, Gauge> = Family::default();
        registry.register(
            "merod_build_info",
            "Constant 1 gauge labeled with merod version and peer_id — \
             operators use it to verify the metrics pipeline end-to-end",
            build_info.clone(),
        );

        let blob_cache_entries = Gauge::default();
        registry.register(
            "blob_cache_entries",
            "Number of blobs currently held in the in-memory blob cache",
            blob_cache_entries.clone(),
        );
        let blob_cache_size_bytes = Gauge::default();
        registry.register(
            "blob_cache_size_bytes",
            "Total resident bytes across all blobs in the blob cache",
            blob_cache_size_bytes.clone(),
        );
        let delta_stores_count = Gauge::default();
        registry.register(
            "delta_stores_count",
            "Number of contexts with a live in-memory DeltaStore",
            delta_stores_count.clone(),
        );
        let sync_sessions_active = Gauge::default();
        registry.register(
            "sync_sessions_active",
            "Number of contexts with an open snapshot-sync session (buffering deltas)",
            sync_sessions_active.clone(),
        );
        let governance_pending_contexts = Gauge::default();
        registry.register(
            "governance_pending_contexts",
            "Number of contexts that currently have at least one delta in the \
             B2 governance-pending buffer",
            governance_pending_contexts.clone(),
        );
        let governance_pending_queue_depth = Gauge::default();
        registry.register(
            "governance_pending_queue_depth",
            "Sum of governance-pending buffer depths across all contexts — \
             monotonic growth indicates B2 buffer cannot drain",
            governance_pending_queue_depth.clone(),
        );
        let specialized_node_pending_invites = Gauge::default();
        registry.register(
            "specialized_node_pending_invites",
            "Number of in-flight specialized-node invite verifications",
            specialized_node_pending_invites.clone(),
        );

        let blob_cache_evictions_age_total = Counter::default();
        registry.register(
            "blob_cache_evictions_age_total",
            "Blob cache entries evicted because they exceeded MAX_BLOB_AGE_S",
            blob_cache_evictions_age_total.clone(),
        );
        let blob_cache_evictions_count_total = Counter::default();
        registry.register(
            "blob_cache_evictions_count_total",
            "Blob cache entries evicted to keep entry count under MAX_BLOB_CACHE_COUNT",
            blob_cache_evictions_count_total.clone(),
        );
        let blob_cache_evictions_memory_total = Counter::default();
        registry.register(
            "blob_cache_evictions_memory_total",
            "Blob cache entries evicted to keep resident bytes under MAX_BLOB_CACHE_SIZE_BYTES",
            blob_cache_evictions_memory_total.clone(),
        );

        let delta_outcomes_total: Family<DeltaApplyLabels, Counter> = Family::default();
        registry.register(
            "delta_outcomes_total",
            "DAG delta-apply outcomes, sliced by outcome \
             (applied, pending, cascaded, duplicate, error)",
            delta_outcomes_total.clone(),
        );
        // Buckets for cascade fan-out — typical value is 0 or 1 (no cascade
        // or single child), pathological partition-heal flows can produce
        // chains of dozens. The 256 ceiling exists because a pathological
        // run that produces 1000s is an apply-loop pathology that should
        // page someone, and we'd rather see it pinned to "overflow" than
        // distort the histogram.
        let delta_cascade_size = Histogram::new([0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 256.0]);
        registry.register(
            "delta_cascade_size",
            "Number of pending deltas applied via cascade when a parent finally lands",
            delta_cascade_size.clone(),
        );
        let delta_missing_parents_total = Counter::default();
        registry.register(
            "delta_missing_parents_total",
            "Number of missing-parent requests issued to peers across all contexts",
            delta_missing_parents_total.clone(),
        );

        // DAG heads observed after every delta apply. Per-context labels are
        // intentionally omitted (see "Cardinality discipline" above); the
        // sum-across-contexts distribution is enough to chart fan-out
        // growth. Buckets stop at 256 because anything past that is a
        // pathology that should page someone — operators will see the
        // overflow bucket fill rather than the histogram lose resolution.
        // The instrumentation step for #2356 item 2: data the cap-mechanism
        // discussion currently lacks.
        let dag_heads_count = Histogram::new([1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0]);
        registry.register(
            "dag_heads_count",
            "Number of concurrent DAG heads observed at the end of each delta apply, \
             across all contexts on this node",
            dag_heads_count.clone(),
        );

        // DAG compaction (#2026): how much history has been reclaimed. Counted
        // on the compactor side, where the figure is unambiguous (a delta
        // catch-up hitting a peer's `DeltaNotFound` can't be attributed to
        // pruning vs. a persist race vs. an unverifiable row, so there is no
        // honest receiver-side fallback counter).
        let dag_compaction_deltas_pruned_total = Counter::default();
        registry.register(
            "dag_compaction_deltas_pruned_total",
            "Total DAG delta rows pruned by compaction across all contexts",
            dag_compaction_deltas_pruned_total.clone(),
        );

        let hc_leaf_drops_total: Family<LeafDropLabels, Counter> = Family::default();
        registry.register(
            "hc_leaf_drops_total",
            "HC / LevelWise / EntityPush leaves dropped by the apply-time \
             current-membership check, labelled by reason",
            hc_leaf_drops_total.clone(),
        );

        let governance_drain_outcomes_total: Family<GovernanceDrainLabels, Counter> =
            Family::default();
        registry.register(
            "governance_drain_outcomes_total",
            "B2 governance-pending drain outcomes per delta",
            governance_drain_outcomes_total.clone(),
        );

        // Registered only on linux — see the field docs. On other platforms
        // these series simply don't exist rather than reading a constant 0.
        #[cfg(target_os = "linux")]
        let process_resident_memory_bytes = Gauge::default();
        #[cfg(target_os = "linux")]
        registry.register(
            "process_resident_memory_bytes",
            "Resident set size of the merod process, in bytes",
            process_resident_memory_bytes.clone(),
        );
        #[cfg(target_os = "linux")]
        let process_virtual_memory_bytes = Gauge::default();
        #[cfg(target_os = "linux")]
        registry.register(
            "process_virtual_memory_bytes",
            "Virtual memory size of the merod process, in bytes",
            process_virtual_memory_bytes.clone(),
        );
        #[cfg(target_os = "linux")]
        let process_threads = Gauge::default();
        #[cfg(target_os = "linux")]
        registry.register(
            "process_threads",
            "Thread count of the merod process",
            process_threads.clone(),
        );
        #[cfg(target_os = "linux")]
        let process_open_fds = Gauge::default();
        #[cfg(target_os = "linux")]
        registry.register(
            "process_open_fds",
            "Open file descriptors of the merod process",
            process_open_fds.clone(),
        );

        Self {
            build_info,
            blob_cache_entries,
            blob_cache_size_bytes,
            delta_stores_count,
            sync_sessions_active,
            governance_pending_contexts,
            governance_pending_queue_depth,
            specialized_node_pending_invites,
            blob_cache_evictions_age_total,
            blob_cache_evictions_count_total,
            blob_cache_evictions_memory_total,
            delta_outcomes_total,
            delta_cascade_size,
            delta_missing_parents_total,
            dag_heads_count,
            dag_compaction_deltas_pruned_total,
            hc_leaf_drops_total,
            governance_drain_outcomes_total,
            #[cfg(target_os = "linux")]
            process_resident_memory_bytes,
            #[cfg(target_os = "linux")]
            process_virtual_memory_bytes,
            #[cfg(target_os = "linux")]
            process_threads,
            #[cfg(target_os = "linux")]
            process_open_fds,
        }
    }

    /// Set the constant-1 build-info beacon. Called once at startup with the
    /// node's identity and crate version.
    pub(crate) fn set_build_info(&self, version: &str, peer_id: &str) {
        self.build_info
            .get_or_create(&BuildInfoLabels {
                version: version.to_owned(),
                peer_id: peer_id.to_owned(),
            })
            .set(1);
    }
}

/// Snapshot of the DashMap-backed [`crate::NodeState`] sizes that the
/// periodic tick will publish into [`NodeMetrics`]. Kept as a plain struct
/// so the tick can be unit-tested without spinning up a real `NodeState`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NodeStateSnapshot {
    pub blob_cache_entries: usize,
    pub blob_cache_size_bytes: usize,
    pub delta_stores_count: usize,
    pub sync_sessions_active: usize,
    pub governance_pending_contexts: usize,
    pub governance_pending_queue_depth: usize,
    pub specialized_node_pending_invites: usize,
}

impl NodeStateSnapshot {
    /// Read every DashMap once. Cheap (`len()` is O(shards)), but the
    /// blob_cache_size_bytes walk is O(entries) because every entry stores
    /// its data size — that's fine at the 30s scrape interval (a few
    /// thousand entries max).
    pub(crate) fn capture(state: &crate::state::NodeState) -> Self {
        let blob_cache_entries = state.blob_cache.len();
        let blob_cache_size_bytes = state
            .blob_cache
            .iter()
            .map(|entry| entry.value().data.len())
            .sum();
        let delta_stores_count = state.delta_stores.len();
        let sync_sessions_active = state.sync_sessions.len();
        // `governance_pending_contexts` is the number of distinct context
        // entries; `governance_pending_queue_depth` is the sum of their
        // queue lengths. Two gauges, one DashMap pass.
        let mut governance_pending_contexts = 0;
        let mut governance_pending_queue_depth = 0;
        for entry in state.governance_pending.iter() {
            governance_pending_contexts += 1;
            governance_pending_queue_depth += entry.value().len();
        }
        let specialized_node_pending_invites = state.pending_specialized_node_invites.len();
        Self {
            blob_cache_entries,
            blob_cache_size_bytes,
            delta_stores_count,
            sync_sessions_active,
            governance_pending_contexts,
            governance_pending_queue_depth,
            specialized_node_pending_invites,
        }
    }

    /// Apply this snapshot to the gauges. Separate from `capture` so the
    /// tick can log + clamp before writing if a future need arises.
    pub(crate) fn publish(&self, metrics: &NodeMetrics) {
        metrics
            .blob_cache_entries
            .set(self.blob_cache_entries as i64);
        metrics
            .blob_cache_size_bytes
            .set(self.blob_cache_size_bytes as i64);
        metrics
            .delta_stores_count
            .set(self.delta_stores_count as i64);
        metrics
            .sync_sessions_active
            .set(self.sync_sessions_active as i64);
        metrics
            .governance_pending_contexts
            .set(self.governance_pending_contexts as i64);
        metrics
            .governance_pending_queue_depth
            .set(self.governance_pending_queue_depth as i64);
        metrics
            .specialized_node_pending_invites
            .set(self.specialized_node_pending_invites as i64);
    }
}

/// Period for the gauge-snapshot tick. Matched to the default scrape
/// interval (30s) so each scrape sees a value at most 30s stale.
pub(crate) const METRICS_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Spawn the periodic metrics tick. The returned task captures clones of
/// the metric handles and a weak handle to the `NodeState`'s DashMaps via
/// the `state` clone (NodeState is itself Clone over Arc'd inner maps).
///
/// The task lives until the runtime shuts down. It deliberately holds a
/// strong reference: a missing scrape during shutdown is harmless, and
/// adding shutdown plumbing here would inflate the surface for no real
/// benefit.
pub(crate) fn spawn_metrics_tick(
    metrics: NodeMetrics,
    state: crate::state::NodeState,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(METRICS_TICK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // `tokio::time::interval` returns immediately on the first
        // `.tick()` call ("immediate-fire") and then ticks every
        // `METRICS_TICK_INTERVAL` thereafter. We *consume* that
        // first immediate-fire so the very first gauge snapshot
        // lands at startup_time + INTERVAL, not at startup_time:
        // recording zero-valued gauges before the node has stood up
        // its DashMaps would pin a misleading "all zero" point on
        // every dashboard. After this, the loop awaits the regular
        // 30s cadence and snapshots once per fire.
        let _ = interval.tick().await;
        loop {
            interval.tick().await;
            let snapshot = NodeStateSnapshot::capture(&state);
            trace!(?snapshot, "node_metrics tick");
            snapshot.publish(&metrics);
            update_process_metrics(&metrics);
        }
    })
}

/// Read process resource counters from `/proc/self/*` on linux and publish
/// them as gauges. No-op on non-linux platforms (RSS/threads/FDs stay at
/// their default 0).
///
/// We intentionally do this inline (no `procfs` / `sysinfo` crate) — the
/// file format is stable, the parse is small, and an extra dep purely for
/// 20 lines of /proc reading is overkill.
fn update_process_metrics(metrics: &NodeMetrics) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                // VmRSS / VmSize fields are reported in kB.
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    if let Some(kb) = rest
                        .split_whitespace()
                        .next()
                        .and_then(|v| v.parse::<i64>().ok())
                    {
                        metrics
                            .process_resident_memory_bytes
                            .set(kb.saturating_mul(1024));
                    }
                } else if let Some(rest) = line.strip_prefix("VmSize:") {
                    if let Some(kb) = rest
                        .split_whitespace()
                        .next()
                        .and_then(|v| v.parse::<i64>().ok())
                    {
                        metrics
                            .process_virtual_memory_bytes
                            .set(kb.saturating_mul(1024));
                    }
                } else if let Some(rest) = line.strip_prefix("Threads:") {
                    if let Some(n) = rest
                        .split_whitespace()
                        .next()
                        .and_then(|v| v.parse::<i64>().ok())
                    {
                        metrics.process_threads.set(n);
                    }
                }
            }
        }
        // /proc/self/fd is a directory whose entry count is the open-fd
        // count. `read_dir` itself opens a transient FD on the directory
        // that appears in the listing — we subtract 1 so the reported
        // value matches what `lsof -p $PID | wc -l` would show.
        // Skip on read errors (sandboxes occasionally restrict this).
        if let Ok(fd_dir) = std::fs::read_dir("/proc/self/fd") {
            let count = (fd_dir.filter_map(Result::ok).count() as i64).saturating_sub(1);
            metrics.process_open_fds.set(count);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // No-op: gauges stay at default 0 on non-linux platforms.
        let _ = metrics;
    }
}

/// Helper for callers (delta store, governance drain, network bridge,
/// HTTP middleware) that need to bump counters but don't carry a
/// `NodeMetrics` clone yet. The global `OnceLock` is set exactly once
/// at startup; clones from `get()` are cheap.
///
/// Why a global: half the recording sites live in synchronous code paths
/// (e.g. blob-cache eviction inside `NodeState`) where plumbing a metric
/// handle would require changing many signatures. The global is the
/// pragmatic alternative — single-write/multi-read, identical to the
/// existing `GROUP_STORE_METRICS` pattern in `crates/context/src/metrics.rs`.
static GLOBAL: std::sync::OnceLock<NodeMetrics> = std::sync::OnceLock::new();

/// Install `metrics` as the global. Idempotent on second call — extra calls
/// are silently discarded so test harnesses that spin up multiple nodes in
/// one process don't panic. The "winner" is whichever `start()` runs first.
pub(crate) fn install_global(metrics: NodeMetrics) {
    let _ = GLOBAL.set(metrics);
}

/// Fetch a clone of the global handles. Returns `None` before
/// [`install_global`] has been called (the unit-test path, or early
/// startup before `run::start` mounts the registry). Recording sites
/// must tolerate `None` — a missing metric is never a correctness issue.
pub(crate) fn global() -> Option<&'static NodeMetrics> {
    GLOBAL.get()
}

/// Bump a delta-outcome counter via the global handle. Silent no-op if
/// the global is not yet installed.
pub(crate) fn record_delta_outcome(outcome: &str) {
    if let Some(m) = global() {
        m.delta_outcomes_total
            .get_or_create(&DeltaApplyLabels {
                outcome: outcome.to_owned(),
            })
            .inc();
    }
}

/// Observe a delta-cascade size. Used at the end of an `apply_pending`
/// pass to record how many deltas the cascade unblocked.
pub(crate) fn observe_delta_cascade(size: usize) {
    if let Some(m) = global() {
        m.delta_cascade_size.observe(size as f64);
    }
}

/// Observe the number of concurrent DAG heads after a delta apply.
/// Called from `DeltaStore::add_delta_internal` right after the DAG
/// reports its post-apply heads. Drives the #2356 item 2 head-fan-out
/// decision: the cap mechanism + threshold should be picked from this
/// histogram's p99 over a representative production window, not from
/// #2293's stale 9.6 s anecdote.
pub(crate) fn observe_dag_heads_count(heads: usize) {
    if let Some(m) = global() {
        m.dag_heads_count.observe(heads as f64);
    }
}

/// Record deltas reclaimed by a compaction sweep (#2026).
pub(crate) fn observe_compaction_pruned(count: usize) {
    if let Some(m) = global() {
        m.dag_compaction_deltas_pruned_total.inc_by(count as u64);
    }
}

/// Bump the missing-parents request counter once per `request_missing_deltas`
/// dispatch.
pub(crate) fn record_missing_parents_request(count: usize) {
    if let Some(m) = global() {
        m.delta_missing_parents_total.inc_by(count as u64);
    }
}

/// Bump a governance-drain outcome counter.
pub(crate) fn record_governance_drain_outcome(outcome: &str) {
    if let Some(m) = global() {
        m.governance_drain_outcomes_total
            .get_or_create(&GovernanceDrainLabels {
                outcome: outcome.to_owned(),
            })
            .inc();
    }
}

/// Bump the HC leaf-drop counter. `reason` is one of:
///
/// * `"unauthorized"` — the current-membership check returned `false`;
///   author was either never a member or has been removed.
/// * `"lookup_error"` — the underlying store lookup raised; receiver
///   dropped the leaf defensively rather than risking a silent bypass.
///
/// Operators care about the *ratio* between these two: a steady stream
/// of `unauthorized` under churn is normal (legitimate post-removal
/// rejection); a non-trivial rate of `lookup_error` is an I/O signal
/// that warrants investigation, since each one is a silently-dropped
/// entity.
pub(crate) fn record_hc_leaf_drop(reason: &str) {
    if let Some(m) = global() {
        m.hc_leaf_drops_total
            .get_or_create(&LeafDropLabels {
                reason: reason.to_owned(),
            })
            .inc();
    }
}

/// Bump a blob-cache eviction counter for the given reason
/// (`"age"`, `"count"`, `"memory"`).
pub(crate) fn record_blob_cache_eviction(reason: &str, n: u64) {
    if let Some(m) = global() {
        let counter = match reason {
            "age" => &m.blob_cache_evictions_age_total,
            "count" => &m.blob_cache_evictions_count_total,
            "memory" => &m.blob_cache_evictions_memory_total,
            _ => return,
        };
        counter.inc_by(n);
    }
}
