//! Dial phase instrumentation for sync connection optimization
//!
//! This module tracks the dial/connection establishment phase separately from
//! peer finding. The key insight is:
//!
//! - Peer finding: <1ms (fast, already optimized)
//! - Peer dialing: ~170ms P50 (this is the bottleneck)
//!
//! ## Log Markers
//!
//! - `PEER_DIAL_BREAKDOWN`: Per-dial attempt timing and result
//! - `DIAL_POOL_STATS`: Connection pool statistics
//!
//! ## Key Metrics
//!
//! - `was_connected_initially`: Did we have an existing connection?
//! - `total_dial_ms`: Time for libp2p open_stream
//! - `reuse_connection`: Did we reuse an existing connection?
//! - `attempt_index`: Which attempt succeeded (1 = first try)

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use libp2p::PeerId;
use tracing::info;

/// Result of a dial attempt
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialResult {
    /// Successfully connected
    Success,
    /// Connection timed out
    Timeout,
    /// Connection refused
    Refused,
    /// No route to peer
    NoRoute,
    /// Other error
    Error,
}

impl std::fmt::Display for DialResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Timeout => write!(f, "timeout"),
            Self::Refused => write!(f, "refused"),
            Self::NoRoute => write!(f, "no_route"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// Breakdown of a dial attempt
#[derive(Debug, Clone, Default)]
pub struct DialBreakdown {
    /// Peer we're dialing
    pub peer_id: Option<PeerId>,

    /// Was peer already connected when we started?
    pub was_connected_initially: bool,

    /// Total time for libp2p open_stream
    pub total_dial_ms: f64,

    /// Did we reuse an existing connection?
    pub reuse_connection: bool,

    /// Which attempt is this (1 = first)
    pub attempt_index: u32,

    /// Result of the dial
    pub result: Option<DialResult>,

    /// Time to first response after stream opened (if tracked)
    pub first_response_ms: Option<f64>,
}

impl DialBreakdown {
    /// Log this breakdown using PEER_DIAL_BREAKDOWN marker
    pub fn log(&self, context_id: &str) {
        let peer = self
            .peer_id
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let result = self
            .result
            .map(|r| r.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let first_resp = self
            .first_response_ms
            .map(|ms| format!("{:.2}", ms))
            .unwrap_or_else(|| "null".to_string());

        info!(
            context_id = %context_id,
            peer_id = %peer,
            was_connected_initially = %self.was_connected_initially,
            total_dial_ms = %format!("{:.2}", self.total_dial_ms),
            reuse_connection = %self.reuse_connection,
            attempt_index = %self.attempt_index,
            first_response_ms = %first_resp,
            result = %result,
            "PEER_DIAL_BREAKDOWN"
        );
    }
}

/// Tracks dial attempts with timing
pub struct DialTracker {
    breakdown: DialBreakdown,
    dial_start: Option<Instant>,
}

impl DialTracker {
    /// Start tracking a dial attempt
    pub fn new(peer_id: PeerId, was_connected: bool, attempt_index: u32) -> Self {
        Self {
            breakdown: DialBreakdown {
                peer_id: Some(peer_id),
                was_connected_initially: was_connected,
                attempt_index,
                ..Default::default()
            },
            dial_start: None,
        }
    }

    /// Start timing the dial
    pub fn start_dial(&mut self) {
        self.dial_start = Some(Instant::now());
    }

    /// End dial with result
    pub fn end_dial(&mut self, result: DialResult, reused: bool) {
        if let Some(start) = self.dial_start.take() {
            self.breakdown.total_dial_ms = start.elapsed().as_secs_f64() * 1000.0;
        }
        self.breakdown.result = Some(result);
        self.breakdown.reuse_connection = reused;
    }

    /// Record time to first response
    pub fn record_first_response(&mut self, ms: f64) {
        self.breakdown.first_response_ms = Some(ms);
    }

    /// Finish and log the breakdown
    pub fn finish(self, context_id: &str) -> DialBreakdown {
        self.breakdown.log(context_id);
        self.breakdown
    }

    /// Get breakdown without logging
    pub fn into_breakdown(self) -> DialBreakdown {
        self.breakdown
    }
}

// ============================================================================
// Connection Pool Tracking
// ============================================================================

/// Statistics for connection pooling
#[derive(Debug, Clone, Default)]
pub struct ConnectionPoolStats {
    /// Total dial attempts
    pub total_dials: u64,

    /// Dials that reused existing connection
    pub reused_connections: u64,

    /// Dials that established new connection
    pub new_connections: u64,

    /// Successful dials
    pub successes: u64,

    /// Failed dials
    pub failures: u64,

    /// Average dial time for reused connections (ms)
    pub avg_reuse_dial_ms: f64,

    /// Average dial time for new connections (ms)
    pub avg_new_dial_ms: f64,

    /// Sum of reuse dial times (for calculating avg)
    sum_reuse_dial_ms: f64,

    /// Sum of new dial times (for calculating avg)
    sum_new_dial_ms: f64,
}

impl ConnectionPoolStats {
    /// Record a dial attempt
    pub fn record(&mut self, breakdown: &DialBreakdown) {
        self.total_dials += 1;

        if breakdown.result == Some(DialResult::Success) {
            self.successes += 1;
        } else {
            self.failures += 1;
        }

        if breakdown.reuse_connection {
            self.reused_connections += 1;
            self.sum_reuse_dial_ms += breakdown.total_dial_ms;
            if self.reused_connections > 0 {
                self.avg_reuse_dial_ms = self.sum_reuse_dial_ms / self.reused_connections as f64;
            }
        } else {
            self.new_connections += 1;
            self.sum_new_dial_ms += breakdown.total_dial_ms;
            if self.new_connections > 0 {
                self.avg_new_dial_ms = self.sum_new_dial_ms / self.new_connections as f64;
            }
        }
    }

    /// Connection reuse rate (0.0 - 1.0)
    pub fn reuse_rate(&self) -> f64 {
        if self.total_dials == 0 {
            0.0
        } else {
            self.reused_connections as f64 / self.total_dials as f64
        }
    }

    /// Log pool statistics
    pub fn log(&self) {
        info!(
            total_dials = %self.total_dials,
            reused_connections = %self.reused_connections,
            new_connections = %self.new_connections,
            reuse_rate = %format!("{:.2}%", self.reuse_rate() * 100.0),
            successes = %self.successes,
            failures = %self.failures,
            avg_reuse_dial_ms = %format!("{:.2}", self.avg_reuse_dial_ms),
            avg_new_dial_ms = %format!("{:.2}", self.avg_new_dial_ms),
            "DIAL_POOL_STATS"
        );
    }
}

/// Thread-safe connection pool stats tracker
pub type SharedPoolStats = Arc<RwLock<ConnectionPoolStats>>;

/// Create a new shared pool stats tracker
pub fn new_pool_stats() -> SharedPoolStats {
    Arc::new(RwLock::new(ConnectionPoolStats::default()))
}

// ============================================================================
// Peer Connection State
// ============================================================================

/// Tracks known connection state for peers
#[derive(Debug, Clone, Default)]
pub struct PeerConnectionState {
    /// When connection was established
    pub connected_since: Option<Instant>,

    /// Last successful dial time
    pub last_dial_ms: Option<f64>,

    /// RTT estimate (ms)
    pub rtt_estimate_ms: Option<f64>,

    /// Consecutive failures
    pub consecutive_failures: u32,
}

impl PeerConnectionState {
    /// Update with successful dial
    pub fn on_success(&mut self, dial_ms: f64) {
        self.connected_since = Some(Instant::now());
        self.last_dial_ms = Some(dial_ms);
        self.consecutive_failures = 0;

        // Update RTT estimate (exponential moving average)
        self.rtt_estimate_ms = Some(match self.rtt_estimate_ms {
            Some(prev) => prev * 0.8 + dial_ms * 0.2,
            None => dial_ms,
        });
    }

    /// Update with failure
    pub fn on_failure(&mut self) {
        self.consecutive_failures += 1;
    }

    /// Check if we believe peer is connected
    pub fn is_likely_connected(&self) -> bool {
        self.connected_since.is_some() && self.consecutive_failures == 0
    }
}

/// Tracks connection state for multiple peers
#[derive(Debug)]
pub struct ConnectionStateTracker {
    peers: HashMap<PeerId, PeerConnectionState>,
}

impl ConnectionStateTracker {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    /// Get or create state for a peer
    pub fn get_mut(&mut self, peer_id: PeerId) -> &mut PeerConnectionState {
        self.peers.entry(peer_id).or_default()
    }

    /// Get state for a peer
    pub fn get(&self, peer_id: &PeerId) -> Option<&PeerConnectionState> {
        self.peers.get(peer_id)
    }

    /// Check if peer is likely connected
    pub fn is_likely_connected(&self, peer_id: &PeerId) -> bool {
        self.peers
            .get(peer_id)
            .map(|s| s.is_likely_connected())
            .unwrap_or(false)
    }

    /// Get peers sorted by RTT (fastest first)
    pub fn peers_by_rtt(&self) -> Vec<(PeerId, f64)> {
        let mut peers: Vec<_> = self
            .peers
            .iter()
            .filter_map(|(id, state)| state.rtt_estimate_ms.map(|rtt| (*id, rtt)))
            .collect();
        peers.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        peers
    }
}

impl Default for ConnectionStateTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe connection state tracker
pub type SharedConnectionState = Arc<RwLock<ConnectionStateTracker>>;

/// Create a new shared connection state tracker
pub fn new_connection_state() -> SharedConnectionState {
    Arc::new(RwLock::new(ConnectionStateTracker::new()))
}

// ============================================================================
// Parallel Dialing Support
// ============================================================================

/// Configuration for parallel dialing
#[derive(Debug, Clone, Copy)]
pub struct ParallelDialConfig {
    /// Maximum concurrent dial attempts
    pub max_concurrent: usize,

    /// Timeout for individual dial attempt
    pub dial_timeout_ms: u64,

    /// Whether to cancel remaining dials on first success
    pub cancel_on_success: bool,
}

impl Default for ParallelDialConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 3,
            dial_timeout_ms: 5000,
            cancel_on_success: true,
        }
    }
}

/// Result of a parallel dial operation
#[derive(Debug)]
pub struct ParallelDialResult {
    /// The peer that succeeded (if any)
    pub success_peer: Option<PeerId>,

    /// Time to first success (ms)
    pub time_to_success_ms: Option<f64>,

    /// Total attempts made
    pub attempts: usize,

    /// Per-peer results
    pub peer_results: Vec<(PeerId, DialResult, f64)>,
}

impl ParallelDialResult {
    /// Check if any dial succeeded
    pub fn succeeded(&self) -> bool {
        self.success_peer.is_some()
    }

    /// Get the winning peer's dial time
    pub fn winning_dial_ms(&self) -> Option<f64> {
        self.time_to_success_ms
    }
}

/// Tracks parallel dial attempts
pub struct ParallelDialTracker {
    config: ParallelDialConfig,
    start: Instant,
    results: Vec<(PeerId, DialResult, f64)>,
    first_success: Option<(PeerId, f64)>,
}

impl ParallelDialTracker {
    /// Create a new parallel dial tracker
    pub fn new(config: ParallelDialConfig) -> Self {
        Self {
            config,
            start: Instant::now(),
            results: Vec::new(),
            first_success: None,
        }
    }

    /// Record a dial result
    pub fn record(&mut self, peer_id: PeerId, result: DialResult, dial_ms: f64) {
        self.results.push((peer_id, result, dial_ms));

        if result == DialResult::Success && self.first_success.is_none() {
            let elapsed = self.start.elapsed().as_secs_f64() * 1000.0;
            self.first_success = Some((peer_id, elapsed));
        }
    }

    /// Finish and get results
    pub fn finish(self, context_id: &str) -> ParallelDialResult {
        let result = ParallelDialResult {
            success_peer: self.first_success.map(|(p, _)| p),
            time_to_success_ms: self.first_success.map(|(_, t)| t),
            attempts: self.results.len(),
            peer_results: self.results,
        };

        // Log parallel dial summary
        info!(
            context_id = %context_id,
            success = %result.succeeded(),
            attempts = %result.attempts,
            time_to_success_ms = %result.time_to_success_ms.map(|t| format!("{:.2}", t)).unwrap_or_else(|| "N/A".to_string()),
            "PARALLEL_DIAL_RESULT"
        );

        result
    }

    /// Get config
    pub fn config(&self) -> &ParallelDialConfig {
        &self.config
    }

    /// Check if we should cancel remaining dials
    pub fn should_cancel(&self) -> bool {
        self.config.cancel_on_success && self.first_success.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_dial_breakdown() {
        let peer = PeerId::random();
        let mut tracker = DialTracker::new(peer, false, 1);

        tracker.start_dial();
        std::thread::sleep(Duration::from_millis(10));
        tracker.end_dial(DialResult::Success, false);

        let breakdown = tracker.into_breakdown();
        assert!(breakdown.total_dial_ms >= 10.0);
        assert_eq!(breakdown.result, Some(DialResult::Success));
        assert!(!breakdown.reuse_connection);
    }

    #[test]
    fn test_pool_stats() {
        let mut stats = ConnectionPoolStats::default();

        // Record a reused connection
        stats.record(&DialBreakdown {
            reuse_connection: true,
            total_dial_ms: 10.0,
            result: Some(DialResult::Success),
            ..Default::default()
        });

        // Record a new connection
        stats.record(&DialBreakdown {
            reuse_connection: false,
            total_dial_ms: 150.0,
            result: Some(DialResult::Success),
            ..Default::default()
        });

        assert_eq!(stats.total_dials, 2);
        assert_eq!(stats.reused_connections, 1);
        assert_eq!(stats.new_connections, 1);
        assert!((stats.reuse_rate() - 0.5).abs() < 0.01);
        assert!((stats.avg_reuse_dial_ms - 10.0).abs() < 0.01);
        assert!((stats.avg_new_dial_ms - 150.0).abs() < 0.01);
    }

    #[test]
    fn test_connection_state_rtt() {
        let mut tracker = ConnectionStateTracker::new();
        let peer = PeerId::random();

        tracker.get_mut(peer).on_success(100.0);
        tracker.get_mut(peer).on_success(150.0);

        // Should be exponential moving average
        let rtt = tracker.get(&peer).unwrap().rtt_estimate_ms.unwrap();
        // 100 * 0.8 + 150 * 0.2 = 80 + 30 = 110
        assert!((rtt - 110.0).abs() < 0.01);
    }

    #[test]
    fn test_connection_state_failure_tracking() {
        let mut tracker = ConnectionStateTracker::new();
        let peer = PeerId::random();

        // Initially no state
        assert!(!tracker.is_likely_connected(&peer));

        // After success, should be connected
        tracker.get_mut(peer).on_success(100.0);
        assert!(tracker.is_likely_connected(&peer));

        // After failure, should not be connected
        tracker.get_mut(peer).on_failure();
        assert!(!tracker.is_likely_connected(&peer));
    }

    #[test]
    fn test_peers_by_rtt() {
        let mut tracker = ConnectionStateTracker::new();
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();
        let peer3 = PeerId::random();

        tracker.get_mut(peer1).on_success(200.0);
        tracker.get_mut(peer2).on_success(50.0);
        tracker.get_mut(peer3).on_success(100.0);

        let sorted = tracker.peers_by_rtt();
        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0].0, peer2); // Fastest
        assert_eq!(sorted[2].0, peer1); // Slowest
    }

    #[test]
    fn test_parallel_dial_tracker() {
        let config = ParallelDialConfig::default();
        let mut tracker = ParallelDialTracker::new(config);

        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        // First dial fails
        tracker.record(peer1, DialResult::Timeout, 5000.0);
        assert!(!tracker.should_cancel());

        // Second dial succeeds
        tracker.record(peer2, DialResult::Success, 100.0);
        assert!(tracker.should_cancel());

        let result = tracker.finish("test-context");
        assert!(result.succeeded());
        assert_eq!(result.success_peer, Some(peer2));
        assert_eq!(result.attempts, 2);
    }

    #[test]
    fn test_parallel_dial_no_success() {
        let config = ParallelDialConfig::default();
        let mut tracker = ParallelDialTracker::new(config);

        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        tracker.record(peer1, DialResult::Timeout, 5000.0);
        tracker.record(peer2, DialResult::Refused, 100.0);

        let result = tracker.finish("test-context");
        assert!(!result.succeeded());
        assert_eq!(result.success_peer, None);
        assert_eq!(result.attempts, 2);
    }
}
