//! Decomposable implementations for CRDT collections
//!
//! Enables structured storage by decomposing collections into key-value entries.

use super::composite_key::CompositeKey;
use super::crdt_meta::{Decomposable, DecomposeError};
use super::{UnorderedMap, Vector};
use crate::store::StorageAdaptor;
use borsh::{BorshDeserialize, BorshSerialize};

// ============================================================================
// UnorderedMap - Decompose into entries
// ============================================================================

impl<K, V, S> Decomposable for UnorderedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + Clone + PartialEq,
    V: BorshSerialize + BorshDeserialize + Clone,
    S: StorageAdaptor,
{
    type Key = CompositeKey;
    type Value = V;

    fn decompose(&self) -> Result<Vec<(Self::Key, Self::Value)>, DecomposeError> {
        let entries = self
            .entries()
            .map_err(|e| DecomposeError::StorageError(format!("Failed to get entries: {:?}", e)))?;

        // Convert (K, V) to (CompositeKey, V)
        // For a nested map, we'll wrap the key in a CompositeKey
        let result = entries
            .into_iter()
            .map(|(k, v)| {
                // Serialize the key to bytes
                let key_bytes = borsh::to_vec(&k).map_err(|e| {
                    DecomposeError::StorageError(format!("Failed to serialize key: {:?}", e))
                })?;

                // Create a CompositeKey from the serialized key
                // For now, just use the key bytes directly
                // When nested inside another map, the outer key will be prepended
                Ok((CompositeKey::from(key_bytes), v))
            })
            .collect::<Result<Vec<_>, DecomposeError>>()?;

        Ok(result)
    }

    fn recompose(entries: Vec<(Self::Key, Self::Value)>) -> Result<Self, DecomposeError> {
        let mut map = UnorderedMap::new_internal();

        for (composite_key, value) in entries {
            // Deserialize the key from composite key bytes
            let key = borsh::from_slice::<K>(composite_key.as_bytes()).map_err(|e| {
                DecomposeError::StorageError(format!("Failed to deserialize key: {:?}", e))
            })?;

            map.insert(key, value)
                .map_err(|e| DecomposeError::StorageError(format!("Failed to insert: {:?}", e)))?;
        }

        Ok(map)
    }
}

// ============================================================================
// Vector - Decompose into indexed entries
// ============================================================================

// Note: Vector decomposition is complex due to operational transformation.
// For now, we'll implement a basic version. Full OT support requires Phase 3.

impl<T, S> Decomposable for Vector<T, S>
where
    T: BorshSerialize + BorshDeserialize + Clone,
    S: StorageAdaptor,
{
    type Key = CompositeKey;
    type Value = T;

    fn decompose(&self) -> Result<Vec<(Self::Key, Self::Value)>, DecomposeError> {
        // Get length
        let len = self
            .len()
            .map_err(|e| DecomposeError::StorageError(format!("Failed to get length: {:?}", e)))?;

        // Extract each element by index
        let mut result = Vec::with_capacity(len);
        for index in 0..len {
            if let Some(value) = self.get(index).map_err(|e| {
                DecomposeError::StorageError(format!("Failed to get index {}: {:?}", index, e))
            })? {
                // Create composite key with index
                let index_bytes = borsh::to_vec(&index).map_err(|e| {
                    DecomposeError::StorageError(format!("Failed to serialize index: {:?}", e))
                })?;

                result.push((CompositeKey::from(index_bytes), value));
            }
        }

        Ok(result)
    }

    fn recompose(entries: Vec<(Self::Key, Self::Value)>) -> Result<Self, DecomposeError> {
        // Note: This creates a Vector with MainStorage
        // For full generic support, Vector would need a from_entries method
        // For now, this limits Vector decompose/recompose to MainStorage

        // Sort by index first
        let mut indexed_entries: Vec<(usize, T)> = entries
            .into_iter()
            .map(|(composite_key, value)| {
                let index = borsh::from_slice::<usize>(composite_key.as_bytes()).map_err(|e| {
                    DecomposeError::StorageError(format!("Failed to deserialize index: {:?}", e))
                })?;
                Ok((index, value))
            })
            .collect::<Result<Vec<_>, DecomposeError>>()?;

        indexed_entries.sort_by_key(|(index, _)| *index);

        // Create a new vector using new_internal (now pub(super))
        let mut vector = Vector::new_internal();

        // Push values in order
        for (expected_index, value) in indexed_entries.into_iter().enumerate() {
            let (actual_index, val) = value;
            if actual_index != expected_index {
                return Err(DecomposeError::InvalidValue(format!(
                    "Vector has gap: expected index {}, got {}",
                    expected_index, actual_index
                )));
            }
            vector
                .push(val)
                .map_err(|e| DecomposeError::StorageError(format!("Failed to push: {:?}", e)))?;
        }

        Ok(vector)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::Root;
    use crate::env;

    #[test]
    fn test_unordered_map_decompose() {
        env::reset_for_testing();

        let mut map = Root::new(|| UnorderedMap::new());
        map.insert("key1".to_owned(), "value1".to_owned()).unwrap();
        map.insert("key2".to_owned(), "value2".to_owned()).unwrap();

        let entries = map.decompose().unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_unordered_map_roundtrip() {
        env::reset_for_testing();

        let mut original = Root::new(|| UnorderedMap::<String, u64>::new());
        original.insert("a".to_owned(), 1u64).unwrap();
        original.insert("b".to_owned(), 2u64).unwrap();
        original.insert("c".to_owned(), 3u64).unwrap();

        // Decompose
        let entries = original.decompose().unwrap();

        // Recompose
        let reconstructed = UnorderedMap::<String, u64>::recompose(entries).unwrap();

        // Verify
        assert_eq!(reconstructed.get(&"a".to_owned()).unwrap(), Some(1u64));
        assert_eq!(reconstructed.get(&"b".to_owned()).unwrap(), Some(2u64));
        assert_eq!(reconstructed.get(&"c".to_owned()).unwrap(), Some(3u64));
    }

    #[test]
    fn test_vector_decompose() {
        env::reset_for_testing();

        let mut vec = Root::new(|| Vector::new());
        vec.push(10u64).unwrap();
        vec.push(20u64).unwrap();
        vec.push(30u64).unwrap();

        let entries = vec.decompose().unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_vector_roundtrip() {
        env::reset_for_testing();

        let mut original = Root::new(|| Vector::<String>::new());
        original.push("first".to_owned()).unwrap();
        original.push("second".to_owned()).unwrap();
        original.push("third".to_owned()).unwrap();

        // Decompose
        let entries = original.decompose().unwrap();

        // Recompose
        let reconstructed = Vector::<String>::recompose(entries).unwrap();

        // Verify all values are present in correct order
        assert_eq!(reconstructed.get(0).unwrap(), Some("first".to_owned()));
        assert_eq!(reconstructed.get(1).unwrap(), Some("second".to_owned()));
        assert_eq!(reconstructed.get(2).unwrap(), Some("third".to_owned()));
        assert_eq!(reconstructed.len().unwrap(), 3);
    }
}
