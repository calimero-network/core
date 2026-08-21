use std::sync::OnceLock;

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;

#[derive(Clone, Debug)]
pub struct Metrics {
    pub execution_count: Family<ExecutionLabels, Gauge>,
    pub execution_duration: Family<ExecutionLabels, Histogram>,

    /// Cumulative count of in-memory context-cache hits (the requested
    /// context was already resident in `ContextManager::contexts`).
    pub context_cache_hits: Counter,
    /// Cumulative count of context-cache misses (the context had to be
    /// fetched from the authoritative datastore and inserted).
    pub context_cache_misses: Counter,
    /// Current number of contexts resident in the in-memory hot cache.
    /// Set from the periodic cache-stats task, so it tracks the cap
    /// (`MAX_CACHED_CONTEXTS`) at ~5-minute resolution.
    pub context_cache_size: Gauge,
    /// Current number of application-metadata entries resident in the
    /// in-memory cache. Reported alongside `context_cache_size`.
    pub application_cache_size: Gauge,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ExecutionLabels {
    pub context_id: String,
    pub method: String,
    pub status: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct NamespaceRetryLabels {
    pub(crate) status: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct NamespaceDecodeLabels {
    pub(crate) status: String,
    pub(crate) kind: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct MembershipPolicyLabels {
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct GovernancePublishLabels {
    pub(crate) op_kind: String,
}

/// Labels for handler-level governance op delivery outcomes.
///
/// `outcome` is one of:
///   - `"acked"`: at least one valid ack was collected within `op_timeout`.
///   - `"empty"`: the op was published but no ack arrived in time. The
///     local DAG advance is durable; downstream peers will reconcile via
///     parent_pull / readiness beacons. Useful as a leading indicator of
///     mesh fragility (cold-start, partition, GRAFT delay).
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct GovernanceHandlerDeliveryLabels {
    pub(crate) handler: String,
    pub(crate) op_kind: String,
    pub(crate) outcome: String,
}

/// Labels for self-purge failures (TEE self-eviction local-state cleanup).
///
/// `branch` is one of:
///   - `"subgroup"`: a subgroup-only purge (`purge_subgroup_for_self`).
///   - `"namespace"`: a namespace-root cascade (`cascade_namespace_state`).
///
/// `class` is the failure class:
///   - `"group_rows"`: the security-critical `delete_group_local_rows`
///     step failed, so the group's encryption keys may linger on disk.
///     This is the load-bearing failure — it keeps the `NamespaceParticipation`
///     anchor + gossipsub subscription alive for the planned reconcile
///     sweep (#2721).
///   - `"context_cleanup"`: a best-effort dead-pointer cleanup step
///     (context-index unregister, parent-edge read, or tree-edge delete)
///     failed. Non-security: the orphaned rows point at soon-to-be / now
///     deleted groups. Namespace deletion + unsubscribe still proceed.
///
/// These are produced in `calimero_context::self_purge`; this struct stays
/// crate-private (only `record_purge_failure`, taking the public
/// `PurgeBranch`/`PurgeFailureClass` enums, crosses the crate boundary).
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct SelfPurgeFailureLabels {
    // `&'static str`, not `String`: both values come from the closed
    // `PurgeBranch` / `PurgeFailureClass` enums via their `as_label`
    // (already `&'static str`), so the label set allocates nothing per
    // `record_purge_failure` call.
    pub(crate) branch: &'static str,
    pub(crate) class: &'static str,
}

/// Labels for self-purge reconcile-sweep outcomes (#2686).
///
/// `outcome` is one per marked namespace the startup sweep processes, one
/// of:
///   - `"reconciled"`: marker present + still-evicted, and the namespace
///     purge fully completed.
///   - `"retained"`: marker present + still-evicted, but the purge returned
///     false (row-purge failure); the marker is kept for the next restart.
///   - `"cleared_stale"`: the marker was stale (already purged / re-admitted)
///     and the clear succeeded.
///   - `"stale_clear_failed"`: the marker was stale but the clear itself
///     failed (the next restart re-evaluates and retries the clear).
///   - `"skipped"`: a read error made the decision uncertain; the marker is
///     kept (never purge on uncertainty) for the next restart.
///
/// As with [`SelfPurgeFailureLabels`], `outcome` is a `&'static str` sourced
/// from the closed [`ReconcileOutcome`] enum via [`ReconcileOutcome::as_label`],
/// so the label set allocates nothing per `record_reconcile_outcome` call.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct SelfPurgeReconcileLabels {
    pub(crate) outcome: &'static str,
}

/// Why an at-cut authority question could not be decided, for
/// `at_cut_undecidable_total`.
///
/// As with [`SelfPurgeReconcileLabels`], `cause` is a `&'static str` from the
/// closed [`UndecidableCause`] enum, so the label set allocates nothing per
/// [`record_at_cut_undecidable`] call. The label space is therefore bounded by
/// the enum's variant count — it can never be widened by a peer's input, which
/// matters because the recording site is driven by ops that arrive off the wire.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct AtCutUndecidableLabels {
    pub(crate) cause: &'static str,
}

#[derive(Clone, Debug)]
struct GroupStoreMetricSink {
    namespace_retry_events: Family<NamespaceRetryLabels, Counter>,
    namespace_decode_events: Family<NamespaceDecodeLabels, Counter>,
    membership_policy_rejections: Family<MembershipPolicyLabels, Counter>,
    governance_publish_mesh_peers: Family<GovernancePublishLabels, Histogram>,
    governance_handler_delivery_total: Family<GovernanceHandlerDeliveryLabels, Counter>,
    governance_handler_delivery_seconds: Family<GovernanceHandlerDeliveryLabels, Histogram>,
    self_purge_failures: Family<SelfPurgeFailureLabels, Counter>,
    self_purge_reconcile: Family<SelfPurgeReconcileLabels, Counter>,
    self_purge_events_dropped: Counter,
    at_cut_undecidable: Family<AtCutUndecidableLabels, Counter>,
}

static GROUP_STORE_METRICS: OnceLock<GroupStoreMetricSink> = OnceLock::new();

impl Metrics {
    pub fn new(registry: &mut Registry) -> Self {
        let context_registry = registry.sub_registry_with_prefix("context");

        let runtime_registry = context_registry.sub_registry_with_prefix("runtime");

        let execution_count = Family::<ExecutionLabels, Gauge>::default();
        runtime_registry.register(
            "execution_count",
            "Context runtime execution counter",
            execution_count.clone(),
        );
        let execution_duration = Family::<ExecutionLabels, Histogram>::new_with_constructor(|| {
            Histogram::new(exponential_buckets(1.0, 2.0, 10))
        });
        runtime_registry.register(
            "execution_duration_seconds",
            "Context runtime execution in seconds",
            execution_duration.clone(),
        );

        // Context in-memory cache effectiveness. Hits/misses are
        // monotonic counters incremented at the cache-aside entry point
        // (`get_or_fetch_context`); the hit *rate* is derived in PromQL as
        // `rate(hits) / (rate(hits) + rate(misses))`. The size gauges are
        // refreshed by the periodic cache-stats task.
        let cache_registry = context_registry.sub_registry_with_prefix("cache");

        let context_cache_hits = Counter::default();
        cache_registry.register(
            "hits",
            "Cumulative in-memory context cache hits",
            context_cache_hits.clone(),
        );
        let context_cache_misses = Counter::default();
        cache_registry.register(
            "misses",
            "Cumulative in-memory context cache misses (datastore fallback)",
            context_cache_misses.clone(),
        );
        let context_cache_size = Gauge::default();
        cache_registry.register(
            "size",
            "Number of contexts resident in the in-memory hot cache. \
             Refreshed by the periodic cache-stats task (~5-minute resolution), \
             so it may lag faster scrape intervals — use hits/misses rates for \
             fine-grained signal",
            context_cache_size.clone(),
        );
        let application_cache_size = Gauge::default();
        cache_registry.register(
            "application_size",
            "Number of application-metadata entries resident in the cache. \
             Refreshed by the periodic cache-stats task (~5-minute resolution)",
            application_cache_size.clone(),
        );

        let group_store_registry = context_registry.sub_registry_with_prefix("group_store");

        let namespace_retry_events = Family::<NamespaceRetryLabels, Counter>::default();
        group_store_registry.register(
            "namespace_retry_events_total",
            "Namespace encrypted-op retry events by status",
            namespace_retry_events.clone(),
        );

        let namespace_decode_events = Family::<NamespaceDecodeLabels, Counter>::default();
        group_store_registry.register(
            "namespace_decode_events_total",
            "Namespace op-log decode events by status and entry kind",
            namespace_decode_events.clone(),
        );

        let membership_policy_rejections = Family::<MembershipPolicyLabels, Counter>::default();
        group_store_registry.register(
            "membership_policy_rejections_total",
            "Membership policy rejection counts by reason",
            membership_policy_rejections.clone(),
        );

        // Stage-0 baseline metric for #2237: number of mesh peers visible at
        // the moment a governance op is published. Buckets match the
        // "cold mesh" detection threshold (mesh_n_low ~= 4).
        let governance_publish_mesh_peers =
            Family::<GovernancePublishLabels, Histogram>::new_with_constructor(|| {
                Histogram::new([0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0])
            });
        group_store_registry.register(
            "governance_publish_mesh_peers_at_publish",
            "Number of mesh peers visible at the moment a governance op is published",
            governance_publish_mesh_peers.clone(),
        );

        // Phase 12.1 (#2237): handler-level delivery outcomes. Counters
        // and a wait-time histogram sliced by handler / op_kind / outcome.
        // Operators use `outcome="empty"` rate as the leading indicator of
        // cold-start mesh fragility — under a healthy mesh it should be
        // approximately zero in steady state.
        let governance_handler_delivery_total =
            Family::<GovernanceHandlerDeliveryLabels, Counter>::default();
        group_store_registry.register(
            "governance_handler_delivery_total",
            "Governance op publish outcomes, sliced by handler / op_kind / outcome",
            governance_handler_delivery_total.clone(),
        );

        // Buckets cover the realistic ack-wait range: 1ms → 30s. Cheap
        // ops (alias / capability) settle ≤100ms; membership ops 100ms–2s;
        // heavy ops (create_context / upgrade) up to ~10s; the 30s tail
        // catches op_timeout-bound publishes.
        let governance_handler_delivery_seconds =
            Family::<GovernanceHandlerDeliveryLabels, Histogram>::new_with_constructor(|| {
                Histogram::new([
                    0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0,
                ])
            });
        group_store_registry.register(
            "governance_handler_delivery_seconds",
            "Governance op ack-collection wait time at the handler boundary",
            governance_handler_delivery_seconds.clone(),
        );

        // #2686: self-purge failures on TEE self-eviction, sliced by
        // branch (subgroup / namespace) and failure-class (group_rows /
        // context_cleanup). The `class="group_rows"` series is the
        // security-relevant one — a nonzero rate means forward-secrecy
        // residue lingered on a node's own disk pending the reconcile
        // sweep (#2721). `class="context_cleanup"` is a best-effort
        // dead-pointer leak and is informational only.
        let self_purge_failures = Family::<SelfPurgeFailureLabels, Counter>::default();
        group_store_registry.register(
            "self_purge_failures_total",
            "Self-purge (TEE self-eviction) local-state cleanup failures, \
             sliced by branch (subgroup / namespace) and failure-class \
             (group_rows / context_cleanup)",
            self_purge_failures.clone(),
        );

        // #2686: reconcile-sweep outcomes. One increment per marked
        // namespace the startup sweep processes, sliced by `outcome`. The
        // `outcome="retained"` series is the operator alerting signal — a
        // namespace stuck `retained` across restarts means a row-purge
        // purge keeps failing and forward-secrecy residue lingers on disk.
        // `outcome="stale_clear_failed"` flags markers the sweep could not
        // clear (benign — re-evaluated next restart). The reconcile read-
        // uncertainty cases land in `outcome="skipped"` (NOT in
        // `self_purge_failures_total`, which is reserved for genuine
        // purge-step delete failures).
        let self_purge_reconcile = Family::<SelfPurgeReconcileLabels, Counter>::default();
        group_store_registry.register(
            "self_purge_reconcile_total",
            "Self-purge startup reconcile-sweep outcomes, one per marked \
             namespace processed, sliced by outcome (reconciled / retained / \
             cleared_stale / stale_clear_failed / skipped)",
            self_purge_reconcile.clone(),
        );

        // #2686: count of self-purge op-events dropped by the broadcast
        // `Lagged` arm. The broadcast reports a single total of dropped
        // events across EVERY `OpEvent` variant (MemberAdded, ContextRegistered,
        // …) — it does not tell us which were dropped, so this is an UPPER
        // BOUND on dropped `TeeMemberRemoved` evictions, not an exact count. A
        // dropped `TeeMemberRemoved` writes no pending-self-purge marker, so the
        // marker-gated reconcile cannot recover it — silent residue. A nonzero
        // rate means the self-purge subscriber fell >1024 events behind (the
        // broadcast capacity) and some evictions may have left un-reconcilable
        // on-disk residue (bounded; not a forward-secrecy hole — FS is held by
        // key rotation).
        let self_purge_events_dropped = Counter::default();
        group_store_registry.register(
            "self_purge_events_dropped_total",
            "All self-purge op-events dropped by the broadcast Lagged arm \
             (across every OpEvent variant — the broadcast does not report which \
             were dropped); an upper bound on dropped TeeMemberRemoved evictions, \
             each of which writes no reconcile marker (un-recoverable residue, \
             bounded)",
            self_purge_events_dropped.clone(),
        );

        // The apply-time authority gate refusing to answer, sliced by cause.
        // Deciding an op's authority means resolving it at the op's own causal
        // cut; when the cited ancestry isn't folded here, the gate refuses
        // (`AuthorityUndecidable`) and the op parks for retry rather than being
        // judged against this replica's current state, which would let two
        // replicas decide one op differently.
        //
        // The point of the `cause` slice is that a park is only *sometimes*
        // transient, and the three outcomes are operationally opposite:
        //
        // - `scope_unfed` / `heads_missing` / `ancestry_gap` — usually a node
        //   mid-backfill. Self-healing: the op retries once sync delivers the
        //   history. Expect a nonzero rate during catch-up, decaying to ~zero.
        // - `log_truncated` — the retained op-log no longer reaches the cut, so
        //   the walk can NEVER complete. Permanent, and it does not self-heal.
        //   A sustained rate here is the signal that a namespace's governance
        //   history has outgrown the retained window and that node's governance
        //   DAG has stopped advancing. This is the series to alert on.
        // - `fold_unavailable` / `namespace_unresolved` — a store or mapping
        //   fault, not a history gap.
        //
        // A rate that never decays for a given cause means ops are parked
        // indefinitely, not retrying successfully.
        let at_cut_undecidable = Family::<AtCutUndecidableLabels, Counter>::default();
        group_store_registry.register(
            "at_cut_undecidable_total",
            "Apply-time at-cut authority questions the gate refused to decide, \
             sliced by cause. Transient (scope_unfed / heads_missing / \
             ancestry_gap) decays as sync catches up; log_truncated is permanent \
             and means that node's governance DAG has stopped advancing",
            at_cut_undecidable.clone(),
        );

        let _ = GROUP_STORE_METRICS.set(GroupStoreMetricSink {
            namespace_retry_events: namespace_retry_events.clone(),
            namespace_decode_events: namespace_decode_events.clone(),
            membership_policy_rejections: membership_policy_rejections.clone(),
            governance_publish_mesh_peers: governance_publish_mesh_peers.clone(),
            governance_handler_delivery_total: governance_handler_delivery_total.clone(),
            governance_handler_delivery_seconds: governance_handler_delivery_seconds.clone(),
            self_purge_failures: self_purge_failures.clone(),
            self_purge_reconcile: self_purge_reconcile.clone(),
            self_purge_events_dropped: self_purge_events_dropped.clone(),
            at_cut_undecidable: at_cut_undecidable.clone(),
        });

        Self {
            execution_count,
            execution_duration,
            context_cache_hits,
            context_cache_misses,
            context_cache_size,
            application_cache_size,
        }
    }
}

pub(crate) fn record_namespace_retry_event(status: &str) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    metrics
        .namespace_retry_events
        .get_or_create(&NamespaceRetryLabels {
            status: status.to_owned(),
        })
        .inc();
}

pub(crate) fn record_namespace_decode_fallback(kind: &str) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    metrics
        .namespace_decode_events
        .get_or_create(&NamespaceDecodeLabels {
            status: "fallback".to_owned(),
            kind: kind.to_owned(),
        })
        .inc();
}

pub(crate) fn record_namespace_decode_invalid(kind: &str) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    metrics
        .namespace_decode_events
        .get_or_create(&NamespaceDecodeLabels {
            status: "invalid".to_owned(),
            kind: kind.to_owned(),
        })
        .inc();
}

pub(crate) fn record_membership_policy_rejection(reason: &str) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    metrics
        .membership_policy_rejections
        .get_or_create(&MembershipPolicyLabels {
            reason: reason.to_owned(),
        })
        .inc();
}

pub(crate) fn record_governance_publish_mesh_peers(op_kind: &str, mesh_count: usize) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    metrics
        .governance_publish_mesh_peers
        .get_or_create(&GovernancePublishLabels {
            op_kind: op_kind.to_owned(),
        })
        .observe(mesh_count as f64);
}

/// Record a handler-level governance op delivery outcome.
///
/// Called from [`crate::governance_broadcast::observe_handler_delivery`] so
/// every API endpoint that publishes a governance op contributes to the
/// `governance_handler_delivery_total` and `governance_handler_delivery_seconds`
/// series with consistent labels. `outcome` is `"acked"` when at least one
/// valid ack arrived within `op_timeout`, `"empty"` otherwise.
pub fn record_governance_handler_delivery(
    handler: &str,
    op_kind: &str,
    outcome: &str,
    elapsed_ms: u64,
) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    let labels = GovernanceHandlerDeliveryLabels {
        handler: handler.to_owned(),
        op_kind: op_kind.to_owned(),
        outcome: outcome.to_owned(),
    };
    metrics
        .governance_handler_delivery_total
        .get_or_create(&labels)
        .inc();
    metrics
        .governance_handler_delivery_seconds
        .get_or_create(&labels)
        .observe(elapsed_ms as f64 / 1000.0);
}

/// Which self-purge branch hit the failure. Stringly-typed labels are
/// error-prone, so the call sites in `calimero-context`'s `self_purge`
/// module pass these enums instead of raw `&str`.
#[derive(Clone, Copy, Debug)]
pub enum PurgeBranch {
    /// A subgroup-only purge (`purge_subgroup_for_self`).
    Subgroup,
    /// A namespace-root cascade (`cascade_namespace_state`).
    Namespace,
}

impl PurgeBranch {
    fn as_label(self) -> &'static str {
        match self {
            PurgeBranch::Subgroup => "subgroup",
            PurgeBranch::Namespace => "namespace",
        }
    }
}

/// The failure class for a self-purge step.
#[derive(Clone, Copy, Debug)]
pub enum PurgeFailureClass {
    /// The security-critical `delete_group_local_rows` step failed —
    /// the group's encryption keys may linger. Load-bearing.
    GroupRows,
    /// A best-effort dead-pointer cleanup step failed (context-index
    /// unregister, parent-edge read, or tree-edge delete). Non-security.
    ContextCleanup,
}

impl PurgeFailureClass {
    fn as_label(self) -> &'static str {
        match self {
            PurgeFailureClass::GroupRows => "group_rows",
            PurgeFailureClass::ContextCleanup => "context_cleanup",
        }
    }
}

/// Record a self-purge cleanup failure, labeled by branch and failure
/// class. No-op until [`Metrics::new`] has installed the process-global
/// sink (e.g. on a node started without a Prometheus registry).
///
/// Called from `calimero-context`'s `self_purge` module on the relevant
/// failure paths (#2686).
pub fn record_purge_failure(branch: PurgeBranch, class: PurgeFailureClass) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    metrics
        .self_purge_failures
        .get_or_create(&SelfPurgeFailureLabels {
            branch: branch.as_label(),
            class: class.as_label(),
        })
        .inc();
}

/// The outcome of processing a single marked namespace in the self-purge
/// startup reconcile sweep (#2686). Exactly one is recorded per marked
/// namespace the sweep visits. Distinct from [`PurgeFailureClass`]: these
/// are sweep-level outcomes, NOT purge-step (delete) failures — read-error
/// uncertainty lands in [`ReconcileOutcome::Skipped`], not in
/// `self_purge_failures_total`.
#[derive(Clone, Copy, Debug)]
pub enum ReconcileOutcome {
    /// Marker present + still-evicted, and the namespace purge fully
    /// completed.
    Reconciled,
    /// Marker present + still-evicted, but the purge returned false
    /// (row-purge failure); the marker is kept for the next restart.
    Retained,
    /// The marker was stale (already purged / re-admitted) and the clear
    /// succeeded.
    ClearedStale,
    /// The marker was stale but the clear itself failed (re-evaluated and
    /// retried on the next restart).
    StaleClearFailed,
    /// A read error made the decision uncertain — the marker is kept (never
    /// purge on uncertainty) for the next restart.
    Skipped,
}

impl ReconcileOutcome {
    fn as_label(self) -> &'static str {
        match self {
            ReconcileOutcome::Reconciled => "reconciled",
            ReconcileOutcome::Retained => "retained",
            ReconcileOutcome::ClearedStale => "cleared_stale",
            ReconcileOutcome::StaleClearFailed => "stale_clear_failed",
            ReconcileOutcome::Skipped => "skipped",
        }
    }
}

/// Record a self-purge reconcile-sweep outcome for one marked namespace.
/// No-op until [`Metrics::new`] has installed the process-global sink.
///
/// Called from `calimero-context`'s `self_purge::reconcile_sweep` (#2686),
/// exactly once per marked namespace processed.
pub fn record_reconcile_outcome(outcome: ReconcileOutcome) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    metrics
        .self_purge_reconcile
        .get_or_create(&SelfPurgeReconcileLabels {
            outcome: outcome.as_label(),
        })
        .inc();
}

/// Record `n` self-purge op-events dropped by the broadcast `Lagged` arm
/// (#2686). No-op until [`Metrics::new`] has installed the process-global
/// sink. Called from `calimero-context`'s `self_purge::run` `Lagged` arm
/// with the broadcast-reported skip count.
pub fn record_events_dropped(n: u64) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    metrics.self_purge_events_dropped.inc_by(n);
}

/// Why an apply-time at-cut authority question could not be decided — the
/// `cause` label of `at_cut_undecidable_total`.
///
/// Deciding authority at a cut asks: was this signer authorized as of the op's
/// own parents? That question has one answer on every replica, which is why the
/// gate must not substitute the replica-local question ("is this signer
/// authorized right now, on me?") when it cannot resolve the cut. It refuses
/// instead, and this records why.
///
/// The variants are ordered from "will fix itself" to "will not": the first four
/// describe a history gap, of which only [`Self::LogTruncated`] is permanent.
/// Distinguishing them is the whole point — before this, every refusal looked
/// identical, so a node parked forever was indistinguishable from a node parked
/// for the next two seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndecidableCause {
    /// The projection holds no ops at all for this scope — never fed, or fed
    /// only after this op arrived. Transient in the normal case (startup /
    /// first contact with a namespace).
    ScopeUnfed,
    /// The log is non-empty but one or more of the cut's own head ids are absent
    /// from it: the op outran its parents. Transient — the ordinary
    /// arrive-before-your-parents case that sync then repairs.
    HeadsMissing,
    /// Every cited head is present, but the walk hit a missing ancestor deeper
    /// in. The "partial frontier" the backfill walk deliberately tolerates.
    /// Usually transient, but NOT necessarily: a node that received state by
    /// snapshot rather than by replaying the DAG has no such history coming.
    AncestryGap,
    /// The retained op-log for this scope has been truncated — evicted by the
    /// live-log bound, or cut short by the backfill walk cap — so it cannot
    /// reach the cut and never will again. **Permanent.** The op parks forever
    /// and that namespace's governance DAG stops advancing on this node.
    LogTruncated,
    /// The ephemeral fold could not be built at all (governance head unreadable
    /// — a store fault). Not a history gap.
    FoldUnavailable,
    /// The group's namespace could not be resolved (a store or mapping fault).
    /// Not a history gap.
    NamespaceUnresolved,
}

impl UndecidableCause {
    fn as_label(self) -> &'static str {
        match self {
            UndecidableCause::ScopeUnfed => "scope_unfed",
            UndecidableCause::HeadsMissing => "heads_missing",
            UndecidableCause::AncestryGap => "ancestry_gap",
            UndecidableCause::LogTruncated => "log_truncated",
            UndecidableCause::FoldUnavailable => "fold_unavailable",
            UndecidableCause::NamespaceUnresolved => "namespace_unresolved",
        }
    }

    /// Can the refusal this cause describes resolve on its own, once sync
    /// delivers more history?
    ///
    /// [`Self::LogTruncated`] cannot: the history is gone from the retained
    /// window, so no amount of sync brings the walk back within reach. Exposed
    /// so the recording site can log a truncation loudly (it needs an operator)
    /// while leaving the self-healing cases quiet, and so a future caller can
    /// branch on permanence without re-deriving the classification.
    #[must_use]
    pub fn is_transient(self) -> bool {
        !matches!(self, UndecidableCause::LogTruncated)
    }
}

/// Record one apply-time at-cut authority refusal, labeled by cause. No-op
/// until [`Metrics::new`] has installed the process-global sink (e.g. a node
/// started without a Prometheus registry).
///
/// Called from `calimero-context`'s at-cut resolution funnel — once per gate
/// decision that refuses, NOT once per predicate: a single op consults several
/// predicates (admin, capability, last-admin) over one fold, and counting each
/// would inflate the rate by a factor that varies with the op's kind.
pub fn record_at_cut_undecidable(cause: UndecidableCause) {
    let Some(metrics) = GROUP_STORE_METRICS.get() else {
        return;
    };
    metrics
        .at_cut_undecidable
        .get_or_create(&AtCutUndecidableLabels {
            cause: cause.as_label(),
        })
        .inc();
}

#[cfg(test)]
mod tests {
    use prometheus_client::encoding::text::encode;

    use super::*;

    /// The `context.cache.*` series register under the expected names and the
    /// counters/gauges round-trip through the Prometheus text encoder. Exercises
    /// the same `inc()`/`set()` calls that `ContextManager` makes on the cache
    /// hit/miss and periodic-log paths.
    #[test]
    fn context_cache_metrics_register_and_encode() {
        let mut registry = Registry::default();
        let metrics = Metrics::new(&mut registry);

        metrics.context_cache_hits.inc();
        metrics.context_cache_hits.inc();
        metrics.context_cache_misses.inc();
        metrics.context_cache_size.set(7);
        metrics.application_cache_size.set(3);

        let mut out = String::new();
        encode(&mut out, &registry).expect("encode registry");

        assert!(
            out.contains("context_cache_hits_total 2"),
            "missing hit counter:\n{out}"
        );
        assert!(
            out.contains("context_cache_misses_total 1"),
            "missing miss counter:\n{out}"
        );
        assert!(
            out.contains("context_cache_size 7"),
            "missing context size gauge:\n{out}"
        );
        assert!(
            out.contains("context_cache_application_size 3"),
            "missing application size gauge:\n{out}"
        );
    }

    /// Every [`UndecidableCause`] round-trips to a distinct `cause` label, and
    /// exactly one variant is classified as permanent.
    ///
    /// The distinctness assertion is the point: two causes collapsing to one
    /// label would silently merge a permanent stall into a transient series, and
    /// the whole reason this metric exists is to tell those apart. Built against
    /// a local family rather than the process-global sink for the reason
    /// `self_purge_failures_register_and_encode` documents below.
    #[test]
    fn undecidable_causes_encode_distinctly_and_only_truncation_is_permanent() {
        let all = [
            UndecidableCause::ScopeUnfed,
            UndecidableCause::HeadsMissing,
            UndecidableCause::AncestryGap,
            UndecidableCause::LogTruncated,
            UndecidableCause::FoldUnavailable,
            UndecidableCause::NamespaceUnresolved,
        ];

        let labels: std::collections::HashSet<&str> = all.iter().map(|c| c.as_label()).collect();
        assert_eq!(
            labels.len(),
            all.len(),
            "two causes share a label, so one would hide inside the other's series",
        );

        let permanent: Vec<&str> = all
            .iter()
            .filter(|c| !c.is_transient())
            .map(|c| c.as_label())
            .collect();
        assert_eq!(
            permanent,
            vec!["log_truncated"],
            "only a truncated log is unrecoverable; the rest clear once sync \
             delivers the missing history",
        );

        let mut registry = Registry::default();
        let family = Family::<AtCutUndecidableLabels, Counter>::default();
        registry.register(
            "at_cut_undecidable",
            "At-cut authority refusals by cause",
            family.clone(),
        );
        for cause in all {
            family
                .get_or_create(&AtCutUndecidableLabels {
                    cause: cause.as_label(),
                })
                .inc();
        }

        let mut out = String::new();
        encode(&mut out, &registry).expect("encode registry");
        for cause in all {
            let series = format!(
                "at_cut_undecidable_total{{cause=\"{}\"}} 1",
                cause.as_label()
            );
            assert!(out.contains(&series), "missing {series}:\n{out}");
        }
    }

    /// The `self_purge_failures_total` family registers against a fresh
    /// registry and the recorded branch/class labels round-trip through the
    /// text encoder.
    ///
    /// We build the family + registry locally instead of going through the
    /// process-global `GROUP_STORE_METRICS` sink: that sink is a
    /// `OnceLock` another test in the same binary may have already set, so
    /// `record_purge_failure` is not guaranteed to target *this*
    /// registry's `Family`. The label-building logic
    /// (`PurgeBranch::as_label` / `PurgeFailureClass::as_label`) is what we
    /// assert on; the no-op-without-sink behaviour of the public recorder
    /// is covered by the early-return and exercised by the self_purge unit
    /// tests in `calimero-context`.
    #[test]
    fn self_purge_failures_register_and_encode() {
        let mut registry = Registry::default();
        let family = Family::<SelfPurgeFailureLabels, Counter>::default();
        registry.register(
            "self_purge_failures",
            "Self-purge cleanup failures by branch and class",
            family.clone(),
        );

        for (branch, class) in [
            (PurgeBranch::Namespace, PurgeFailureClass::GroupRows),
            (PurgeBranch::Subgroup, PurgeFailureClass::ContextCleanup),
        ] {
            family
                .get_or_create(&SelfPurgeFailureLabels {
                    branch: branch.as_label(),
                    class: class.as_label(),
                })
                .inc();
        }

        let mut out = String::new();
        encode(&mut out, &registry).expect("encode registry");

        assert!(
            out.contains("branch=\"namespace\"") && out.contains("class=\"group_rows\""),
            "missing group_rows/namespace labels:\n{out}"
        );
        assert!(
            out.contains("branch=\"subgroup\"") && out.contains("class=\"context_cleanup\""),
            "missing context_cleanup/subgroup labels:\n{out}"
        );
        // And the public recorder must not panic whether or not the global
        // sink is installed.
        record_purge_failure(PurgeBranch::Subgroup, PurgeFailureClass::GroupRows);
    }

    /// The `self_purge_reconcile_total` family registers against a fresh
    /// registry and ALL `outcome` labels round-trip through the text encoder.
    ///
    /// As with `self_purge_failures_register_and_encode`, we build the family
    /// + registry locally rather than going through the process-global
    ///   `GROUP_STORE_METRICS` sink (a `OnceLock` another test may have set), so
    ///   the assertion targets *this* registry. The label-building logic
    ///   (`ReconcileOutcome::as_label`) is what we assert on; the
    ///   no-op-without-sink behaviour of the public recorder is covered by the
    ///   early-return and exercised below.
    #[test]
    fn self_purge_reconcile_register_and_encode() {
        let mut registry = Registry::default();
        let family = Family::<SelfPurgeReconcileLabels, Counter>::default();
        registry.register(
            "self_purge_reconcile",
            "Self-purge reconcile-sweep outcomes by outcome",
            family.clone(),
        );

        for outcome in [
            ReconcileOutcome::Reconciled,
            ReconcileOutcome::Retained,
            ReconcileOutcome::ClearedStale,
            ReconcileOutcome::StaleClearFailed,
            ReconcileOutcome::Skipped,
        ] {
            family
                .get_or_create(&SelfPurgeReconcileLabels {
                    outcome: outcome.as_label(),
                })
                .inc();
        }

        let mut out = String::new();
        encode(&mut out, &registry).expect("encode registry");

        for label in [
            "reconciled",
            "retained",
            "cleared_stale",
            "stale_clear_failed",
            "skipped",
        ] {
            assert!(
                out.contains(&format!("outcome=\"{label}\"")),
                "missing reconcile outcome label {label}:\n{out}"
            );
        }

        // The public recorder must not panic whether or not the global sink
        // is installed.
        record_reconcile_outcome(ReconcileOutcome::Reconciled);
        record_reconcile_outcome(ReconcileOutcome::StaleClearFailed);
    }

    /// The `self_purge_events_dropped_total` counter registers against a
    /// fresh registry and `inc_by` round-trips through the text encoder.
    /// Also asserts the public recorder is a safe no-op without/with a sink.
    #[test]
    fn self_purge_events_dropped_register_and_encode() {
        let mut registry = Registry::default();
        let counter = Counter::<u64>::default();
        registry.register(
            "self_purge_events_dropped",
            "Self-purge op-events dropped by the broadcast Lagged arm",
            counter.clone(),
        );

        counter.inc_by(5);

        let mut out = String::new();
        encode(&mut out, &registry).expect("encode registry");
        assert!(
            out.contains("self_purge_events_dropped_total 5"),
            "missing dropped-events counter:\n{out}"
        );

        // Public recorder must not panic whether or not the global sink is
        // installed (including the n=0 edge).
        record_events_dropped(0);
        record_events_dropped(3);
    }
}
