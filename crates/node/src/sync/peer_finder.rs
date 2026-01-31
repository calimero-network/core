//! Peer finding instrumentation and optimization
//!
//! This module provides detailed instrumentation for peer discovery
//! to identify bottlenecks in the sync peer selection process.
//!
//! ## Log Markers
//!
//! - `PEER_FIND_BREAKDOWN`: Detailed timing for each peer finding attempt

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use libp2p::PeerId;
use tracing::{debug, info};

/// Maximum number of recent peers to cache per context
const RECENT_PEER_CACHE_SIZE: usize = 10;

/// Default recent success threshold (5 minutes)
const RECENT_SUCCESS_THRESHOLD_SECS: u64 = 300;

/// Source from which a peer was found
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSource {
    /// From gossipsub mesh
    Mesh,
    /// From routing table / Kademlia
    RoutingTable,
    /// From address book (persisted)
    AddressBook,
    /// From recent successful peers cache
    RecentCache,
    /// Unknown / not tracked
    Unknown,
}

impl std::fmt::Display for PeerSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mesh => write!(f, "mesh"),
            Self::RoutingTable => write!(f, "routing"),
            Self::AddressBook => write!(f, "book"),
            Self::RecentCache => write!(f, "recent"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Detailed breakdown of peer finding timing
#[derive(Debug, Default, Clone)]
pub struct PeerFindBreakdown {
    /// Total time spent finding peers
    pub total_ms: f64,

    /// Time spent querying gossipsub mesh
    pub from_mesh_ms: f64,

    /// Time spent querying routing table
    pub from_routing_table_ms: f64,

    /// Time spent querying address book
    pub from_address_book_ms: f64,

    /// Time spent querying recent peers cache
    pub from_recent_peers_ms: f64,

    /// Total candidates found
    pub candidates_total: usize,

    /// Candidates from mesh
    pub candidates_from_mesh: usize,

    /// Candidates from routing
    pub candidates_from_routing: usize,

    /// Candidates from address book
    pub candidates_from_book: usize,

    /// Candidates from recent cache
    pub candidates_from_recent: usize,

    /// Candidates after filtering (backoff, failure filters)
    pub candidates_after_filters: usize,

    /// Source of selected peer
    pub selected_peer_source: Option<PeerSource>,

    /// Whether selected peer was recently successful
    pub was_recently_successful: bool,

    /// Number of recent failures for selected peer
    pub recent_failure_count: u32,

    /// Milliseconds since last success (if known)
    pub last_success_ms_ago: Option<u64>,
}

impl PeerFindBreakdown {
    /// Log this breakdown using the PEER_FIND_BREAKDOWN marker
    pub fn log(&self, context_id: &str) {
        let selected_source = self
            .selected_peer_source
            .map(|s| s.to_string())
            .unwrap_or_else(|| "none".to_string());
        let last_success = self
            .last_success_ms_ago
            .map(|ms| ms.to_string())
            .unwrap_or_else(|| "null".to_string());

        info!(
            context_id = %context_id,
            peer_find_total_ms = %format!("{:.2}", self.total_ms),
            from_mesh_ms = %format!("{:.2}", self.from_mesh_ms),
            from_routing_table_ms = %format!("{:.2}", self.from_routing_table_ms),
            from_address_book_ms = %format!("{:.2}", self.from_address_book_ms),
            from_recent_peers_ms = %format!("{:.2}", self.from_recent_peers_ms),
            candidates_total = %self.candidates_total,
            candidates_from_mesh = %self.candidates_from_mesh,
            candidates_from_routing = %self.candidates_from_routing,
            candidates_from_book = %self.candidates_from_book,
            candidates_from_recent = %self.candidates_from_recent,
            candidates_after_filters = %self.candidates_after_filters,
            selected_peer_source = %selected_source,
            was_recently_successful = %self.was_recently_successful,
            recent_failure_count = %self.recent_failure_count,
            last_success_ms_ago = %last_success,
            "PEER_FIND_BREAKDOWN"
        );
    }
}

/// Quality information about a peer
#[derive(Debug, Clone)]
pub struct PeerQuality {
    /// When this peer was last successfully synced with
    pub last_success: Option<Instant>,

    /// Number of consecutive failures
    pub failure_count: u32,

    /// When the last failure occurred
    pub last_failure: Option<Instant>,

    /// Source from which this peer was originally found
    pub source: PeerSource,
}

impl Default for PeerQuality {
    fn default() -> Self {
        Self {
            last_success: None,
            failure_count: 0,
            last_failure: None,
            source: PeerSource::Unknown,
        }
    }
}

impl PeerQuality {
    /// Check if this peer was recently successful (within threshold)
    pub fn was_recently_successful(&self, threshold_secs: u64) -> bool {
        self.last_success
            .map(|t| t.elapsed().as_secs() < threshold_secs)
            .unwrap_or(false)
    }

    /// Get milliseconds since last success
    pub fn last_success_ms_ago(&self) -> Option<u64> {
        self.last_success.map(|t| t.elapsed().as_millis() as u64)
    }

    /// Check if this peer should be in backoff
    pub fn is_in_backoff(&self, backoff_duration: Duration) -> bool {
        if self.failure_count == 0 {
            return false;
        }

        self.last_failure
            .map(|t| t.elapsed() < backoff_duration)
            .unwrap_or(false)
    }
}

/// Cache of recent successful peers per context
#[derive(Debug, Default)]
pub struct RecentPeerCache {
    /// Per-context LRU of recent successful peers
    cache: HashMap<[u8; 32], VecDeque<PeerId>>,

    /// Quality info for each peer
    quality: HashMap<PeerId, PeerQuality>,
}

impl RecentPeerCache {
    /// Create a new recent peer cache
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful sync with a peer
    pub fn record_success(&mut self, context_id: [u8; 32], peer_id: PeerId, source: PeerSource) {
        // Update quality
        let quality = self.quality.entry(peer_id).or_default();
        quality.last_success = Some(Instant::now());
        quality.failure_count = 0;
        quality.source = source;

        // Update LRU cache
        let recent = self.cache.entry(context_id).or_default();

        // Remove if already present (to move to front)
        recent.retain(|p| *p != peer_id);

        // Add to front
        recent.push_front(peer_id);

        // Trim to max size
        while recent.len() > RECENT_PEER_CACHE_SIZE {
            recent.pop_back();
        }

        debug!(
            context_id = hex::encode(context_id),
            %peer_id,
            cache_size = recent.len(),
            "Recorded successful peer sync"
        );
    }

    /// Record a failed sync attempt with a peer
    pub fn record_failure(&mut self, peer_id: PeerId) {
        let quality = self.quality.entry(peer_id).or_default();
        quality.failure_count += 1;
        quality.last_failure = Some(Instant::now());

        debug!(
            %peer_id,
            failure_count = quality.failure_count,
            "Recorded peer sync failure"
        );
    }

    /// Get recent peers for a context (most recent first)
    pub fn get_recent(&self, context_id: [u8; 32]) -> Vec<PeerId> {
        self.cache
            .get(&context_id)
            .map(|q| q.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Get quality info for a peer
    pub fn get_quality(&self, peer_id: &PeerId) -> Option<&PeerQuality> {
        self.quality.get(peer_id)
    }

    /// Filter peers by quality criteria
    pub fn filter_viable(&self, peers: &[PeerId], backoff_duration: Duration) -> Vec<PeerId> {
        peers
            .iter()
            .filter(|p| {
                self.quality
                    .get(p)
                    .map(|q| !q.is_in_backoff(backoff_duration))
                    .unwrap_or(true) // Unknown peers are viable
            })
            .copied()
            .collect()
    }
}

/// Thread-safe wrapper for recent peer cache
pub type SharedRecentPeerCache = Arc<RwLock<RecentPeerCache>>;

/// Create a new shared recent peer cache
pub fn new_recent_peer_cache() -> SharedRecentPeerCache {
    Arc::new(RwLock::new(RecentPeerCache::new()))
}

/// Builder for peer finding with instrumentation
pub struct PeerFinder {
    start: Instant,
    breakdown: PeerFindBreakdown,
}

impl PeerFinder {
    /// Start a new peer finding operation
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
            breakdown: PeerFindBreakdown::default(),
        }
    }

    /// Record mesh query timing and results
    pub fn record_mesh_query(&mut self, duration: Duration, candidates: &[PeerId]) {
        self.breakdown.from_mesh_ms = duration.as_secs_f64() * 1000.0;
        self.breakdown.candidates_from_mesh = candidates.len();
        self.breakdown.candidates_total += candidates.len();
    }

    /// Record routing table query timing and results
    pub fn record_routing_query(&mut self, duration: Duration, candidates: &[PeerId]) {
        self.breakdown.from_routing_table_ms = duration.as_secs_f64() * 1000.0;
        self.breakdown.candidates_from_routing = candidates.len();
        self.breakdown.candidates_total += candidates.len();
    }

    /// Record address book query timing and results
    pub fn record_address_book_query(&mut self, duration: Duration, candidates: &[PeerId]) {
        self.breakdown.from_address_book_ms = duration.as_secs_f64() * 1000.0;
        self.breakdown.candidates_from_book = candidates.len();
        self.breakdown.candidates_total += candidates.len();
    }

    /// Record recent peers cache query timing and results
    pub fn record_recent_query(&mut self, duration: Duration, candidates: &[PeerId]) {
        self.breakdown.from_recent_peers_ms = duration.as_secs_f64() * 1000.0;
        self.breakdown.candidates_from_recent = candidates.len();
        self.breakdown.candidates_total += candidates.len();
    }

    /// Record filtering results
    pub fn record_filtering(&mut self, candidates_after: usize) {
        self.breakdown.candidates_after_filters = candidates_after;
    }

    /// Record selected peer
    pub fn record_selection(&mut self, source: PeerSource, quality: Option<&PeerQuality>) {
        self.breakdown.selected_peer_source = Some(source);

        if let Some(q) = quality {
            self.breakdown.was_recently_successful =
                q.was_recently_successful(RECENT_SUCCESS_THRESHOLD_SECS);
            self.breakdown.recent_failure_count = q.failure_count;
            self.breakdown.last_success_ms_ago = q.last_success_ms_ago();
        }
    }

    /// Finish and return the breakdown
    pub fn finish(mut self) -> PeerFindBreakdown {
        self.breakdown.total_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        self.breakdown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recent_peer_cache() {
        let mut cache = RecentPeerCache::new();
        let context_id = [1u8; 32];
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        // Record successes
        cache.record_success(context_id, peer1, PeerSource::Mesh);
        cache.record_success(context_id, peer2, PeerSource::Mesh);

        // Check order (most recent first)
        let recent = cache.get_recent(context_id);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0], peer2); // peer2 was added last

        // Check quality
        let q1 = cache.get_quality(&peer1).unwrap();
        assert!(q1.was_recently_successful(300));
        assert_eq!(q1.failure_count, 0);
    }

    #[test]
    fn test_peer_backoff() {
        let mut cache = RecentPeerCache::new();
        let peer = PeerId::random();

        // Record failure
        cache.record_failure(peer);

        let q = cache.get_quality(&peer).unwrap();
        assert!(q.is_in_backoff(Duration::from_secs(60)));
        assert!(!q.is_in_backoff(Duration::from_millis(1)));
    }

    #[test]
    fn test_peer_finder_instrumentation() {
        let mut finder = PeerFinder::start();

        // Simulate mesh query
        let mesh_peers = vec![PeerId::random()];
        finder.record_mesh_query(Duration::from_millis(5), &mesh_peers);

        // Simulate filtering
        finder.record_filtering(1);

        // Simulate selection
        finder.record_selection(PeerSource::Mesh, None);

        let breakdown = finder.finish();

        assert!(breakdown.total_ms >= 0.0);
        assert_eq!(breakdown.candidates_from_mesh, 1);
        assert_eq!(breakdown.candidates_after_filters, 1);
        assert_eq!(breakdown.selected_peer_source, Some(PeerSource::Mesh));
    }
}
