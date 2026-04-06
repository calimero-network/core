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

#[derive(Clone, Debug)]
struct GroupStoreMetricSink {
    namespace_retry_events: Family<NamespaceRetryLabels, Counter>,
    namespace_decode_events: Family<NamespaceDecodeLabels, Counter>,
    membership_policy_rejections: Family<MembershipPolicyLabels, Counter>,
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

        let _ = GROUP_STORE_METRICS.set(GroupStoreMetricSink {
            namespace_retry_events: namespace_retry_events.clone(),
            namespace_decode_events: namespace_decode_events.clone(),
            membership_policy_rejections: membership_policy_rejections.clone(),
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
