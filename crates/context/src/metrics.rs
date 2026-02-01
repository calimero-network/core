use std::sync::atomic::AtomicI64;

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;

#[derive(Clone, Debug)]
pub(crate) struct Metrics {
    pub(crate) execution_count: Family<ExecutionLabels, Gauge>,
    pub(crate) execution_duration: Family<ExecutionLabels, Histogram>,
    /// Number of executions currently waiting for a permit (queued).
    pub(crate) queued_executions: Gauge<i64, AtomicI64>,
    /// Number of executions currently running (holding a permit).
    pub(crate) active_executions: Gauge<i64, AtomicI64>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct ExecutionLabels {
    pub(crate) context_id: String,
    pub(crate) method: String,
    pub(crate) status: String,
}

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

        let queued_executions = Gauge::<i64, AtomicI64>::default();
        runtime_registry.register(
            "queued_executions",
            "Number of WASM executions waiting for a permit",
            queued_executions.clone(),
        );

        let active_executions = Gauge::<i64, AtomicI64>::default();
        runtime_registry.register(
            "active_executions",
            "Number of WASM executions currently running",
            active_executions.clone(),
        );

        Self {
            execution_count,
            execution_duration,
            queued_executions,
            active_executions,
        }
    }
}
