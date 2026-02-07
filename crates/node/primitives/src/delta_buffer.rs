//! Delta buffering for sync scenarios.
//!
//! When a snapshot sync is in progress, incoming deltas are buffered so they
//! can be replayed after the snapshot completes. This ensures that:
//! 1. Deltas arriving during sync aren't lost
//! 2. Event handlers can execute for buffered deltas after context is initialized

use calimero_crypto::Nonce;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;

/// A single buffered delta.
///
/// Contains ALL fields needed for replay after snapshot sync completes.
/// Previously missing fields (nonce, author_id, root_hash, events) caused
/// data loss because deltas couldn't be decrypted or processed.
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
#[derive(Debug)]
pub struct DeltaBuffer {
    /// Buffered deltas.
    deltas: Vec<BufferedDelta>,
    /// HLC timestamp when buffering started.
    sync_start_hlc: u64,
    /// Maximum buffer size before overflow.
    max_size: usize,
}

/// Error when delta buffer is full.
#[derive(Debug, Clone)]
pub struct DeltaBufferFull {
    /// Number of deltas already buffered.
    pub buffered_count: usize,
}

impl std::fmt::Display for DeltaBufferFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Delta buffer full ({} deltas buffered)",
            self.buffered_count
        )
    }
}

impl std::error::Error for DeltaBufferFull {}

impl DeltaBuffer {
    /// Create a new delta buffer with specified capacity.
    #[must_use]
    pub fn new(max_size: usize, sync_start_hlc: u64) -> Self {
        Self {
            deltas: Vec::with_capacity(max_size.min(1000)),
            sync_start_hlc,
            max_size,
        }
    }

    /// Add a delta to the buffer.
    ///
    /// Returns `Err(DeltaBufferFull)` if buffer is at capacity.
    pub fn push(&mut self, delta: BufferedDelta) -> Result<(), DeltaBufferFull> {
        if self.deltas.len() >= self.max_size {
            return Err(DeltaBufferFull {
                buffered_count: self.deltas.len(),
            });
        }
        self.deltas.push(delta);
        Ok(())
    }

    /// Get all buffered deltas for replay, clearing the buffer.
    #[must_use]
    pub fn drain(&mut self) -> Vec<BufferedDelta> {
        std::mem::take(&mut self.deltas)
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

        buffer.push(make_test_delta(1)).unwrap();
        assert_eq!(buffer.len(), 1);

        let drained = buffer.drain();
        assert_eq!(drained.len(), 1);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_buffer_overflow() {
        let mut buffer = DeltaBuffer::new(2, 0);
        buffer.push(make_test_delta(1)).unwrap();
        buffer.push(make_test_delta(2)).unwrap();

        let result = buffer.push(make_test_delta(3));
        assert!(result.is_err());
    }
}
