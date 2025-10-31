//! Last-Write-Wins Register - A CRDT for single values
//!
//! The LWW Register resolves conflicts by choosing the value with the latest timestamp.
//! When timestamps are equal, it uses node_id for deterministic tie-breaking.
//!
//! ## Use Cases
//!
//! - Document titles, usernames, settings
//! - Any field that should keep the "last write"
//! - Alternative to manual LWW logic in application code
//!
//! ## Example
//!
//! ```ignore
//! use calimero_storage::collections::LwwRegister;
//!
//! let mut title = LwwRegister::new("Draft".to_string());
//! title.set("Final".to_string());
//!
//! assert_eq!(title.get(), "Final");
//!
//! // Concurrent updates merge deterministically
//! let mut node1 = LwwRegister::new("Alice's version".to_string());
//! let node2 = LwwRegister::new("Bob's version".to_string());
//!
//! node1.merge(&node2); // Latest timestamp wins
//! ```

use borsh::{BorshDeserialize, BorshSerialize};

use crate::env;
use crate::logical_clock::HybridTimestamp;

/// Last-Write-Wins Register - a CRDT for single values
///
/// Automatically resolves conflicts by timestamp, with node_id tie-breaking.
/// Safe to use in concurrent multi-node environments.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct LwwRegister<T> {
    /// The current value
    value: T,
    /// HLC timestamp of last write
    timestamp: HybridTimestamp,
    /// Node that performed the write (for tie-breaking)
    node_id: [u8; 32],
}

impl<T> LwwRegister<T> {
    /// Create a new LWW register with the given value
    ///
    /// Uses current HLC timestamp and executor ID.
    pub fn new(value: T) -> Self {
        Self {
            value,
            timestamp: env::hlc_timestamp(),
            node_id: env::executor_id(),
        }
    }

    /// Create a new LWW register with explicit timestamp and node_id
    ///
    /// Useful for testing or manual construction.
    pub fn new_with_metadata(value: T, timestamp: HybridTimestamp, node_id: [u8; 32]) -> Self {
        Self {
            value,
            timestamp,
            node_id,
        }
    }

    /// Get the current value
    #[must_use]
    pub fn get(&self) -> &T {
        &self.value
    }

    /// Get a mutable reference to the value
    ///
    /// **WARNING:** Direct mutation bypasses timestamp updates!
    /// Use `set()` instead for proper CRDT behavior.
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }

    /// Set a new value (updates timestamp and node_id)
    pub fn set(&mut self, value: T) {
        self.value = value;
        self.timestamp = env::hlc_timestamp();
        self.node_id = env::executor_id();
    }

    /// Get the timestamp of the last write
    #[must_use]
    pub fn timestamp(&self) -> HybridTimestamp {
        self.timestamp
    }

    /// Get the node ID of the last write
    #[must_use]
    pub fn node_id(&self) -> [u8; 32] {
        self.node_id
    }

    /// Consume the register and return the inner value
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T: Clone> LwwRegister<T> {
    /// Merge with another register (CRDT merge operation)
    ///
    /// # Merge Rules
    ///
    /// 1. If `other.timestamp > self.timestamp` → take other's value
    /// 2. If timestamps equal → use node_id for tie-breaking (higher wins)
    /// 3. Otherwise → keep current value
    ///
    /// This ensures deterministic, conflict-free merging across all nodes.
    pub fn merge(&mut self, other: &Self) {
        // LWW rule with deterministic tie-breaking
        let should_update = other.timestamp > self.timestamp
            || (other.timestamp == self.timestamp && other.node_id > self.node_id);

        if should_update {
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
            self.node_id = other.node_id;
        }
    }

    /// Check if this register would be updated by merging with `other`
    ///
    /// Useful for detecting conflicts before applying merge.
    #[must_use]
    pub fn would_update(&self, other: &Self) -> bool {
        other.timestamp > self.timestamp
            || (other.timestamp == self.timestamp && other.node_id > self.node_id)
    }
}

impl<T: Default> Default for LwwRegister<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

// Deref for convenient read access without calling .get()
impl<T> std::ops::Deref for LwwRegister<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

// AsRef for automatic conversion to reference
impl<T> AsRef<T> for LwwRegister<T> {
    fn as_ref(&self) -> &T {
        &self.value
    }
}

// Borrow for compatibility with HashMap, BTreeMap, etc.
impl<T> std::borrow::Borrow<T> for LwwRegister<T> {
    fn borrow(&self) -> &T {
        &self.value
    }
}

// From inner value to create LwwRegister
impl<T> From<T> for LwwRegister<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

// Display for easy debugging
impl<T: std::fmt::Display> std::fmt::Display for LwwRegister<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}
