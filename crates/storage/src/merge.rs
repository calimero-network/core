//! CRDT merge logic for concurrent updates.
//!
//! This module implements merge strategies for resolving conflicts when
//! multiple nodes update the same data concurrently.

pub mod registry;
pub use registry::{register_crdt_merge, try_merge_registered};

#[cfg(test)]
pub use registry::clear_merge_registry;

use borsh::{BorshDeserialize, BorshSerialize};

/// Attempts to merge two Borsh-serialized app state blobs using CRDT semantics.
///
/// # When is This Called?
///
/// **ONLY during remote synchronization**, specifically:
/// 1. When receiving a remote delta that updates the ROOT entity
/// 2. When concurrent updates to root state occur (same timestamp)
/// 3. NOT on local operations (those are O(1) direct writes)
///
/// # Performance
///
/// - **Local operations:** O(1) - this function is NOT called
/// - **Remote sync (different entities):** O(1) - this function is NOT called
/// - **Remote sync (root conflict):** O(N) - this function IS called
///   - Where N = number of root-level fields
///   - Frequency: RARE (only on concurrent root modifications)
///   - Typically: N = 10-100 fields
///   - Network latency >> merge time
///
/// # Strategy
///
/// 1. **Try registered merge:** If app called `register_crdt_merge()`, use type-specific merge
/// 2. **Fallback to LWW:** If no registered merge, use Last-Write-Wins
///
/// # Arguments
/// * `existing` - The currently stored state (Borsh-serialized)
/// * `incoming` - The new state being synced (Borsh-serialized)
/// * `existing_ts` - Timestamp of existing state
/// * `incoming_ts` - Timestamp of incoming state
///
/// # Returns
/// Merged state as Borsh-serialized bytes
///
/// # Errors
/// Returns error if merge fails (falls back to LWW in that case)
pub fn merge_root_state(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Try registered CRDT merge functions first
    // This enables automatic nested CRDT merging when apps use #[app::state]
    if let Some(result) = try_merge_registered(existing, incoming, existing_ts, incoming_ts) {
        return result;
    }

    // NOTE: We can't blindly deserialize without knowing the type.
    // The collections (UnorderedMap, Vector, Counter, etc.) already handle
    // CRDT merging through their own element IDs and storage mechanisms.
    //
    // For root entities, concurrent updates should be rare since most operations
    // target nested entities (RGA characters, Map entries, etc.) which have their
    // own IDs and merge independently.
    //
    // Fallback: use LWW if no registered merge function
    // This is safe for simple apps or backward compatibility
    if incoming_ts >= existing_ts {
        Ok(incoming.to_vec())
    } else {
        Ok(existing.to_vec())
    }
}

/// Trait for app state types that need custom CRDT merge.
///
/// Implement this on your app's root state type to enable proper
/// concurrent update resolution.
///
/// # Example
///
/// ```ignore
/// #[derive(BorshSerialize, BorshDeserialize)]
/// struct MyAppState {
///     counter: GCounter,
///     items: UnorderedMap<String, String>,
/// }
///
/// impl CrdtMerge for MyAppState {
///     fn crdt_merge(&mut self, other: &Self) {
///         // Merge G-Counter
///         self.counter.merge(&other.counter);
///         
///         // UnorderedMap uses LWW per-key (handled by storage layer)
///     }
/// }
/// ```
pub trait CrdtMerge: BorshSerialize + BorshDeserialize {
    /// Merge another instance into self using CRDT semantics.
    fn crdt_merge(&mut self, other: &Self);
}
