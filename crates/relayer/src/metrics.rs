use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;

/// Metrics for the relayer service
#[derive(Clone)]
pub struct RelayerMetrics {
    // HTTP metrics
    pub http_requests_total: Family<HttpLabels, Counter>,
    pub http_request_duration_seconds: Family<HttpLabels, Histogram>,
    pub http_request_size_bytes: Family<HttpRequestLabels, Histogram>,
    pub http_response_size_bytes: Family<HttpLabels, Histogram>,
    pub http_requests_active: Gauge,

    // Protocol metrics
    pub protocol_requests_total: Family<ProtocolLabels, Counter>,
    pub protocol_request_duration_seconds: Family<ProtocolStatusLabels, Histogram>,
    pub protocol_errors_total: Family<ProtocolErrorLabels, Counter>,

    // Queue metrics
    pub queue_depth: Gauge,

    // Mock relayer metrics
    pub mock_relayer_operations_total: Family<MockRelayerLabels, Counter>,
}

/// Labels for HTTP requests
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct HttpLabels {
    pub method: String,
    pub path: String,
    pub status_code: String,
}

/// Labels for HTTP request size (no status code since it's measured before processing)
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct HttpRequestLabels {
    pub method: String,
    pub path: String,
}

/// Labels for protocol requests
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ProtocolLabels {
    pub protocol: String,
}

/// Labels for protocol requests with status
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ProtocolStatusLabels {
    pub protocol: String,
    pub status: String,
}

/// Labels for protocol errors
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ProtocolErrorLabels {
    pub protocol: String,
    pub error_type: String,
}

/// Labels for mock relayer operations
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MockRelayerLabels {
    pub method: String,
}

impl RelayerMetrics {
    /// Create new metrics and register them with the given registry
    pub fn new(registry: &mut Registry) -> Self {
        let relayer_registry = registry.sub_registry_with_prefix("mero_relayer");

        // HTTP metrics
        let http_registry = relayer_registry.sub_registry_with_prefix("http");

        let http_requests_total = Family::<HttpLabels, Counter>::default();
        http_registry.register(
            "requests",
            "Total number of HTTP requests",
            http_requests_total.clone(),
        );

        let http_request_duration_seconds =
            Family::<HttpLabels, Histogram>::new_with_constructor(|| {
                Histogram::new(exponential_buckets(0.001, 2.0, 15))
            });
        http_registry.register(
            "request_duration_seconds",
            "HTTP request duration in seconds",
            http_request_duration_seconds.clone(),
        );

        let http_request_size_bytes =
            Family::<HttpRequestLabels, Histogram>::new_with_constructor(|| {
                Histogram::new(exponential_buckets(64.0, 2.0, 15))
            });
        http_registry.register(
            "request_size_bytes",
            "HTTP request body size in bytes",
            http_request_size_bytes.clone(),
        );

        let http_response_size_bytes =
            Family::<HttpLabels, Histogram>::new_with_constructor(|| {
                Histogram::new(exponential_buckets(64.0, 2.0, 15))
            });
        http_registry.register(
            "response_size_bytes",
            "HTTP response body size in bytes",
            http_response_size_bytes.clone(),
        );

        let http_requests_active = Gauge::default();
        http_registry.register(
            "requests_active",
            "Number of HTTP requests currently being processed",
            http_requests_active.clone(),
        );

        // Protocol metrics
        let protocol_registry = relayer_registry.sub_registry_with_prefix("protocol");

        let protocol_requests_total = Family::<ProtocolLabels, Counter>::default();
        protocol_registry.register(
            "requests",
            "Total number of protocol requests",
            protocol_requests_total.clone(),
        );

        let protocol_request_duration_seconds =
            Family::<ProtocolStatusLabels, Histogram>::new_with_constructor(|| {
                Histogram::new(exponential_buckets(0.001, 2.0, 15))
            });
        protocol_registry.register(
            "request_duration_seconds",
            "Protocol request duration in seconds",
            protocol_request_duration_seconds.clone(),
        );

        let protocol_errors_total = Family::<ProtocolErrorLabels, Counter>::default();
        protocol_registry.register(
            "errors",
            "Total number of protocol errors",
            protocol_errors_total.clone(),
        );

        // Queue metrics
        let queue_depth = Gauge::default();
        relayer_registry.register(
            "queue_depth",
            "Current depth of the request queue",
            queue_depth.clone(),
        );

        // Mock relayer metrics
        let mock_registry = relayer_registry.sub_registry_with_prefix("mock");

        let mock_relayer_operations_total = Family::<MockRelayerLabels, Counter>::default();
        mock_registry.register(
            "operations",
            "Total number of mock relayer operations by method",
            mock_relayer_operations_total.clone(),
        );

        Self {
            http_requests_total,
            http_request_duration_seconds,
            http_request_size_bytes,
            http_response_size_bytes,
            http_requests_active,
            protocol_requests_total,
            protocol_request_duration_seconds,
            protocol_errors_total,
            queue_depth,
            mock_relayer_operations_total,
        }
    }

    /// Increment protocol request counter
    pub fn inc_protocol_requests(&self, protocol: &str) {
        self.protocol_requests_total
            .get_or_create(&ProtocolLabels {
                protocol: protocol.to_string(),
            })
            .inc();
    }

    /// Record protocol request duration
    pub fn record_protocol_duration(&self, protocol: &str, status: &str, duration: f64) {
        self.protocol_request_duration_seconds
            .get_or_create(&ProtocolStatusLabels {
                protocol: protocol.to_string(),
                status: status.to_string(),
            })
            .observe(duration);
    }

    /// Increment protocol error counter
    pub fn inc_protocol_errors(&self, protocol: &str, error_type: &str) {
        self.protocol_errors_total
            .get_or_create(&ProtocolErrorLabels {
                protocol: protocol.to_string(),
                error_type: error_type.to_string(),
            })
            .inc();
    }

    /// Increment mock relayer operations counter
    pub fn inc_mock_operations(&self, method: &str) {
        self.mock_relayer_operations_total
            .get_or_create(&MockRelayerLabels {
                method: method.to_string(),
            })
            .inc();
    }

    /// Set queue depth gauge
    pub fn set_queue_depth(&self, depth: i64) {
        self.queue_depth.set(depth);
    }

    /// Record HTTP request metrics
    pub fn record_http_request(&self, method: &str, path: &str, status_code: &str, duration: f64) {
        let labels = HttpLabels {
            method: method.to_string(),
            path: path.to_string(),
            status_code: status_code.to_string(),
        };
        self.http_requests_total.get_or_create(&labels).inc();
        self.http_request_duration_seconds
            .get_or_create(&labels)
            .observe(duration);
    }

    /// Record HTTP request size
    pub fn record_http_request_size(&self, method: &str, path: &str, size: f64) {
        self.http_request_size_bytes
            .get_or_create(&HttpRequestLabels {
                method: method.to_string(),
                path: path.to_string(),
            })
            .observe(size);
    }

    /// Record HTTP response size
    pub fn record_http_response_size(
        &self,
        method: &str,
        path: &str,
        status_code: &str,
        size: f64,
    ) {
        self.http_response_size_bytes
            .get_or_create(&HttpLabels {
                method: method.to_string(),
                path: path.to_string(),
                status_code: status_code.to_string(),
            })
            .observe(size);
    }

    /// Increment active HTTP requests
    pub fn inc_http_active(&self) {
        self.http_requests_active.inc();
    }

    /// Decrement active HTTP requests
    pub fn dec_http_active(&self) {
        self.http_requests_active.dec();
    }
}
