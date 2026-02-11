//! State digest computation for convergence checking.
//!
//! See spec §7 - State Digest and Hashing.

use sha2::{Digest, Sha256};

use super::actions::EntityMetadata;
use super::types::{EntityId, StateDigest};

/// Compute canonical metadata digest.
///
/// Fields serialized in fixed order as per spec §7.2.
pub fn hash_metadata(metadata: &EntityMetadata) -> [u8; 32] {
    let mut hasher = Sha256::new();

    // CrdtType discriminant
    match &metadata.crdt_type {
        calimero_primitives::crdt::CrdtType::LwwRegister => hasher.update([0u8]),
        calimero_primitives::crdt::CrdtType::GCounter => hasher.update([1u8]),
        calimero_primitives::crdt::CrdtType::PnCounter => hasher.update([2u8]),
        calimero_primitives::crdt::CrdtType::Rga => hasher.update([3u8]),
        calimero_primitives::crdt::CrdtType::UnorderedMap => hasher.update([4u8]),
        calimero_primitives::crdt::CrdtType::UnorderedSet => hasher.update([5u8]),
        calimero_primitives::crdt::CrdtType::Vector => hasher.update([6u8]),
        calimero_primitives::crdt::CrdtType::UserStorage => hasher.update([7u8]),
        calimero_primitives::crdt::CrdtType::FrozenStorage => hasher.update([8u8]),
        calimero_primitives::crdt::CrdtType::Custom(s) => {
            hasher.update([9u8]);
            // Include the custom type identifier to differentiate different custom types
            // Encode length first to prevent ambiguous serialization (e.g., "ab" + timestamp vs "a" + different timestamp)
            hasher.update((s.len() as u64).to_le_bytes());
            hasher.update(s.as_bytes());
        }
    };

    hasher.update(metadata.hlc_timestamp.to_le_bytes());
    hasher.update(metadata.version.to_le_bytes());
    hasher.update(metadata.collection_id);

    hasher.finalize().into()
}

/// Compute value digest.
pub fn hash_value(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Entity with data and metadata for digest computation.
#[derive(Clone, Debug)]
pub struct DigestEntity {
    /// Entity ID.
    pub id: EntityId,
    /// Entity data.
    pub data: Vec<u8>,
    /// Entity metadata.
    pub metadata: EntityMetadata,
}

/// Compute state digest from a collection of entities.
///
/// See spec §7.1 - Canonical State Digest.
///
/// Entities are sorted by EntityId for deterministic ordering.
pub fn compute_state_digest(entities: &[DigestEntity]) -> StateDigest {
    if entities.is_empty() {
        return StateDigest::ZERO;
    }

    // Sort by entity ID for deterministic ordering
    let mut sorted: Vec<_> = entities.iter().collect();
    sorted.sort_by_key(|e| e.id);

    let mut hasher = Sha256::new();

    for entity in sorted {
        // EntityId (32 bytes)
        hasher.update(entity.id.as_bytes());

        // ValueDigest = H(entity.data)
        let value_digest = hash_value(&entity.data);
        hasher.update(value_digest);

        // MetadataDigest = H(canonical_serialize(entity.metadata))
        let metadata_digest = hash_metadata(&entity.metadata);
        hasher.update(metadata_digest);
    }

    StateDigest::from_bytes(hasher.finalize().into())
}

/// Builder for incremental digest computation with caching.
///
/// Uses HashMap internally for O(1) amortized upsert/remove operations.
#[derive(Debug, Default)]
pub struct DigestCache {
    /// Cached digest (None if invalidated).
    cached: Option<StateDigest>,
    /// Entities by ID for O(1) lookup/upsert/remove.
    entities: std::collections::HashMap<EntityId, DigestEntity>,
}

impl DigestCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or update an entity. O(1) amortized.
    pub fn upsert(&mut self, entity: DigestEntity) {
        self.cached = None;
        self.entities.insert(entity.id, entity);
    }

    /// Remove an entity. O(1) amortized.
    pub fn remove(&mut self, id: &EntityId) {
        if self.entities.remove(id).is_some() {
            self.cached = None;
        }
    }

    /// Get the state digest (computing if necessary).
    pub fn digest(&mut self) -> StateDigest {
        if let Some(digest) = self.cached {
            return digest;
        }

        // Collect and sort for deterministic hashing
        let entities: Vec<_> = self.entities.values().cloned().collect();
        let digest = compute_state_digest(&entities);
        self.cached = Some(digest);
        digest
    }

    /// Clear the cache.
    pub fn clear(&mut self) {
        self.cached = None;
        self.entities.clear();
    }

    /// Get entity count.
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Get entity by ID. O(1).
    pub fn get(&self, id: &EntityId) -> Option<&DigestEntity> {
        self.entities.get(id)
    }

    /// Iterate over entities (unordered).
    pub fn iter(&self) -> impl Iterator<Item = &DigestEntity> {
        self.entities.values()
    }

    /// Get all entities sorted by ID.
    pub fn entities_sorted(&self) -> Vec<DigestEntity> {
        let mut entities: Vec<_> = self.entities.values().cloned().collect();
        entities.sort_by_key(|e| e.id);
        entities
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::crdt::CrdtType;

    fn make_entity(id: u64, data: &[u8]) -> DigestEntity {
        DigestEntity {
            id: EntityId::from_u64(id),
            data: data.to_vec(),
            metadata: EntityMetadata::new(CrdtType::LwwRegister, id * 100),
        }
    }

    #[test]
    fn test_empty_digest() {
        let digest = compute_state_digest(&[]);
        assert_eq!(digest, StateDigest::ZERO);
    }

    #[test]
    fn test_single_entity_digest() {
        let entity = make_entity(1, b"hello");
        let digest = compute_state_digest(&[entity]);
        assert_ne!(digest, StateDigest::ZERO);
    }

    #[test]
    fn test_digest_deterministic() {
        let entities = vec![make_entity(1, b"hello"), make_entity(2, b"world")];

        let digest1 = compute_state_digest(&entities);
        let digest2 = compute_state_digest(&entities);

        assert_eq!(digest1, digest2);
    }

    #[test]
    fn test_digest_order_independent() {
        let e1 = make_entity(1, b"hello");
        let e2 = make_entity(2, b"world");

        let digest1 = compute_state_digest(&[e1.clone(), e2.clone()]);
        let digest2 = compute_state_digest(&[e2, e1]);

        // Should be same because we sort by entity ID
        assert_eq!(digest1, digest2);
    }

    #[test]
    fn test_digest_different_data() {
        let e1 = make_entity(1, b"hello");
        let e2 = make_entity(1, b"world");

        let digest1 = compute_state_digest(&[e1]);
        let digest2 = compute_state_digest(&[e2]);

        assert_ne!(digest1, digest2);
    }

    #[test]
    fn test_digest_different_metadata() {
        let mut e1 = make_entity(1, b"hello");
        let mut e2 = make_entity(1, b"hello");

        e1.metadata.hlc_timestamp = 100;
        e2.metadata.hlc_timestamp = 200;

        let digest1 = compute_state_digest(&[e1]);
        let digest2 = compute_state_digest(&[e2]);

        assert_ne!(digest1, digest2);
    }

    #[test]
    fn test_digest_cache() {
        let mut cache = DigestCache::new();

        assert!(cache.is_empty());
        assert_eq!(cache.digest(), StateDigest::ZERO);

        cache.upsert(make_entity(1, b"hello"));
        let d1 = cache.digest();
        assert_ne!(d1, StateDigest::ZERO);

        // Cached
        let d2 = cache.digest();
        assert_eq!(d1, d2);

        // Update invalidates cache
        cache.upsert(make_entity(2, b"world"));
        let d3 = cache.digest();
        assert_ne!(d3, d1);

        // Remove
        cache.remove(&EntityId::from_u64(2));
        let d4 = cache.digest();
        assert_eq!(d4, d1); // Back to single entity
    }

    #[test]
    fn test_metadata_hash_different_crdt_types() {
        let m1 = EntityMetadata::new(CrdtType::LwwRegister, 100);
        let m2 = EntityMetadata::new(CrdtType::GCounter, 100);

        let h1 = hash_metadata(&m1);
        let h2 = hash_metadata(&m2);

        assert_ne!(h1, h2);
    }

    #[test]
    fn test_metadata_hash_different_custom_types() {
        // Different custom type identifiers should produce different hashes
        let m1 = EntityMetadata::new(CrdtType::Custom("type_a".to_string()), 100);
        let m2 = EntityMetadata::new(CrdtType::Custom("type_b".to_string()), 100);
        let m3 = EntityMetadata::new(CrdtType::Custom("type_a".to_string()), 100);

        let h1 = hash_metadata(&m1);
        let h2 = hash_metadata(&m2);
        let h3 = hash_metadata(&m3);

        // Different custom types should have different hashes
        assert_ne!(
            h1, h2,
            "Custom('type_a') and Custom('type_b') should differ"
        );

        // Same custom type should have same hash
        assert_eq!(h1, h3, "Custom('type_a') should match itself");
    }
}
