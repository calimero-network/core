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

use calimero_crypto::Nonce;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;

/// Default buffer capacity (10,000 deltas per context).
pub const DEFAULT_BUFFER_CAPACITY: usize = 10_000;

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
#[derive(Debug)]
pub struct DeltaBuffer {
    /// Buffered deltas (FIFO queue - oldest at front).
    deltas: std::collections::VecDeque<BufferedDelta>,
    /// HLC timestamp when buffering started.
    sync_start_hlc: u64,
    /// Maximum buffer size before eviction.
    capacity: usize,
    /// Number of deltas dropped due to buffer overflow (observable metric).
    drops: u64,
}

impl DeltaBuffer {
    /// Create a new delta buffer with specified capacity.
    #[must_use]
    pub fn new(capacity: usize, sync_start_hlc: u64) -> Self {
        Self {
            deltas: std::collections::VecDeque::with_capacity(capacity.min(1000)),
            sync_start_hlc,
            capacity,
            drops: 0,
        }
    }

    /// Add a delta to the buffer.
    ///
    /// If the buffer is full, the oldest delta is evicted (oldest-first policy)
    /// and the `drops` counter is incremented. This ensures we never reject
    /// incoming deltas but may lose old ones under extreme load.
    ///
    /// Returns `None` if the delta was added without eviction, or `Some(evicted_id)`
    /// containing the ID of the evicted delta if one was dropped.
    ///
    /// # Edge case: zero capacity
    ///
    /// If capacity is 0, the incoming delta is immediately dropped (not added)
    /// and its ID is returned. This is the correct behavior - a zero-capacity
    /// buffer cannot hold any deltas.
    pub fn push(&mut self, delta: BufferedDelta) -> Option<[u8; 32]> {
        // Handle zero capacity: drop incoming delta immediately
        if self.capacity == 0 {
            self.drops += 1;
            return Some(delta.id);
        }

        if self.deltas.len() >= self.capacity {
            // Evict oldest delta (front of queue)
            let evicted = self.deltas.pop_front();
            self.drops += 1;
            self.deltas.push_back(delta);
            // Return the evicted delta's ID (not the newly added one)
            evicted.map(|d| d.id)
        } else {
            self.deltas.push_back(delta);
            None
        }
    }

    /// Get all buffered deltas for replay, clearing the buffer.
    ///
    /// Returns deltas in FIFO order (oldest first), preserving causality.
    #[must_use]
    pub fn drain(&mut self) -> Vec<BufferedDelta> {
        self.deltas.drain(..).collect()
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

        let evicted = buffer.push(make_test_delta(1));
        assert!(evicted.is_none(), "Should add without eviction");
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
        assert!(buffer.push(make_test_delta(1)).is_none());
        assert!(buffer.push(make_test_delta(2)).is_none());
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
        assert!(buffer.push(make_test_delta(1)).is_none()); // No eviction
        assert!(buffer.push(make_test_delta(2)).is_none()); // No eviction
        assert_eq!(buffer.drops(), 0);

        // Third delta causes eviction of oldest (delta 1)
        let evicted = buffer.push(make_test_delta(3));
        assert_eq!(evicted, Some([1; 32]), "Should evict delta 1");
        assert_eq!(buffer.drops(), 1);
        assert_eq!(buffer.len(), 2);

        // Fourth delta causes another eviction (delta 2)
        let evicted = buffer.push(make_test_delta(4));
        assert_eq!(evicted, Some([2; 32]), "Should evict delta 2");
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

        // First push should drop immediately and return the incoming delta's ID
        let evicted = buffer.push(make_test_delta(1));
        assert_eq!(
            evicted,
            Some([1; 32]),
            "Zero capacity should drop incoming delta"
        );
        assert_eq!(buffer.drops(), 1);
        assert!(buffer.is_empty(), "Buffer should remain empty");
        assert_eq!(buffer.len(), 0);

        // Second push should also drop
        let evicted = buffer.push(make_test_delta(2));
        assert_eq!(evicted, Some([2; 32]));
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
}
