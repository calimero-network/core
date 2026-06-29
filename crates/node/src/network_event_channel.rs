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
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

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

    /// When the channel is full, how long an overflow event may wait for
    /// capacity before it is given up on and counted as a true drop.
    ///
    /// Instead of dropping immediately on a full channel, the sender hands
    /// the event to a bounded async retry that applies backpressure for up
    /// to this duration. Default: 5 seconds.
    pub send_timeout: Duration,

    /// Maximum number of overflow events that may be waiting for capacity
    /// concurrently. Bounds the extra memory the retry path can buffer on
    /// top of `channel_size`; past this cap, overflow events are dropped
    /// (with escalation) rather than queued. Default: equal to `channel_size`.
    pub max_pending_retries: usize,
}

impl Default for NetworkEventChannelConfig {
    fn default() -> Self {
        Self {
            channel_size: 1000,
            warning_threshold: 0.8,
            stats_log_interval: Duration::from_secs(30),
            send_timeout: Duration::from_secs(5),
            max_pending_retries: 1000,
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

    /// Events dropped after the bounded retry failed (timeout / closed /
    /// retry queue saturated). Unlike before, a full channel no longer
    /// implies a drop — only a sustained-overload give-up does.
    pub events_dropped: Counter,

    /// Overflow events that hit a full channel and were handed to the
    /// bounded async retry instead of being dropped outright.
    pub events_retried: Counter,

    /// Overflow events that the retry path successfully delivered once
    /// capacity freed up (i.e. drops that backpressure prevented).
    pub events_recovered: Counter,

    /// 1 while at least one overflow event is waiting for capacity, 0
    /// otherwise. This is the escalated backpressure signal: a sustained
    /// `1` means the processor cannot keep up with the inbound feed.
    pub backpressure_active: Gauge,

    /// Processing latency histogram (time from send to receive).
    pub processing_latency: Histogram,

    /// High watermark (maximum channel depth seen).
    pub high_watermark: Arc<AtomicU64>,

    /// Number of overflow events currently waiting for capacity in the
    /// retry path. Backs `max_pending_retries` and drives
    /// `backpressure_active`. Not a registered metric (operational state).
    pub pending_retries: Arc<AtomicU64>,
}

impl NetworkEventChannelMetrics {
    /// Create new metrics and register with the provided registry.
    pub fn new(registry: &mut Registry) -> Self {
        let channel_depth = Gauge::default();
        let events_received = Counter::default();
        let events_processed = Counter::default();
        let events_dropped = Counter::default();
        let events_retried = Counter::default();
        let events_recovered = Counter::default();
        let backpressure_active = Gauge::default();

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
            "Number of events dropped after the bounded retry gave up",
            events_dropped.clone(),
        );
        sub_registry.register(
            "retried_total",
            "Number of overflow events handed to the bounded retry path",
            events_retried.clone(),
        );
        sub_registry.register(
            "recovered_total",
            "Number of overflow events the retry path eventually delivered",
            events_recovered.clone(),
        );
        sub_registry.register(
            "backpressure_active",
            "1 while overflow events are waiting for channel capacity",
            backpressure_active.clone(),
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
            events_retried,
            events_recovered,
            backpressure_active,
            processing_latency,
            high_watermark: Arc::new(AtomicU64::new(0)),
            pending_retries: Arc::new(AtomicU64::new(0)),
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
            events_retried: Counter::default(),
            events_recovered: Counter::default(),
            backpressure_active: Gauge::default(),
            processing_latency: Histogram::new(exponential_buckets(0.0001, 2.0, 18)),
            high_watermark: Arc::new(AtomicU64::new(0)),
            pending_retries: Arc::new(AtomicU64::new(0)),
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
    /// The fast path uses `try_send` so a non-full channel never blocks the
    /// network thread. When the channel is *full*, the event is not dropped
    /// silently: it is handed to a bounded async retry (see
    /// [`NetworkEventChannelConfig::send_timeout`] /
    /// [`max_pending_retries`](NetworkEventChannelConfig::max_pending_retries))
    /// that waits for capacity, turning a transient burst into backpressure
    /// rather than lost control events. An event is only truly dropped when
    /// the retry times out, the retry queue is saturated, or the channel is
    /// closed.
    ///
    /// Returns `true` if the event was enqueued or accepted for retry,
    /// `false` if it was dropped or the channel is closed.
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
            Err(mpsc::error::TrySendError::Full(timestamped)) => {
                // Channel full: don't drop. Apply backpressure via a bounded
                // async retry that waits for capacity.
                self.spawn_retry(event_type, timestamped)
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

    /// Hand a full-channel overflow event to a bounded async retry instead
    /// of dropping it.
    ///
    /// The network thread can't `await`, so the wait happens on a detached
    /// task that holds a clone of the sender and `send().await`s for up to
    /// `send_timeout`. The number of concurrent waiters is capped by
    /// `max_pending_retries`; past the cap (or with no async runtime to
    /// spawn onto) the event is dropped with escalation. Returns `true` when
    /// the event was accepted for retry, `false` when it was dropped.
    fn spawn_retry(&self, event_type: &'static str, event: TimestampedEvent) -> bool {
        let Ok(handle) = Handle::try_current() else {
            // No async runtime to wait on (e.g. a synchronous caller) —
            // fall back to a true drop rather than blocking the caller.
            self.record_drop(event_type, event, "no async runtime for retry");
            return false;
        };

        // Soft cap on concurrent waiters bounds extra buffering on top of
        // the channel itself. Reserve a slot first; release it if we're over.
        let pending = self.metrics.pending_retries.fetch_add(1, Ordering::AcqRel) + 1;
        if pending > self.config.max_pending_retries as u64 {
            let _ = self.metrics.pending_retries.fetch_sub(1, Ordering::AcqRel);
            self.record_drop(event_type, event, "retry queue saturated");
            return false;
        }

        self.metrics.events_retried.inc();
        self.metrics.backpressure_active.set(1);
        warn!(
            event_type,
            pending,
            channel_size = self.config.channel_size,
            "Network event channel full - applying backpressure (retrying event)"
        );

        let tx = self.tx.clone();
        let metrics = self.metrics.clone();
        let send_timeout = self.config.send_timeout;

        let _detached = handle.spawn(async move {
            match timeout(send_timeout, tx.send(event)).await {
                Ok(Ok(())) => {
                    metrics.events_received.inc();
                    metrics.events_recovered.inc();
                }
                Ok(Err(_closed)) => {
                    metrics.events_dropped.inc();
                    warn!(
                        event_type,
                        "Network event channel closed while retrying - dropping event"
                    );
                }
                Err(_elapsed) => {
                    metrics.events_dropped.inc();
                    error!(
                        event_type,
                        timeout_secs = send_timeout.as_secs_f64(),
                        "Network event channel saturated - dropping event after \
                         backpressure timeout. The processor cannot keep up with \
                         the inbound feed."
                    );
                }
            }

            // Release the waiter slot and clear the signal once the last
            // waiter drains.
            let remaining = metrics.pending_retries.fetch_sub(1, Ordering::AcqRel) - 1;
            if remaining == 0 {
                metrics.backpressure_active.set(0);
            }
        });

        true
    }

    /// Record a true drop (retry could not be attempted or was refused).
    fn record_drop(&self, event_type: &'static str, dropped: TimestampedEvent, reason: &str) {
        self.metrics.events_dropped.inc();
        error!(
            event_type,
            reason,
            channel_size = self.config.channel_size,
            "Network event channel dropping event - processor cannot keep up"
        );
        debug!(?dropped.event, "Dropped event details");
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
    async fn test_full_channel_applies_backpressure_and_recovers() {
        let config = NetworkEventChannelConfig {
            channel_size: 2,
            warning_threshold: 0.5,
            send_timeout: Duration::from_secs(5),
            ..Default::default()
        };
        let (sender, mut receiver) = channel_unregistered(config);

        // Fill the channel.
        assert!(sender.send(create_test_message_event()));
        assert!(sender.send(create_test_message_event()));

        // The overflow event is accepted for retry, not dropped silently.
        assert!(sender.send(create_test_message_event()));
        assert_eq!(sender.metrics.events_retried.get(), 1);
        assert_eq!(sender.metrics.events_dropped.get(), 0);

        // Draining frees capacity, so the retried event eventually lands —
        // all three events are delivered, none lost.
        let mut received = 0;
        while received < 3 {
            match receiver.recv().await {
                Some(_) => received += 1,
                None => break,
            }
        }
        assert_eq!(received, 3);

        // The retry path reports the recovery and clears backpressure.
        for _ in 0..100 {
            if sender.metrics.events_recovered.get() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(sender.metrics.events_recovered.get(), 1);
        assert_eq!(sender.metrics.events_dropped.get(), 0);
        assert_eq!(sender.metrics.backpressure_active.get(), 0);
    }

    #[tokio::test]
    async fn test_retry_timeout_drops_event() {
        let config = NetworkEventChannelConfig {
            channel_size: 1,
            send_timeout: Duration::from_millis(50),
            ..Default::default()
        };
        // Keep the receiver alive (so the channel stays open) but never drain
        // it, so the retry can only end by timing out.
        let (sender, _receiver) = channel_unregistered(config);

        assert!(sender.send(create_test_message_event()));

        // Overflow is accepted for retry first...
        assert!(sender.send(create_test_message_event()));
        assert_eq!(sender.metrics.events_retried.get(), 1);

        // ...then dropped once the backpressure timeout elapses with no room.
        for _ in 0..100 {
            if sender.metrics.events_dropped.get() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(sender.metrics.events_dropped.get(), 1);
        assert_eq!(sender.metrics.events_recovered.get(), 0);
        assert_eq!(sender.metrics.backpressure_active.get(), 0);
    }

    #[tokio::test]
    async fn test_retry_queue_saturation_drops() {
        let config = NetworkEventChannelConfig {
            channel_size: 1,
            send_timeout: Duration::from_secs(5),
            max_pending_retries: 1,
            ..Default::default()
        };
        let (sender, _receiver) = channel_unregistered(config);

        // Fill the channel; nothing drains it.
        assert!(sender.send(create_test_message_event()));

        // First overflow takes the single retry slot.
        assert!(sender.send(create_test_message_event()));
        assert_eq!(sender.metrics.events_retried.get(), 1);

        // Second overflow exceeds the cap and is dropped immediately rather
        // than buffered without bound.
        assert!(!sender.send(create_test_message_event()));
        assert_eq!(sender.metrics.events_dropped.get(), 1);
        assert_eq!(sender.metrics.events_retried.get(), 1);
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
