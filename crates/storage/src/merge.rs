//! CRDT merge logic for concurrent updates.
//!
//! This module implements merge strategies for resolving conflicts when
//! multiple nodes update the same data concurrently.

use borsh::{BorshDeserialize, BorshSerialize};

/// Attempts to merge two Borsh-serialized app state blobs using CRDT semantics.
///
/// This function tries to intelligently merge data by:
/// 1. Detecting the type (via trial deserialization)
/// 2. Applying type-specific CRDT merge rules
/// 3. Falling back to LWW if merge fails
///
/// # Arguments
/// * `existing` - The currently stored data
/// * `incoming` - The new data being applied
/// * `existing_ts` - Timestamp of existing data
/// * `incoming_ts` - Timestamp of incoming data
///
/// # Returns
/// Merged data as Borsh-serialized bytes
///
/// # Errors
/// Currently does not return errors as it uses LWW (Last-Write-Wins) strategy.
pub fn merge_root_state(
    existing: &[u8],
    incoming: &[u8],
    existing_ts: u64,
    incoming_ts: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // NOTE: We can't blindly deserialize without knowing the type.
    // The proper solution is that collections (UnorderedMap, Vector, Counter, etc.)
    // already handle CRDT merging by storing each entry with a unique ID.
    //
    // For root entities, concurrent updates should be rare since they typically
    // only contain collection references (which are IDs, not data).
    //
    // Fallback: use LWW
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
