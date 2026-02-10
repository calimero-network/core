//! Dedicated channel for NetworkEvent processing.
//!
//! This module provides a reliable message channel between NetworkManager (Arbiter A)
//! and the event processing loop (Tokio runtime), bypassing Actix's cross-arbiter
//! message passing which has reliability issues under load.
//!
//! ## Why This Exists
//!
//! The previous architecture used `LazyRecipient<NetworkEvent>` to send messages
//! from NetworkManager to NodeManager across different Actix arbiters. Under high
//! load (e.g., 40+ messages in ~700ms), messages were silently lost due to:
//! - Cross-arbiter scheduling issues
//! - Competition with spawned futures in the receiving actor
//!
//! This channel provides:
//! - **Guaranteed delivery** or explicit error (never silent loss)
//! - **Backpressure visibility** via metrics and logging
//! - **Independent processing** from Actix arbiter scheduling

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_network_primitives::messages::{NetworkEvent, NetworkEventDispatcher};
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Configuration for the network event channel.
#[derive(Debug, Clone, Copy)]
pub struct NetworkEventChannelConfig {
    /// Maximum number of events that can be buffered.
    /// Default: 1000
    pub channel_size: usize,

    /// Log a warning when channel depth exceeds this percentage of capacity.
    /// Default: 0.8 (80%)
    pub warning_threshold: f64,

    /// Interval for logging channel statistics.
    /// Default: 30 seconds
    pub stats_log_interval: Duration,
}

impl Default for NetworkEventChannelConfig {
    fn default() -> Self {
        Self {
            channel_size: 1000,
            warning_threshold: 0.8,
            stats_log_interval: Duration::from_secs(30),
        }
    }
}

/// Labels for network event metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct EventTypeLabel {
    pub event_type: String,
}

/// Metrics for the network event channel.
#[derive(Debug, Clone)]
pub struct NetworkEventChannelMetrics {
    /// Current number of events in the channel.
    pub channel_depth: Gauge,

    /// Total events received (sent to channel).
    pub events_received: Counter,

    /// Total events processed (received from channel).
    pub events_processed: Counter,

    /// Events dropped due to full channel.
    pub events_dropped: Counter,

    /// Processing latency histogram (time from send to receive).
    pub processing_latency: Histogram,

    /// High watermark (maximum channel depth seen).
    pub high_watermark: Arc<AtomicU64>,
}

impl NetworkEventChannelMetrics {
    /// Create new metrics and register with the provided registry.
    pub fn new(registry: &mut Registry) -> Self {
        let channel_depth = Gauge::default();
        let events_received = Counter::default();
        let events_processed = Counter::default();
        let events_dropped = Counter::default();

        // Latency buckets: 100μs to 10s
        let processing_latency = Histogram::new(exponential_buckets(0.0001, 2.0, 18));

        let sub_registry = registry.sub_registry_with_prefix("network_event_channel");

        sub_registry.register(
            "depth",
            "Current number of events waiting in the channel",
            channel_depth.clone(),
        );
        sub_registry.register(
            "received_total",
            "Total number of events sent to the channel",
            events_received.clone(),
        );
        sub_registry.register(
            "processed_total",
            "Total number of events received from the channel",
            events_processed.clone(),
        );
        sub_registry.register(
            "dropped_total",
            "Number of events dropped due to full channel",
            events_dropped.clone(),
        );
        sub_registry.register(
            "processing_latency_seconds",
            "Time from event send to processing start",
            processing_latency.clone(),
        );

        Self {
            channel_depth,
            events_received,
            events_processed,
            events_dropped,
            processing_latency,
            high_watermark: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Create metrics without registry (for testing).
    #[cfg(test)]
    pub fn new_unregistered() -> Self {
        Self {
            channel_depth: Gauge::default(),
            events_received: Counter::default(),
            events_processed: Counter::default(),
            events_dropped: Counter::default(),
            processing_latency: Histogram::new(exponential_buckets(0.0001, 2.0, 18)),
            high_watermark: Arc::new(AtomicU64::new(0)),
        }
    }

    fn update_high_watermark(&self, current_depth: u64) {
        let mut current_max = self.high_watermark.load(Ordering::Relaxed);
        while current_depth > current_max {
            match self.high_watermark.compare_exchange_weak(
                current_max,
                current_depth,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }
    }
}

/// Wrapper for events with timing information.
#[derive(Debug)]
pub struct TimestampedEvent {
    pub event: NetworkEvent,
    pub enqueued_at: Instant,
}

/// Sender half of the network event channel.
///
/// This is used by NetworkManager to send events.
#[derive(Debug, Clone)]
pub struct NetworkEventSender {
    tx: mpsc::Sender<TimestampedEvent>,
    config: NetworkEventChannelConfig,
    metrics: NetworkEventChannelMetrics,
}

impl NetworkEventSender {
    /// Send an event to the channel.
    ///
    /// Uses `try_send` to avoid blocking the network thread.
    /// Returns `true` if sent successfully, `false` if channel is full.
    pub fn send(&self, event: NetworkEvent) -> bool {
        let event_type = event_type_name(&event);
        let timestamped = TimestampedEvent {
            event,
            enqueued_at: Instant::now(),
        };

        match self.tx.try_send(timestamped) {
            Ok(()) => {
                self.metrics.events_received.inc();

                // Update channel depth estimate
                let capacity = self.tx.capacity();
                let max_capacity = self.config.channel_size;
                let current_depth = max_capacity.saturating_sub(capacity) as u64;

                self.metrics.channel_depth.set(current_depth as i64);
                self.metrics.update_high_watermark(current_depth);

                // Check warning threshold
                let fill_ratio = current_depth as f64 / max_capacity as f64;
                if fill_ratio >= self.config.warning_threshold {
                    warn!(
                        current_depth,
                        max_capacity,
                        fill_percent = fill_ratio * 100.0,
                        event_type,
                        "Network event channel approaching capacity"
                    );
                }

                true
            }
            Err(mpsc::error::TrySendError::Full(dropped)) => {
                self.metrics.events_dropped.inc();
                warn!(
                    event_type,
                    channel_size = self.config.channel_size,
                    "Network event channel FULL - dropping event! \
                     This indicates the processor cannot keep up with incoming events."
                );

                // Log the dropped event details for debugging
                debug!(
                    ?dropped.event,
                    "Dropped event details"
                );

                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Channel closed - processor has shut down
                warn!(
                    event_type,
                    "Network event channel closed - processor has shut down"
                );
                false
            }
        }
    }

    /// Get the current approximate depth of the channel.
    pub fn depth(&self) -> usize {
        self.config.channel_size.saturating_sub(self.tx.capacity())
    }

    /// Check if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Implement NetworkEventDispatcher for NetworkEventSender.
///
/// This allows the sender to be used as a boxed dispatcher by NetworkManager.
impl NetworkEventDispatcher for NetworkEventSender {
    fn dispatch(&self, event: NetworkEvent) -> bool {
        self.send(event)
    }
}

/// Receiver half of the network event channel.
///
/// This is used by the event processor task.
pub struct NetworkEventReceiver {
    rx: mpsc::Receiver<TimestampedEvent>,
    metrics: NetworkEventChannelMetrics,
    last_stats_log: Instant,
    config: NetworkEventChannelConfig,
}

impl NetworkEventReceiver {
    /// Receive the next event from the channel.
    ///
    /// Returns `None` when the channel is closed and empty.
    pub async fn recv(&mut self) -> Option<NetworkEvent> {
        let timestamped = self.rx.recv().await?;

        // Record processing latency
        let latency = timestamped.enqueued_at.elapsed();
        self.metrics
            .processing_latency
            .observe(latency.as_secs_f64());
        self.metrics.events_processed.inc();

        // Update channel depth
        let remaining = self.rx.len();
        self.metrics.channel_depth.set(remaining as i64);

        // Periodic stats logging
        if self.last_stats_log.elapsed() >= self.config.stats_log_interval {
            self.log_stats();
            self.last_stats_log = Instant::now();
        }

        Some(timestamped.event)
    }

    /// Try to receive without blocking.
    pub fn try_recv(&mut self) -> Option<NetworkEvent> {
        match self.rx.try_recv() {
            Ok(timestamped) => {
                let latency = timestamped.enqueued_at.elapsed();
                self.metrics
                    .processing_latency
                    .observe(latency.as_secs_f64());
                self.metrics.events_processed.inc();
                self.metrics.channel_depth.set(self.rx.len() as i64);
                Some(timestamped.event)
            }
            Err(_) => None,
        }
    }

    /// Drain all remaining events (for graceful shutdown).
    ///
    /// Returns the number of events drained.
    pub fn drain(&mut self) -> Vec<NetworkEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.try_recv() {
            events.push(event);
        }
        if !events.is_empty() {
            info!(
                count = events.len(),
                "Drained remaining events during shutdown"
            );
        }
        events
    }

    /// Close the receiver, preventing new events from being sent.
    pub fn close(&mut self) {
        self.rx.close();
    }

    fn log_stats(&self) {
        let received = self.metrics.events_received.get();
        let processed = self.metrics.events_processed.get();
        let dropped = self.metrics.events_dropped.get();
        let high_watermark = self.metrics.high_watermark.load(Ordering::Relaxed);
        let current_depth = self.rx.len();

        info!(
            received,
            processed, dropped, current_depth, high_watermark, "Network event channel statistics"
        );
    }
}

/// Create a new network event channel.
///
/// Returns a sender (for NetworkManager) and receiver (for the processor task).
pub fn channel(
    config: NetworkEventChannelConfig,
    registry: &mut Registry,
) -> (NetworkEventSender, NetworkEventReceiver) {
    let (tx, rx) = mpsc::channel(config.channel_size);
    let metrics = NetworkEventChannelMetrics::new(registry);

    let sender = NetworkEventSender {
        tx,
        config,
        metrics: metrics.clone(),
    };

    let receiver = NetworkEventReceiver {
        rx,
        metrics,
        last_stats_log: Instant::now(),
        config,
    };

    (sender, receiver)
}

/// Create channel without metrics registration (for testing).
#[cfg(test)]
pub fn channel_unregistered(
    config: NetworkEventChannelConfig,
) -> (NetworkEventSender, NetworkEventReceiver) {
    let (tx, rx) = mpsc::channel(config.channel_size);
    let metrics = NetworkEventChannelMetrics::new_unregistered();

    let sender = NetworkEventSender {
        tx,
        config,
        metrics: metrics.clone(),
    };

    let receiver = NetworkEventReceiver {
        rx,
        metrics,
        last_stats_log: Instant::now(),
        config,
    };

    (sender, receiver)
}

/// Get a string name for an event type (for metrics/logging).
fn event_type_name(event: &NetworkEvent) -> &'static str {
    match event {
        NetworkEvent::ListeningOn { .. } => "listening_on",
        NetworkEvent::Subscribed { .. } => "subscribed",
        NetworkEvent::Unsubscribed { .. } => "unsubscribed",
        NetworkEvent::Message { .. } => "message",
        NetworkEvent::StreamOpened { .. } => "stream_opened",
        NetworkEvent::BlobRequested { .. } => "blob_requested",
        NetworkEvent::BlobProvidersFound { .. } => "blob_providers_found",
        NetworkEvent::BlobDownloaded { .. } => "blob_downloaded",
        NetworkEvent::BlobDownloadFailed { .. } => "blob_download_failed",
        NetworkEvent::SpecializedNodeVerificationRequest { .. } => {
            "specialized_node_verification_request"
        }
        NetworkEvent::SpecializedNodeInvitationResponse { .. } => {
            "specialized_node_invitation_response"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::gossipsub::{MessageId, TopicHash};
    use libp2p::PeerId;

    fn create_test_message_event() -> NetworkEvent {
        NetworkEvent::Message {
            id: MessageId::new(b"test"),
            message: libp2p::gossipsub::Message {
                source: Some(PeerId::random()),
                data: vec![0, 1, 2, 3],
                sequence_number: Some(1),
                topic: TopicHash::from_raw("test-topic"),
            },
        }
    }

    #[tokio::test]
    async fn test_basic_send_receive() {
        let config = NetworkEventChannelConfig {
            channel_size: 10,
            ..Default::default()
        };
        let (sender, mut receiver) = channel_unregistered(config);

        let event = create_test_message_event();
        assert!(sender.send(event));

        let received = receiver.recv().await;
        assert!(received.is_some());
    }

    #[tokio::test]
    async fn test_channel_full_drops_events() {
        let config = NetworkEventChannelConfig {
            channel_size: 2,
            warning_threshold: 0.5,
            ..Default::default()
        };
        let (sender, mut receiver) = channel_unregistered(config);

        // Fill the channel
        assert!(sender.send(create_test_message_event()));
        assert!(sender.send(create_test_message_event()));

        // Third should be dropped
        assert!(!sender.send(create_test_message_event()));

        // Verify metrics
        assert_eq!(sender.metrics.events_received.get(), 2);
        assert_eq!(sender.metrics.events_dropped.get(), 1);

        // Drain and verify
        let events = receiver.drain();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn test_graceful_shutdown_drain() {
        let config = NetworkEventChannelConfig {
            channel_size: 100,
            ..Default::default()
        };
        let (sender, mut receiver) = channel_unregistered(config);

        // Send several events
        for _ in 0..10 {
            sender.send(create_test_message_event());
        }

        // Close and drain
        receiver.close();
        let drained = receiver.drain();
        assert_eq!(drained.len(), 10);
    }

    #[tokio::test]
    async fn test_latency_tracking() {
        let config = NetworkEventChannelConfig {
            channel_size: 10,
            ..Default::default()
        };
        let (sender, mut receiver) = channel_unregistered(config);

        sender.send(create_test_message_event());

        // Small delay to ensure measurable latency
        tokio::time::sleep(Duration::from_millis(1)).await;

        let _ = receiver.recv().await;

        // Latency should be recorded (we can't easily check histogram values in tests)
        assert_eq!(receiver.metrics.events_processed.get(), 1);
    }
}
