//! CrdtMeta implementations for all CRDT types in the storage layer
//!
//! This module implements the CrdtMeta trait for:
//! - LwwRegister
//! - Counter
//! - ReplicatedGrowableArray (RGA)
//! - UnorderedMap
//! - UnorderedSet
//! - Vector

use super::crdt_meta::{CrdtMeta, CrdtType, MergeError, Mergeable, StorageStrategy};
use super::{Counter, LwwRegister, ReplicatedGrowableArray, UnorderedMap, UnorderedSet, Vector};
use crate::store::StorageAdaptor;

// ============================================================================
// Primitive Types - Implement Mergeable with LWW semantics
// ============================================================================

// For primitive types, we can't do true CRDT merging because they don't have
// timestamps or other metadata. We implement Mergeable with simple LWW semantics
// to allow them to be used in nested CRDT structures like Map<K, String>.
//
// **Important:** This is a fallback! For proper conflict resolution with timestamps,
// use LwwRegister<T> instead of bare primitives.
//
// Performance: O(1) - just overwrites the value
// Correctness: LWW (may lose concurrent updates to same field)
// When called: Only during root-level merge (rare)

macro_rules! impl_mergeable_lww {
    ($($t:ty),*) => {
        $(
            impl Mergeable for $t {
                fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
                    // LWW: just take the other value
                    // ⚠️  Warning: This loses information! Use LwwRegister<T> for proper timestamps
                    *self = other.clone();
                    Ok(())
                }
            }
        )*
    };
}

// Implement for common primitive types
impl_mergeable_lww!(String, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, bool, char);

// Note: Vec and Option Mergeable implementations commented out for now
// They require Clone which creates issues with collections
// Most apps don't have Vec/Option at root level anyway
// Can be re-enabled if needed with proper Clone implementations

// impl<T: Clone> Mergeable for Vec<T> {
//     fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
//         // LWW for vectors: take the other value
//         *self = other.clone();
//         Ok(())
//     }
// }

// impl<T: Clone> Mergeable for Option<T> {
//     fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
//         // LWW for options: take the other value if it's Some
//         if other.is_some() {
//             *self = other.clone();
//         }
//         Ok(())
//     }
// }

// ============================================================================
// LwwRegister
// ============================================================================

impl<T> CrdtMeta for LwwRegister<T> {
    fn crdt_type() -> CrdtType {
        CrdtType::LwwRegister
    }

    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Blob // LWW stores as single value
    }

    fn can_contain_crdts() -> bool {
        false // Register wraps a value, doesn't contain other CRDTs
    }
}

impl<T: Clone> Mergeable for LwwRegister<T> {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Use existing merge implementation
        LwwRegister::merge(self, other);
        Ok(())
    }
}

// ============================================================================
// Counter
// ============================================================================

impl CrdtMeta for Counter {
    fn crdt_type() -> CrdtType {
        CrdtType::Counter
    }

    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Blob // Counter stores as single u64
    }

    fn can_contain_crdts() -> bool {
        false // Counter is a primitive value
    }
}

impl Mergeable for Counter {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Counter merge: sum the values
        let other_value = other.value().map_err(|e| {
            MergeError::StorageError(format!("Failed to get counter value: {:?}", e))
        })?;

        // Increment by other's value
        for _ in 0..other_value {
            self.increment()
                .map_err(|e| MergeError::StorageError(format!("Failed to increment: {:?}", e)))?;
        }
        Ok(())
    }
}

// ============================================================================
// ReplicatedGrowableArray (RGA)
// ============================================================================

impl CrdtMeta for ReplicatedGrowableArray {
    fn crdt_type() -> CrdtType {
        CrdtType::Rga
    }

    fn storage_strategy() -> StorageStrategy {
        // RGA could be structured (store each character separately)
        // but for now, treat as blob for simplicity
        StorageStrategy::Blob
    }

    fn can_contain_crdts() -> bool {
        false // RGA contains characters, not CRDTs
    }
}

impl Mergeable for ReplicatedGrowableArray {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        let _ = other; // RGA merge is handled at the storage layer through tombstone preservation
                       // The UnorderedMap inside RGA already handles the merge
                       // This is a no-op at this level
        Ok(())
    }
}

// ============================================================================
// UnorderedMap
// ============================================================================

impl<K, V, S> CrdtMeta for UnorderedMap<K, V, S>
where
    S: StorageAdaptor,
{
    fn crdt_type() -> CrdtType {
        CrdtType::UnorderedMap
    }

    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured // Maps should decompose into entries
    }

    fn can_contain_crdts() -> bool {
        true // Map can contain CRDT values!
    }
}

impl<K, V, S> Mergeable for UnorderedMap<K, V, S>
where
    K: borsh::BorshSerialize + borsh::BorshDeserialize + AsRef<[u8]> + Clone + PartialEq,
    V: borsh::BorshSerialize + borsh::BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    /// Merge two maps entry-by-entry
    ///
    /// # Performance
    ///
    /// - **Cost:** O(N) where N = number of entries in `other`
    /// - **When called:** Only during root-level merge (rare)
    /// - **NOT called:** On local operations or element-level sync
    ///
    /// # Algorithm
    ///
    /// For each entry in `other`:
    /// - If key exists in both: recursively merge values (preserves both updates)
    /// - If key only in other: add it (add-wins semantics)
    /// - If key only in ours: keep it (implicit add-wins)
    ///
    /// This ensures ALL concurrent updates are preserved, preventing divergence.
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Iterate all entries in the other map
        // Performance: O(N) but only called during rare root conflicts
        let other_entries = other
            .entries()
            .map_err(|e| MergeError::StorageError(format!("Failed to get entries: {:?}", e)))?;

        for (key, other_value) in other_entries {
            if let Some(mut our_value) = self
                .get(&key)
                .map_err(|e| MergeError::StorageError(format!("Failed to get value: {:?}", e)))?
            {
                // Key exists in both - recursively merge values
                // This is where nested CRDT merging happens!
                our_value.merge(&other_value)?;
                drop(
                    self.insert(key, our_value).map_err(|e| {
                        MergeError::StorageError(format!("Failed to insert: {:?}", e))
                    })?,
                );
            } else {
                // Key only in other - add it (add-wins semantics)
                drop(
                    self.insert(key, other_value).map_err(|e| {
                        MergeError::StorageError(format!("Failed to insert: {:?}", e))
                    })?,
                );
            }
        }

        Ok(())
    }
}

// ============================================================================
// UnorderedSet
// ============================================================================

impl<T, S> CrdtMeta for UnorderedSet<T, S>
where
    S: StorageAdaptor,
{
    fn crdt_type() -> CrdtType {
        CrdtType::UnorderedSet
    }

    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured // Sets decompose into elements
    }

    fn can_contain_crdts() -> bool {
        false // Set elements are values, not CRDTs
    }
}

impl<T, S> Mergeable for UnorderedSet<T, S>
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize + AsRef<[u8]> + Clone + PartialEq,
    S: StorageAdaptor,
{
    /// Merge two sets using union (add-wins) semantics
    ///
    /// # Strategy
    ///
    /// - Add all elements from `other` to `self`
    /// - Duplicate elements are deduplicated (set semantics)
    /// - No removal - add-wins CRDT semantics
    ///
    /// # Performance
    ///
    /// - **Cost:** O(N) where N = number of elements in `other`
    /// - **When called:** Only during root-level merge (rare)
    ///
    /// # Use Cases
    ///
    /// - ✅ Perfect for: Unique users, tags, IDs
    /// - ✅ Works when nested: Map<K, Set<V>>
    /// - ❌ Avoid for: CRDT values (use Map instead)
    ///
    /// See: crates/storage/src/collections/set_merge.md
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Set merge: union (add-wins semantics)
        // All elements from both sets are preserved
        let other_values = other
            .iter()
            .map_err(|e| MergeError::StorageError(format!("Failed to get values: {:?}", e)))?;

        for value in other_values {
            // Insert returns true if new, false if already exists
            // We don't care - idempotent add-wins semantics
            let _ = self
                .insert(value)
                .map_err(|e| MergeError::StorageError(format!("Failed to insert: {:?}", e)))?;
        }

        Ok(())
    }
}

// ============================================================================
// Vector
// ============================================================================

impl<T, S> CrdtMeta for Vector<T, S>
where
    S: StorageAdaptor,
{
    fn crdt_type() -> CrdtType {
        CrdtType::Vector
    }

    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured // Vectors decompose into indexed entries
    }

    fn can_contain_crdts() -> bool {
        true // Vector can contain CRDT values!
    }
}

impl<T, S> Mergeable for Vector<T, S>
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    /// Merge two vectors using element-wise strategy
    ///
    /// # Strategy
    ///
    /// - **Same length:** Merge element-by-element at same indices
    /// - **Different length:** Take longer vector (LWW semantics)
    /// - **Nested CRDTs:** Recursively merge values at same index
    ///
    /// # Performance
    ///
    /// - **Cost:** O(min(N, M)) where N, M = vector lengths
    /// - **When called:** Only during root-level merge (rare)
    ///
    /// # Limitations
    ///
    /// This is a simple approach suitable for append-heavy workloads.
    /// Concurrent inserts at arbitrary positions may conflict.
    /// For full positional CRDT, use RGA for text or implement OT.
    ///
    /// See: crates/storage/src/collections/vector_merge.md
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        let our_len = self
            .len()
            .map_err(|e| MergeError::StorageError(format!("Failed to get length: {:?}", e)))?;
        let their_len = other.len().map_err(|e| {
            MergeError::StorageError(format!("Failed to get other length: {:?}", e))
        })?;

        // Merge elements at same indices
        let min_len = our_len.min(their_len);
        for i in 0..min_len {
            if let Some(mut our_value) = self
                .get(i)
                .map_err(|e| MergeError::StorageError(format!("Failed to get element: {:?}", e)))?
            {
                if let Some(their_value) = other.get(i).map_err(|e| {
                    MergeError::StorageError(format!("Failed to get other element: {:?}", e))
                })? {
                    // Recursively merge values at same index
                    our_value.merge(&their_value)?;
                    drop(self.update(i, our_value).map_err(|e| {
                        MergeError::StorageError(format!("Failed to update element: {:?}", e))
                    })?);
                }
            }
        }

        // If other is longer, append remaining elements (LWW: take their additions)
        if their_len > our_len {
            for i in our_len..their_len {
                if let Some(value) = other.get(i).map_err(|e| {
                    MergeError::StorageError(format!("Failed to get other element: {:?}", e))
                })? {
                    self.push(value).map_err(|e| {
                        MergeError::StorageError(format!("Failed to push element: {:?}", e))
                    })?;
                }
            }
        }
        // If we're longer, keep our additional elements (LWW: keep ours)

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env;

    #[test]
    fn test_lww_register_is_crdt() {
        assert!(LwwRegister::<String>::is_crdt());
        assert_eq!(LwwRegister::<String>::crdt_type(), CrdtType::LwwRegister);
        assert!(!LwwRegister::<String>::can_contain_crdts());
    }

    #[test]
    fn test_counter_is_crdt() {
        assert!(Counter::is_crdt());
        assert_eq!(Counter::crdt_type(), CrdtType::Counter);
        assert!(!Counter::can_contain_crdts());
    }

    #[test]
    fn test_rga_is_crdt() {
        assert!(ReplicatedGrowableArray::is_crdt());
        assert_eq!(ReplicatedGrowableArray::crdt_type(), CrdtType::Rga);
        assert!(!ReplicatedGrowableArray::can_contain_crdts());
    }

    #[test]
    fn test_map_can_contain_crdts() {
        type TestMap = UnorderedMap<String, Counter>;
        assert!(TestMap::is_crdt());
        assert_eq!(TestMap::crdt_type(), CrdtType::UnorderedMap);
        assert!(TestMap::can_contain_crdts()); // Maps CAN contain CRDTs!
    }

    #[test]
    fn test_counter_merge() {
        env::reset_for_testing();

        let mut c1 = Counter::new();
        c1.increment().unwrap();
        c1.increment().unwrap();

        let mut c2 = Counter::new();
        c2.increment().unwrap();

        c1.merge(&c2).unwrap();

        // Should sum: 2 + 1 = 3
        assert_eq!(c1.value().unwrap(), 3);
    }

    #[test]
    fn test_lww_register_merge() {
        env::reset_for_testing();

        let mut reg1 = LwwRegister::new("Alice".to_owned());
        std::thread::sleep(std::time::Duration::from_millis(1));

        let reg2 = LwwRegister::new("Bob".to_owned());

        // Use the Mergeable trait method
        Mergeable::merge(&mut reg1, &reg2).unwrap();

        // Later timestamp wins
        assert_eq!(reg1.get(), "Bob");
    }

    #[test]
    fn test_vector_merge_same_length() {
        env::reset_for_testing();

        // Create two vectors with counters
        let mut vec1 = Vector::new();
        let mut c1 = Counter::new();
        c1.increment().unwrap();
        c1.increment().unwrap(); // value = 2
        vec1.push(c1).unwrap();

        let mut c2 = Counter::new();
        c2.increment().unwrap(); // value = 1
        vec1.push(c2).unwrap();

        // Create second vector
        let mut vec2 = Vector::new();
        let mut c3 = Counter::new();
        c3.increment().unwrap(); // value = 1
        vec2.push(c3).unwrap();

        let mut c4 = Counter::new();
        c4.increment().unwrap();
        c4.increment().unwrap();
        c4.increment().unwrap(); // value = 3
        vec2.push(c4).unwrap();

        // Merge
        vec1.merge(&vec2).unwrap();

        // Verify: Counters at same indices should sum
        assert_eq!(vec1.len().unwrap(), 2);
        assert_eq!(vec1.get(0).unwrap().unwrap().value().unwrap(), 3); // 2 + 1
        assert_eq!(vec1.get(1).unwrap().unwrap().value().unwrap(), 4); // 1 + 3
    }

    #[test]
    fn test_vector_merge_different_length() {
        env::reset_for_testing();

        // vec1 = [Counter(2)]
        let mut vec1 = Vector::new();
        let mut c1 = Counter::new();
        c1.increment().unwrap();
        c1.increment().unwrap();
        vec1.push(c1).unwrap();

        // vec2 = [Counter(3), Counter(5), Counter(7)]
        let mut vec2 = Vector::new();
        let mut c2 = Counter::new();
        c2.increment().unwrap();
        c2.increment().unwrap();
        c2.increment().unwrap();
        vec2.push(c2).unwrap();

        let mut c3 = Counter::new();
        for _ in 0..5 {
            c3.increment().unwrap();
        }
        vec2.push(c3).unwrap();

        let mut c4 = Counter::new();
        for _ in 0..7 {
            c4.increment().unwrap();
        }
        vec2.push(c4).unwrap();

        // Merge
        vec1.merge(&vec2).unwrap();

        // Verify: First element merged, others appended
        assert_eq!(vec1.len().unwrap(), 3);
        assert_eq!(vec1.get(0).unwrap().unwrap().value().unwrap(), 5); // 2 + 3
        assert_eq!(vec1.get(1).unwrap().unwrap().value().unwrap(), 5); // Added from vec2
        assert_eq!(vec1.get(2).unwrap().unwrap().value().unwrap(), 7); // Added from vec2
    }

    #[test]
    fn test_vector_merge_with_lww_registers() {
        env::reset_for_testing();

        // vec1 = [LwwRegister("A")]
        let mut vec1 = Vector::new();
        vec1.push(LwwRegister::new("A".to_owned())).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(1));

        // vec2 = [LwwRegister("B")]
        let mut vec2 = Vector::new();
        vec2.push(LwwRegister::new("B".to_owned())).unwrap();

        // Merge
        vec1.merge(&vec2).unwrap();

        // Verify: Later timestamp wins
        assert_eq!(vec1.len().unwrap(), 1);
        assert_eq!(vec1.get(0).unwrap().unwrap().get(), "B");
    }

    #[test]
    fn test_set_merge_disjoint() {
        env::reset_for_testing();

        use super::UnorderedSet;

        // set1 = {"alice", "bob"}
        let mut set1 = UnorderedSet::new();
        set1.insert("alice".to_owned()).unwrap();
        set1.insert("bob".to_owned()).unwrap();

        // set2 = {"charlie", "dave"}
        let mut set2 = UnorderedSet::new();
        set2.insert("charlie".to_owned()).unwrap();
        set2.insert("dave".to_owned()).unwrap();

        // Merge
        set1.merge(&set2).unwrap();

        // Verify: Union of both sets
        assert!(set1.contains(&"alice".to_owned()).unwrap());
        assert!(set1.contains(&"bob".to_owned()).unwrap());
        assert!(set1.contains(&"charlie".to_owned()).unwrap());
        assert!(set1.contains(&"dave".to_owned()).unwrap());
    }

    #[test]
    fn test_set_merge_overlapping() {
        env::reset_for_testing();

        use super::UnorderedSet;

        // set1 = {"alice", "bob", "charlie"}
        let mut set1 = UnorderedSet::new();
        set1.insert("alice".to_owned()).unwrap();
        set1.insert("bob".to_owned()).unwrap();
        set1.insert("charlie".to_owned()).unwrap();

        // set2 = {"bob", "charlie", "dave"}
        let mut set2 = UnorderedSet::new();
        set2.insert("bob".to_owned()).unwrap();
        set2.insert("charlie".to_owned()).unwrap();
        set2.insert("dave".to_owned()).unwrap();

        // Merge
        set1.merge(&set2).unwrap();

        // Verify: Union (duplicates deduplicated)
        assert!(set1.contains(&"alice".to_owned()).unwrap());
        assert!(set1.contains(&"bob".to_owned()).unwrap());
        assert!(set1.contains(&"charlie".to_owned()).unwrap());
        assert!(set1.contains(&"dave".to_owned()).unwrap());
    }

    #[test]
    fn test_set_merge_empty() {
        env::reset_for_testing();

        use super::UnorderedSet;

        // set1 = {"alice"}
        let mut set1 = UnorderedSet::new();
        set1.insert("alice".to_owned()).unwrap();

        // set2 = {} (empty)
        let set2 = UnorderedSet::<String>::new();

        // Merge
        set1.merge(&set2).unwrap();

        // Verify: set1 unchanged
        assert!(set1.contains(&"alice".to_owned()).unwrap());
    }
}
