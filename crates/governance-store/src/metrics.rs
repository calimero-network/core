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
///   - `"subgroup"`: a subgroup-only purge ([`purge_subgroup_for_self`]).
///   - `"namespace"`: a namespace-root cascade ([`cascade_namespace_state`]).
///
/// `class` is the failure class:
///   - `"signing_key"`: the security-critical `delete_group_local_rows`
///     step failed, so private signing-key material may linger on disk.
///     This is the load-bearing failure — it keeps the `NamespaceIdentity`
///     anchor + gossipsub subscription alive for the planned reconcile
///     sweep (#2721).
///   - `"context_cleanup"`: a best-effort dead-pointer cleanup step
///     (context-index unregister, parent-edge read, or tree-edge delete)
///     failed. Non-security: the orphaned rows point at soon-to-be / now
///     deleted groups. Namespace deletion + unsubscribe still proceed.
///
/// [`purge_subgroup_for_self`]: ../../calimero_context/self_purge/index.html
/// [`cascade_namespace_state`]: ../../calimero_context/self_purge/index.html
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct SelfPurgeFailureLabels {
    pub branch: String,
    pub class: String,
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
        // branch (subgroup / namespace) and failure-class (signing_key /
        // context_cleanup). The `class="signing_key"` series is the
        // security-relevant one — a nonzero rate means forward-secrecy
        // residue lingered on a node's own disk pending the reconcile
        // sweep (#2721). `class="context_cleanup"` is a best-effort
        // dead-pointer leak and is informational only.
        let self_purge_failures = Family::<SelfPurgeFailureLabels, Counter>::default();
        group_store_registry.register(
            "self_purge_failures_total",
            "Self-purge (TEE self-eviction) local-state cleanup failures, \
             sliced by branch (subgroup / namespace) and failure-class \
             (signing_key / context_cleanup)",
            self_purge_failures.clone(),
        );

        let _ = GROUP_STORE_METRICS.set(GroupStoreMetricSink {
            namespace_retry_events: namespace_retry_events.clone(),
            namespace_decode_events: namespace_decode_events.clone(),
            membership_policy_rejections: membership_policy_rejections.clone(),
            governance_publish_mesh_peers: governance_publish_mesh_peers.clone(),
            governance_handler_delivery_total: governance_handler_delivery_total.clone(),
            governance_handler_delivery_seconds: governance_handler_delivery_seconds.clone(),
            self_purge_failures: self_purge_failures.clone(),
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
    /// private signing-key material may linger. Load-bearing.
    SigningKey,
    /// A best-effort dead-pointer cleanup step failed (context-index
    /// unregister, parent-edge read, or tree-edge delete). Non-security.
    ContextCleanup,
}

impl PurgeFailureClass {
    fn as_label(self) -> &'static str {
        match self {
            PurgeFailureClass::SigningKey => "signing_key",
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
            branch: branch.as_label().to_owned(),
            class: class.as_label().to_owned(),
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
            (PurgeBranch::Namespace, PurgeFailureClass::SigningKey),
            (PurgeBranch::Subgroup, PurgeFailureClass::ContextCleanup),
        ] {
            family
                .get_or_create(&SelfPurgeFailureLabels {
                    branch: branch.as_label().to_owned(),
                    class: class.as_label().to_owned(),
                })
                .inc();
        }

        let mut out = String::new();
        encode(&mut out, &registry).expect("encode registry");

        assert!(
            out.contains("branch=\"namespace\"") && out.contains("class=\"signing_key\""),
            "missing signing_key/namespace labels:\n{out}"
        );
        assert!(
            out.contains("branch=\"subgroup\"") && out.contains("class=\"context_cleanup\""),
            "missing context_cleanup/subgroup labels:\n{out}"
        );
        // And the public recorder must not panic whether or not the global
        // sink is installed.
        record_purge_failure(PurgeBranch::Subgroup, PurgeFailureClass::SigningKey);
    }
}
