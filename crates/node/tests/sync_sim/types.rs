//! Core types for the simulation framework.

use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};

/// Node identifier in the simulation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub String);

impl NodeId {
    /// Create a new node ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get the ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for NodeId {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// Unique message identifier for deduplication.
///
/// See spec §4.1 - Message Identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub struct MessageId {
    /// Sender node.
    pub sender: String,
    /// Sender's session (increments on restart).
    pub session: u64,
    /// Monotonic sequence within session.
    pub seq: u64,
}

impl MessageId {
    /// Create a new message ID.
    pub fn new(sender: impl Into<String>, session: u64, seq: u64) -> Self {
        Self {
            sender: sender.into(),
            session,
            seq,
        }
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.sender, self.session, self.seq)
    }
}

/// Timer identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimerId(pub u64);

impl TimerId {
    /// Create a new timer ID.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Timer kind for convergence checking.
///
/// See spec §8.1 - C4: sync timers only block convergence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerKind {
    /// Sync protocol timer (affects convergence check).
    Sync,
    /// Background/housekeeping timer (does not affect convergence).
    Housekeeping,
}

/// Entity identifier (32 bytes).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize)]
pub struct EntityId(pub [u8; 32]);

impl EntityId {
    /// Create from bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Create from a u64 (for testing).
    pub fn from_u64(n: u64) -> Self {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&n.to_le_bytes());
        Self(bytes)
    }

    /// Get as bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityId({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}{:02x}..{:02x}{:02x}",
            self.0[0], self.0[1], self.0[30], self.0[31]
        )
    }
}

impl From<[u8; 32]> for EntityId {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<u64> for EntityId {
    fn from(n: u64) -> Self {
        Self::from_u64(n)
    }
}

/// Delta identifier (32 bytes, typically a hash).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize)]
pub struct DeltaId(pub [u8; 32]);

impl DeltaId {
    /// Create from bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get as bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Zero/genesis delta ID.
    pub const ZERO: DeltaId = DeltaId([0; 32]);
}

impl fmt::Debug for DeltaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeltaId({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

impl From<[u8; 32]> for DeltaId {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// State digest for convergence checking.
///
/// See spec §7 - State Digest and Hashing.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StateDigest(pub [u8; 32]);

impl StateDigest {
    /// Create from bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get as bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Zero digest (empty state).
    pub const ZERO: StateDigest = StateDigest([0; 32]);
}

impl fmt::Debug for StateDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StateDigest({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

impl From<[u8; 32]> for StateDigest {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id() {
        let id = NodeId::new("alice");
        assert_eq!(id.as_str(), "alice");
        assert_eq!(format!("{id}"), "alice");
    }

    #[test]
    fn test_message_id() {
        let mid = MessageId::new("alice", 1, 42);
        assert_eq!(mid.sender, "alice");
        assert_eq!(mid.session, 1);
        assert_eq!(mid.seq, 42);
        assert_eq!(format!("{mid}"), "alice:1:42");
    }

    #[test]
    fn test_entity_id_from_u64() {
        let id = EntityId::from_u64(12345);
        let bytes = id.as_bytes();

        // First 8 bytes are the u64 in little endian
        let n = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        assert_eq!(n, 12345);

        // Rest are zeros
        assert!(bytes[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn test_entity_id_ordering() {
        let id1 = EntityId::from_u64(1);
        let id2 = EntityId::from_u64(2);
        let id3 = EntityId::from_u64(1);

        assert!(id1 < id2);
        assert_eq!(id1, id3);
    }
}
