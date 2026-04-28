use std::sync::OnceLock;

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;

#[derive(Clone, Debug)]
pub(crate) struct Metrics {
    pub(crate) execution_count: Family<ExecutionLabels, Gauge>,
    pub(crate) execution_duration: Family<ExecutionLabels, Histogram>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct ExecutionLabels {
    pub(crate) context_id: String,
    pub(crate) method: String,
    pub(crate) status: String,
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

#[derive(Clone, Debug)]
struct GroupStoreMetricSink {
    namespace_retry_events: Family<NamespaceRetryLabels, Counter>,
    namespace_decode_events: Family<NamespaceDecodeLabels, Counter>,
    membership_policy_rejections: Family<MembershipPolicyLabels, Counter>,
    governance_publish_mesh_peers: Family<GovernancePublishLabels, Histogram>,
    governance_handler_delivery_total: Family<GovernanceHandlerDeliveryLabels, Counter>,
    governance_handler_delivery_seconds: Family<GovernanceHandlerDeliveryLabels, Histogram>,
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

        let _ = GROUP_STORE_METRICS.set(GroupStoreMetricSink {
            namespace_retry_events: namespace_retry_events.clone(),
            namespace_decode_events: namespace_decode_events.clone(),
            membership_policy_rejections: membership_policy_rejections.clone(),
            governance_publish_mesh_peers: governance_publish_mesh_peers.clone(),
            governance_handler_delivery_total: governance_handler_delivery_total.clone(),
            governance_handler_delivery_seconds: governance_handler_delivery_seconds.clone(),
        });

        Self {
            execution_count,
            execution_duration,
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
