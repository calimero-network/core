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

#[derive(Clone, Debug)]
struct GroupStoreMetricSink {
    namespace_retry_events: Family<NamespaceRetryLabels, Counter>,
    namespace_decode_events: Family<NamespaceDecodeLabels, Counter>,
    membership_policy_rejections: Family<MembershipPolicyLabels, Counter>,
    governance_publish_mesh_peers: Family<GovernancePublishLabels, Histogram>,
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

        let _ = GROUP_STORE_METRICS.set(GroupStoreMetricSink {
            namespace_retry_events: namespace_retry_events.clone(),
            namespace_decode_events: namespace_decode_events.clone(),
            membership_policy_rejections: membership_policy_rejections.clone(),
            governance_publish_mesh_peers: governance_publish_mesh_peers.clone(),
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

/// Stable label for `op_kind` on `governance_publish_mesh_peers_at_publish`.
///
/// `NamespaceOp` and `RootOp` are exhaustive (no `#[non_exhaustive]`); a new
/// variant added upstream will surface as a compile error here so the metric
/// label set stays in sync with the wire format.
pub(crate) fn op_kind_label_namespace(
    op: &calimero_context_client::local_governance::NamespaceOp,
) -> &'static str {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp};
    match op {
        NamespaceOp::Root(RootOp::MemberJoined { .. }) => "member_joined",
        NamespaceOp::Root(RootOp::KeyDelivery { .. }) => "key_delivery",
        NamespaceOp::Root(RootOp::PolicyUpdated { .. }) => "policy_updated",
        NamespaceOp::Root(RootOp::GroupCreated { .. }) => "group_created",
        NamespaceOp::Root(RootOp::GroupReparented { .. }) => "group_reparented",
        NamespaceOp::Root(RootOp::GroupDeleted { .. }) => "group_deleted",
        NamespaceOp::Root(RootOp::AdminChanged { .. }) => "admin_changed",
        NamespaceOp::Group { .. } => "group_op",
    }
}

/// Stable label for `op_kind` when the inner cleartext `GroupOp` is known
/// at the call site (i.e. before encryption inside the namespace publish).
pub(crate) fn op_kind_label_group(
    op: &calimero_context_client::local_governance::GroupOp,
) -> &'static str {
    use calimero_context_client::local_governance::GroupOp;
    match op {
        GroupOp::MemberAdded { .. } => "member_added",
        GroupOp::MemberRemoved { .. } => "member_removed",
        GroupOp::MemberRoleSet { .. } => "member_role_set",
        GroupOp::MemberCapabilitySet { .. } => "member_capability_set",
        GroupOp::DefaultCapabilitiesSet { .. } => "default_capabilities_set",
        GroupOp::DefaultVisibilitySet { .. } => "default_visibility_set",
        GroupOp::UpgradePolicySet { .. } => "upgrade_policy_set",
        GroupOp::TargetApplicationSet { .. } => "target_application_set",
        GroupOp::ContextRegistered { .. } => "context_registered",
        GroupOp::ContextDetached { .. } => "context_detached",
        GroupOp::ContextAliasSet { .. } => "context_alias_set",
        GroupOp::MemberAliasSet { .. } => "member_alias_set",
        GroupOp::GroupAliasSet { .. } => "group_alias_set",
        GroupOp::GroupDelete => "group_delete",
        GroupOp::GroupMigrationSet { .. } => "group_migration_set",
        GroupOp::ContextCapabilityGranted { .. } => "context_capability_granted",
        GroupOp::ContextCapabilityRevoked { .. } => "context_capability_revoked",
        GroupOp::TeeAdmissionPolicySet { .. } => "tee_admission_policy_set",
        GroupOp::MemberJoinedViaTeeAttestation { .. } => "member_joined_via_tee",
        GroupOp::MemberSetAutoFollow { .. } => "member_set_auto_follow",
        GroupOp::Noop => "noop",
        _ => "other",
    }
}
