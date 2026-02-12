//! Delta buffering for sync scenarios (Invariant I6).
//!
//! When a snapshot sync is in progress, incoming deltas are buffered so they
//! can be replayed after the snapshot completes. This ensures that:
//! 1. Deltas arriving during sync aren't lost (Invariant I6 - Liveness Guarantee)
//! 2. Event handlers can execute for buffered deltas after context is initialized
//!
//! ## Delivery Contract
//!
//! - **Buffer size**: Configurable, default 10,000 deltas per context
//! - **Drop policy**: Oldest-first when buffer full (with metric increment)
//! - **Backpressure**: None (fire-and-forget from network layer)
//! - **Metrics**: `drops` counter MUST be observable
//!
//! ## Minimum Capacity Warning
//!
//! If buffer capacity is set below `MIN_RECOMMENDED_CAPACITY`, a warning should
//! be logged at startup. Zero capacity is valid but will drop ALL deltas.

use std::collections::HashSet;

use calimero_crypto::Nonce;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;

/// Default buffer capacity (10,000 deltas per context).
pub const DEFAULT_BUFFER_CAPACITY: usize = 10_000;

/// Minimum recommended buffer capacity.
///
/// Capacities below this value may cause excessive delta loss under normal load.
/// A warning should be logged if capacity is set below this threshold.
pub const MIN_RECOMMENDED_CAPACITY: usize = 100;

/// Result of pushing a delta to the buffer.
///
/// Provides clear semantics about what happened to both the incoming delta
/// and any evicted delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushResult {
    /// Delta was added to the buffer without eviction.
    Added,
    /// Delta was a duplicate (already in buffer) - no action taken.
    Duplicate,
    /// Delta was added, but oldest delta was evicted due to capacity.
    /// Contains the ID of the evicted delta.
    Evicted([u8; 32]),
    /// Delta was dropped immediately (zero capacity buffer).
    /// Contains the ID of the dropped delta.
    DroppedZeroCapacity([u8; 32]),
}

impl PushResult {
    /// Returns true if the delta was successfully added to the buffer.
    #[must_use]
    pub fn was_added(&self) -> bool {
        matches!(self, Self::Added | Self::Evicted(_))
    }

    /// Returns true if any delta was lost (evicted or dropped).
    #[must_use]
    pub fn had_data_loss(&self) -> bool {
        matches!(self, Self::Evicted(_) | Self::DroppedZeroCapacity(_))
    }

    /// Returns the ID of the lost delta, if any.
    #[must_use]
    pub fn lost_delta_id(&self) -> Option<[u8; 32]> {
        match self {
            Self::Evicted(id) | Self::DroppedZeroCapacity(id) => Some(*id),
            Self::Added | Self::Duplicate => None,
        }
    }
}

/// A single buffered delta.
///
/// Contains ALL fields needed for replay after snapshot sync completes.
/// Previously missing fields (nonce, author_id, root_hash, events) caused
/// data loss because deltas couldn't be decrypted or processed.
///
/// **POC Bug 7**: This struct MUST include all fields for replay - not just
/// `id`, `parents`, `hlc`, `payload`, but also `nonce`, `author_id`, `root_hash`,
/// `events`, and `source_peer`.
#[derive(Debug, Clone)]
pub struct BufferedDelta {
    /// Delta ID.
    pub id: [u8; 32],
    /// Parent IDs.
    pub parents: Vec<[u8; 32]>,
    /// HLC timestamp.
    pub hlc: u64,
    /// Serialized (encrypted) payload.
    pub payload: Vec<u8>,
    /// Nonce for decryption (12 bytes for XChaCha20-Poly1305).
    pub nonce: Nonce,
    /// Author public key (needed to get sender key for decryption).
    pub author_id: PublicKey,
    /// Expected root hash after applying this delta.
    pub root_hash: Hash,
    /// Optional serialized events (for handler execution after replay).
    pub events: Option<Vec<u8>>,
    /// Source peer ID (for requesting sender key if needed).
    pub source_peer: libp2p::PeerId,
}

/// Buffer for storing deltas during snapshot sync.
///
/// Implements Invariant I6: Deltas received during state-based sync MUST be
/// preserved and applied after sync completes.
///
/// When the buffer is full, the oldest delta is evicted (FIFO eviction policy)
/// and the `drops` counter is incremented. Drops MUST be observable via metrics.
///
/// ## Deduplication
///
/// The buffer tracks seen delta IDs to prevent duplicate deltas from being buffered.
/// This protects against replay attacks where an adversary might flood the buffer
/// with duplicate deltas to cause eviction of legitimate deltas.
#[derive(Debug)]
pub struct DeltaBuffer {
    /// Buffered deltas (FIFO queue - oldest at front).
    deltas: std::collections::VecDeque<BufferedDelta>,
    /// Set of delta IDs currently in the buffer (for O(1) deduplication).
    seen_ids: HashSet<[u8; 32]>,
    /// HLC timestamp when buffering started.
    sync_start_hlc: u64,
    /// Maximum buffer size before eviction.
    capacity: usize,
    /// Number of deltas dropped due to buffer overflow (observable metric).
    drops: u64,
}

impl DeltaBuffer {
    /// Create a new delta buffer with specified capacity.
    ///
    /// # Capacity Warning
    ///
    /// If capacity is below `MIN_RECOMMENDED_CAPACITY`, callers should log a
    /// warning at startup. Zero capacity is valid but will drop ALL deltas.
    #[must_use]
    pub fn new(capacity: usize, sync_start_hlc: u64) -> Self {
        Self {
            deltas: std::collections::VecDeque::with_capacity(capacity.min(1000)),
            seen_ids: HashSet::with_capacity(capacity.min(1000)),
            sync_start_hlc,
            capacity,
            drops: 0,
        }
    }

    /// Check if capacity is below recommended minimum.
    ///
    /// Callers should log a warning at session start if this returns true.
    #[must_use]
    pub fn is_capacity_below_recommended(&self) -> bool {
        self.capacity < MIN_RECOMMENDED_CAPACITY
    }

    /// Add a delta to the buffer.
    ///
    /// Returns a `PushResult` indicating what happened:
    /// - `Added`: Delta was added successfully
    /// - `Duplicate`: Delta ID was already in buffer (no action taken)
    /// - `Evicted(id)`: Delta was added but oldest delta was evicted
    /// - `DroppedZeroCapacity(id)`: Delta was dropped (zero capacity buffer)
    ///
    /// # Deduplication
    ///
    /// If a delta with the same ID is already in the buffer, it is not added
    /// again and `PushResult::Duplicate` is returned. This prevents replay attacks.
    ///
    /// # Edge case: zero capacity
    ///
    /// If capacity is 0, the incoming delta is immediately dropped (not added)
    /// and `PushResult::DroppedZeroCapacity` is returned with the dropped delta's ID.
    pub fn push(&mut self, delta: BufferedDelta) -> PushResult {
        let delta_id = delta.id;

        // Handle zero capacity: drop incoming delta immediately
        if self.capacity == 0 {
            self.drops += 1;
            return PushResult::DroppedZeroCapacity(delta_id);
        }

        // Deduplication check (#2: prevents replay attacks)
        if self.seen_ids.contains(&delta_id) {
            return PushResult::Duplicate;
        }

        if self.deltas.len() >= self.capacity {
            // Evict oldest delta (front of queue)
            if let Some(evicted) = self.deltas.pop_front() {
                self.seen_ids.remove(&evicted.id);
                let evicted_id = evicted.id;
                self.drops += 1;
                self.seen_ids.insert(delta_id);
                self.deltas.push_back(delta);
                PushResult::Evicted(evicted_id)
            } else {
                // This shouldn't happen, but handle gracefully
                self.seen_ids.insert(delta_id);
                self.deltas.push_back(delta);
                PushResult::Added
            }
        } else {
            self.seen_ids.insert(delta_id);
            self.deltas.push_back(delta);
            PushResult::Added
        }
    }

    /// Get all buffered deltas for replay, clearing the buffer.
    ///
    /// Returns deltas in FIFO order (oldest first), preserving causality.
    /// Also clears the deduplication set.
    #[must_use]
    pub fn drain(&mut self) -> Vec<BufferedDelta> {
        self.seen_ids.clear();
        self.deltas.drain(..).collect()
    }

    /// Check if a delta ID is already in the buffer.
    ///
    /// This is O(1) due to the internal HashSet tracking.
    #[must_use]
    pub fn contains(&self, id: &[u8; 32]) -> bool {
        self.seen_ids.contains(id)
    }

    /// Number of buffered deltas.
    #[must_use]
    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    /// Check if buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    /// Get the sync start HLC.
    #[must_use]
    pub fn sync_start_hlc(&self) -> u64 {
        self.sync_start_hlc
    }

    /// Get the number of deltas dropped due to buffer overflow.
    ///
    /// This metric MUST be observable per Invariant I6 delivery contract.
    #[must_use]
    pub fn drops(&self) -> u64 {
        self.drops
    }

    /// Get the buffer capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_delta(id: u8) -> BufferedDelta {
        BufferedDelta {
            id: [id; 32],
            parents: vec![[0; 32]],
            hlc: 12345,
            payload: vec![1, 2, 3],
            nonce: [0; 12],
            author_id: PublicKey::from([0; 32]),
            root_hash: Hash::from([0; 32]),
            events: None,
            source_peer: libp2p::PeerId::random(),
        }
    }

    #[test]
    fn test_buffer_basic() {
        let mut buffer = DeltaBuffer::new(100, 12345);
        assert!(buffer.is_empty());
        assert_eq!(buffer.sync_start_hlc(), 12345);
        assert_eq!(buffer.capacity(), 100);
        assert_eq!(buffer.drops(), 0);
        assert!(!buffer.is_capacity_below_recommended());

        let result = buffer.push(make_test_delta(1));
        assert_eq!(result, PushResult::Added, "Should add without eviction");
        assert!(result.was_added());
        assert!(!result.had_data_loss());
        assert_eq!(buffer.len(), 1);

        let drained = buffer.drain();
        assert_eq!(drained.len(), 1);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_buffer_only_during_sync() {
        // Buffer should only accept deltas - caller decides when to buffer
        let mut buffer = DeltaBuffer::new(10, 12345);
        assert!(buffer.is_empty());

        // Push deltas
        assert_eq!(buffer.push(make_test_delta(1)), PushResult::Added);
        assert_eq!(buffer.push(make_test_delta(2)), PushResult::Added);
        assert_eq!(buffer.len(), 2);

        // Drain returns all in FIFO order
        let drained = buffer.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].id[0], 1);
        assert_eq!(drained[1].id[0], 2);
    }

    #[test]
    fn test_buffer_overflow_drops_oldest() {
        let mut buffer = DeltaBuffer::new(2, 0);

        // Fill buffer
        assert_eq!(buffer.push(make_test_delta(1)), PushResult::Added);
        assert_eq!(buffer.push(make_test_delta(2)), PushResult::Added);
        assert_eq!(buffer.drops(), 0);

        // Third delta causes eviction of oldest (delta 1)
        let result = buffer.push(make_test_delta(3));
        assert_eq!(result, PushResult::Evicted([1; 32]), "Should evict delta 1");
        assert!(result.had_data_loss());
        assert_eq!(result.lost_delta_id(), Some([1; 32]));
        assert_eq!(buffer.drops(), 1);
        assert_eq!(buffer.len(), 2);

        // Fourth delta causes another eviction (delta 2)
        let result = buffer.push(make_test_delta(4));
        assert_eq!(result, PushResult::Evicted([2; 32]), "Should evict delta 2");
        assert_eq!(buffer.drops(), 2);
        assert_eq!(buffer.len(), 2);

        // Verify remaining deltas are 3 and 4 (FIFO order)
        let drained = buffer.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].id[0], 3);
        assert_eq!(drained[1].id[0], 4);
    }

    #[test]
    fn test_zero_capacity_drops_immediately() {
        let mut buffer = DeltaBuffer::new(0, 0);
        assert!(buffer.is_empty());
        assert_eq!(buffer.capacity(), 0);
        assert_eq!(buffer.drops(), 0);
        assert!(buffer.is_capacity_below_recommended());

        // First push should drop immediately
        let result = buffer.push(make_test_delta(1));
        assert_eq!(
            result,
            PushResult::DroppedZeroCapacity([1; 32]),
            "Zero capacity should drop incoming delta"
        );
        assert!(result.had_data_loss());
        assert!(!result.was_added());
        assert_eq!(result.lost_delta_id(), Some([1; 32]));
        assert_eq!(buffer.drops(), 1);
        assert!(buffer.is_empty(), "Buffer should remain empty");
        assert_eq!(buffer.len(), 0);

        // Second push should also drop
        let result = buffer.push(make_test_delta(2));
        assert_eq!(result, PushResult::DroppedZeroCapacity([2; 32]));
        assert_eq!(buffer.drops(), 2);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_finish_sync_returns_fifo() {
        let mut buffer = DeltaBuffer::new(100, 0);

        // Add deltas in order
        buffer.push(make_test_delta(1));
        buffer.push(make_test_delta(2));
        buffer.push(make_test_delta(3));

        // Drain should return in FIFO order
        let drained = buffer.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].id[0], 1);
        assert_eq!(drained[1].id[0], 2);
        assert_eq!(drained[2].id[0], 3);
    }

    #[test]
    fn test_cancel_sync_clears_buffer() {
        let mut buffer = DeltaBuffer::new(100, 0);
        buffer.push(make_test_delta(1));
        buffer.push(make_test_delta(2));
        assert_eq!(buffer.len(), 2);

        // Simulate cancel by draining and discarding
        let _ = buffer.drain();
        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_drops_counter_observable() {
        let mut buffer = DeltaBuffer::new(1, 0);
        assert_eq!(buffer.drops(), 0);

        buffer.push(make_test_delta(1));
        assert_eq!(buffer.drops(), 0);

        // Each overflow increments drops
        buffer.push(make_test_delta(2));
        assert_eq!(buffer.drops(), 1);

        buffer.push(make_test_delta(3));
        assert_eq!(buffer.drops(), 2);

        buffer.push(make_test_delta(4));
        assert_eq!(buffer.drops(), 3);
    }

    #[test]
    fn test_deduplication_prevents_double_buffering() {
        let mut buffer = DeltaBuffer::new(10, 0);

        // Add a delta
        assert_eq!(buffer.push(make_test_delta(1)), PushResult::Added);
        assert_eq!(buffer.len(), 1);

        // Try to add same delta again - should be duplicate
        let result = buffer.push(make_test_delta(1));
        assert_eq!(result, PushResult::Duplicate);
        assert!(!result.had_data_loss());
        assert!(!result.was_added()); // Duplicate counts as "not added"
        assert_eq!(buffer.len(), 1); // Still only 1

        // Add a different delta - should work
        assert_eq!(buffer.push(make_test_delta(2)), PushResult::Added);
        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn test_deduplication_cleared_on_drain() {
        let mut buffer = DeltaBuffer::new(10, 0);

        // Add a delta
        assert_eq!(buffer.push(make_test_delta(1)), PushResult::Added);
        assert!(buffer.contains(&[1; 32]));

        // Drain
        let _ = buffer.drain();
        assert!(!buffer.contains(&[1; 32]));

        // Now can add same delta again
        assert_eq!(buffer.push(make_test_delta(1)), PushResult::Added);
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn test_deduplication_cleared_on_eviction() {
        let mut buffer = DeltaBuffer::new(2, 0);

        // Fill buffer
        buffer.push(make_test_delta(1));
        buffer.push(make_test_delta(2));
        assert!(buffer.contains(&[1; 32]));

        // Evict delta 1 by adding delta 3
        buffer.push(make_test_delta(3));
        assert!(!buffer.contains(&[1; 32])); // delta 1 evicted
        assert!(buffer.contains(&[2; 32]));
        assert!(buffer.contains(&[3; 32]));

        // Can now add delta 1 again (it was evicted)
        let result = buffer.push(make_test_delta(1));
        assert_eq!(result, PushResult::Evicted([2; 32])); // delta 2 gets evicted
    }

    #[test]
    fn test_capacity_below_recommended() {
        // Below recommended
        let buffer = DeltaBuffer::new(50, 0);
        assert!(buffer.is_capacity_below_recommended());

        // At recommended
        let buffer = DeltaBuffer::new(MIN_RECOMMENDED_CAPACITY, 0);
        assert!(!buffer.is_capacity_below_recommended());

        // Above recommended
        let buffer = DeltaBuffer::new(MIN_RECOMMENDED_CAPACITY + 1, 0);
        assert!(!buffer.is_capacity_below_recommended());
    }
}
