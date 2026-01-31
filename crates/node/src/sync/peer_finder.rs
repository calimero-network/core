//! Peer finding instrumentation and optimization
//!
//! This module provides detailed instrumentation for peer discovery
//! to identify bottlenecks in the sync peer selection process.
//!
//! ## Key Distinction: Finding vs Connecting
//!
//! **Peer finding** is the process of identifying viable candidates.
//! **Peer connecting (dialing)** is a separate operation tracked elsewhere.
//!
//! This module ONLY measures finding time, not connection time.
//!
//! ## Log Markers
//!
//! - `PEER_FIND_PHASES`: Per-phase timing (candidate lookup, filtering, selection)
//! - `PEER_FIND_BREAKDOWN`: Legacy detailed breakdown (deprecated)
//!
//! ## Primary KPIs (all exclude dial time)
//!
//! - `time_to_candidate_ms`: Time to produce candidate list (no filtering)
//! - `time_to_viable_peer_ms`: Time to select viable peer (after filtering)
//!
//! ## Peer Finding Strategies
//!
//! - `A0_Baseline`: Current mesh-only approach
//! - `A1_MeshFirst`: Only gossipsub mesh peers, no fallback
//! - `A2_RecentFirst`: LRU cache → mesh → routing
//! - `A3_AddressBookFirst`: Persisted peers → mesh → routing
//! - `A4_ParallelFind`: Query all sources in parallel
//! - `A5_HealthFiltered`: Exclude peers with recent failures

use std::collections::{HashMap, VecDeque};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use libp2p::PeerId;
use tracing::{debug, info};

/// Peer finding strategy for A/B testing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerFindStrategy {
    /// A0: Current baseline - mesh only, wait for formation
    #[default]
    Baseline,
    /// A1: Mesh-first - only mesh peers, fail if empty
    MeshFirst,
    /// A2: Recent-first - try LRU cache of successful peers first
    RecentFirst,
    /// A3: Address-book-first - try persisted known peers first
    AddressBookFirst,
    /// A4: Parallel find - query all sources simultaneously
    ParallelFind,
    /// A5: Health-filtered - exclude peers with recent failures
    HealthFiltered,
}

impl std::fmt::Display for PeerFindStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Baseline => write!(f, "baseline"),
            Self::MeshFirst => write!(f, "mesh-first"),
            Self::RecentFirst => write!(f, "recent-first"),
            Self::AddressBookFirst => write!(f, "address-book-first"),
            Self::ParallelFind => write!(f, "parallel"),
            Self::HealthFiltered => write!(f, "health-filtered"),
        }
    }
}

impl FromStr for PeerFindStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "baseline" | "a0" => Ok(Self::Baseline),
            "mesh-first" | "mesh" | "a1" => Ok(Self::MeshFirst),
            "recent-first" | "recent" | "a2" => Ok(Self::RecentFirst),
            "address-book-first" | "address-book" | "book" | "a3" => Ok(Self::AddressBookFirst),
            "parallel" | "parallel-find" | "a4" => Ok(Self::ParallelFind),
            "health-filtered" | "health" | "a5" => Ok(Self::HealthFiltered),
            _ => Err(format!("Unknown peer find strategy: {}", s)),
        }
    }
}

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

/// Result of a peer finding attempt
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerFindResult {
    /// Successfully found and selected a viable peer
    Success,
    /// Timed out waiting for candidates
    Timeout,
    /// No candidates found from any source
    NoCandidates,
    /// Candidates found but all filtered out
    AllFiltered,
}

impl std::fmt::Display for PeerFindResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Timeout => write!(f, "timeout"),
            Self::NoCandidates => write!(f, "no_candidates"),
            Self::AllFiltered => write!(f, "all_filtered"),
        }
    }
}

/// Per-phase timing for peer finding (separates finding from connecting)
///
/// **CRITICAL**: This struct measures FINDING time only, NOT dial/connection time.
/// Dial time is tracked separately in the SyncManager.
#[derive(Debug, Clone, Default)]
pub struct PeerFindPhases {
    /// Phase 1: Time to get raw candidate list from all sources
    /// (mesh + recent + address_book lookups, NO filtering)
    pub candidate_lookup_ms: f64,

    /// Phase 2: Time to apply filters (backoff, health, etc.)
    pub filtering_ms: f64,

    /// Phase 3: Time to select final peer from filtered list
    pub selection_ms: f64,

    // --- Counts ---
    /// Number of raw candidates before filtering
    pub candidates_raw: usize,

    /// Number of candidates after filtering
    pub candidates_filtered: usize,

    /// Number of attempts before success (0 = first try)
    pub attempt_count: u32,

    // --- Source breakdown ---
    /// Candidates from each source
    pub candidates_from_mesh: usize,
    pub candidates_from_recent: usize,
    pub candidates_from_book: usize,
    pub candidates_from_routing: usize,

    /// Final selected peer source
    pub peer_source: Option<PeerSource>,

    /// Was the selected peer in our recent success cache?
    pub was_recent_success: bool,

    /// Result of the find operation
    pub result: Option<PeerFindResult>,
}

impl PeerFindPhases {
    /// Total time to find a viable peer (excludes dial time)
    pub fn time_to_viable_peer_ms(&self) -> f64 {
        self.candidate_lookup_ms + self.filtering_ms + self.selection_ms
    }

    /// Log this using the PEER_FIND_PHASES marker
    pub fn log(&self, context_id: &str) {
        let result = self
            .result
            .map(|r| r.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let source = self
            .peer_source
            .map(|s| s.to_string())
            .unwrap_or_else(|| "none".to_string());

        info!(
            context_id = %context_id,
            // Primary KPIs (finding time only, NO dial)
            time_to_candidate_ms = %format!("{:.2}", self.candidate_lookup_ms),
            time_to_viable_peer_ms = %format!("{:.2}", self.time_to_viable_peer_ms()),
            // Phase breakdown
            candidate_lookup_ms = %format!("{:.2}", self.candidate_lookup_ms),
            filtering_ms = %format!("{:.2}", self.filtering_ms),
            selection_ms = %format!("{:.2}", self.selection_ms),
            // Counts
            candidates_raw = %self.candidates_raw,
            candidates_filtered = %self.candidates_filtered,
            attempt_count = %self.attempt_count,
            // Source breakdown
            from_mesh = %self.candidates_from_mesh,
            from_recent = %self.candidates_from_recent,
            from_book = %self.candidates_from_book,
            from_routing = %self.candidates_from_routing,
            // Selection info
            peer_source = %source,
            was_recent_success = %self.was_recent_success,
            result = %result,
            "PEER_FIND_PHASES"
        );
    }
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

    /// Select peers using the specified strategy
    ///
    /// Returns (selected_peers, source) where source indicates where peers came from
    pub fn select_by_strategy(
        &self,
        strategy: PeerFindStrategy,
        context_id: [u8; 32],
        mesh_peers: &[PeerId],
        backoff_duration: Duration,
    ) -> (Vec<PeerId>, PeerSource) {
        match strategy {
            PeerFindStrategy::Baseline | PeerFindStrategy::MeshFirst => {
                // A0/A1: Use mesh peers directly
                (mesh_peers.to_vec(), PeerSource::Mesh)
            }
            PeerFindStrategy::RecentFirst => {
                // A2: Try recent successful peers first, then mesh
                let recent = self.get_recent(context_id);
                let viable_recent: Vec<_> = recent
                    .into_iter()
                    .filter(|p| mesh_peers.contains(p)) // Must also be in mesh
                    .filter(|p| {
                        self.quality
                            .get(p)
                            .map(|q| !q.is_in_backoff(backoff_duration))
                            .unwrap_or(true)
                    })
                    .collect();

                if !viable_recent.is_empty() {
                    (viable_recent, PeerSource::RecentCache)
                } else {
                    (mesh_peers.to_vec(), PeerSource::Mesh)
                }
            }
            PeerFindStrategy::AddressBookFirst => {
                // A3: Would use persisted address book - for now, same as baseline
                // TODO: Integrate with libp2p address book
                (mesh_peers.to_vec(), PeerSource::Mesh)
            }
            PeerFindStrategy::ParallelFind => {
                // A4: Combine all sources (recent + mesh), deduplicated
                let recent = self.get_recent(context_id);
                let mut all_peers: Vec<_> = recent;
                for peer in mesh_peers {
                    if !all_peers.contains(peer) {
                        all_peers.push(*peer);
                    }
                }
                let viable = self.filter_viable(&all_peers, backoff_duration);
                if viable
                    .iter()
                    .any(|p| self.get_recent(context_id).contains(p))
                {
                    (viable, PeerSource::RecentCache)
                } else {
                    (viable, PeerSource::Mesh)
                }
            }
            PeerFindStrategy::HealthFiltered => {
                // A5: Filter out peers with recent failures
                let viable = self.filter_viable(mesh_peers, backoff_duration);
                // Sort by quality - peers with recent success first
                let mut sorted: Vec<_> = viable
                    .into_iter()
                    .map(|p| {
                        let score = self
                            .quality
                            .get(&p)
                            .map(|q| {
                                if q.was_recently_successful(300) {
                                    1000 - q.failure_count as i32
                                } else {
                                    -(q.failure_count as i32)
                                }
                            })
                            .unwrap_or(0);
                        (p, score)
                    })
                    .collect();
                sorted.sort_by(|a, b| b.1.cmp(&a.1));
                (
                    sorted.into_iter().map(|(p, _)| p).collect(),
                    PeerSource::Mesh,
                )
            }
        }
    }
}

/// Thread-safe wrapper for recent peer cache
pub type SharedRecentPeerCache = Arc<RwLock<RecentPeerCache>>;

/// Create a new shared recent peer cache
pub fn new_recent_peer_cache() -> SharedRecentPeerCache {
    Arc::new(RwLock::new(RecentPeerCache::new()))
}

// ============================================================================
// NEW: Phase-based tracker (separates finding from connecting)
// ============================================================================

/// Tracks peer finding phases with proper separation from dial time
pub struct PeerFindTracker {
    phases: PeerFindPhases,

    // Phase timers
    candidate_lookup_start: Option<Instant>,
    filtering_start: Option<Instant>,
    selection_start: Option<Instant>,
}

impl PeerFindTracker {
    /// Start a new peer finding operation
    pub fn new() -> Self {
        Self {
            phases: PeerFindPhases::default(),
            candidate_lookup_start: None,
            filtering_start: None,
            selection_start: None,
        }
    }

    /// Start the candidate lookup phase
    pub fn start_candidate_lookup(&mut self) {
        self.candidate_lookup_start = Some(Instant::now());
    }

    /// End candidate lookup, start filtering
    pub fn end_candidate_lookup(
        &mut self,
        candidates: &[PeerId],
        source_breakdown: SourceBreakdown,
    ) {
        if let Some(start) = self.candidate_lookup_start.take() {
            self.phases.candidate_lookup_ms = start.elapsed().as_secs_f64() * 1000.0;
        }
        self.phases.candidates_raw = candidates.len();
        self.phases.candidates_from_mesh = source_breakdown.mesh;
        self.phases.candidates_from_recent = source_breakdown.recent;
        self.phases.candidates_from_book = source_breakdown.book;
        self.phases.candidates_from_routing = source_breakdown.routing;
        self.filtering_start = Some(Instant::now());
    }

    /// End filtering, start selection
    pub fn end_filtering(&mut self, candidates_after: usize) {
        if let Some(start) = self.filtering_start.take() {
            self.phases.filtering_ms = start.elapsed().as_secs_f64() * 1000.0;
        }
        self.phases.candidates_filtered = candidates_after;
        self.selection_start = Some(Instant::now());
    }

    /// End selection with success
    pub fn end_selection(&mut self, source: PeerSource, was_recent: bool) {
        if let Some(start) = self.selection_start.take() {
            self.phases.selection_ms = start.elapsed().as_secs_f64() * 1000.0;
        }
        self.phases.peer_source = Some(source);
        self.phases.was_recent_success = was_recent;
        self.phases.result = Some(PeerFindResult::Success);
    }

    /// Mark as failed with reason
    pub fn mark_failed(&mut self, result: PeerFindResult) {
        // End any open phases
        if let Some(start) = self.candidate_lookup_start.take() {
            self.phases.candidate_lookup_ms = start.elapsed().as_secs_f64() * 1000.0;
        }
        if let Some(start) = self.filtering_start.take() {
            self.phases.filtering_ms = start.elapsed().as_secs_f64() * 1000.0;
        }
        if let Some(start) = self.selection_start.take() {
            self.phases.selection_ms = start.elapsed().as_secs_f64() * 1000.0;
        }
        self.phases.result = Some(result);
    }

    /// Increment attempt count
    pub fn increment_attempt(&mut self) {
        self.phases.attempt_count += 1;
    }

    /// Finish and log the phases
    pub fn finish(self, context_id: &str) -> PeerFindPhases {
        self.phases.log(context_id);
        self.phases
    }

    /// Get the phases without logging
    pub fn into_phases(self) -> PeerFindPhases {
        self.phases
    }
}

impl Default for PeerFindTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Source breakdown for candidate lookup
#[derive(Debug, Clone, Copy, Default)]
pub struct SourceBreakdown {
    pub mesh: usize,
    pub recent: usize,
    pub book: usize,
    pub routing: usize,
}

// ============================================================================
// LEGACY: Old PeerFinder (kept for compatibility)
// ============================================================================

/// Builder for peer finding with instrumentation
#[deprecated(note = "Use PeerFindTracker instead for proper phase separation")]
pub struct PeerFinder {
    start: Instant,
    breakdown: PeerFindBreakdown,
}

#[allow(deprecated)]
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
        // Should be in backoff for a long duration (60s hasn't elapsed)
        assert!(q.is_in_backoff(Duration::from_secs(60)));

        // Wait a tiny bit to ensure we're outside 0ms backoff
        std::thread::sleep(Duration::from_millis(5));

        // Should NOT be in backoff if backoff duration is 0 (already elapsed)
        assert!(!q.is_in_backoff(Duration::ZERO));
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
