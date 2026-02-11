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

    // CrdtType discriminant (1 byte conceptually, but we use debug string for simplicity)
    let crdt_discriminant = match &metadata.crdt_type {
        calimero_primitives::crdt::CrdtType::LwwRegister => 0u8,
        calimero_primitives::crdt::CrdtType::GCounter => 1u8,
        calimero_primitives::crdt::CrdtType::PnCounter => 2u8,
        calimero_primitives::crdt::CrdtType::Rga => 3u8,
        calimero_primitives::crdt::CrdtType::UnorderedMap => 4u8,
        calimero_primitives::crdt::CrdtType::UnorderedSet => 5u8,
        calimero_primitives::crdt::CrdtType::Vector => 6u8,
        calimero_primitives::crdt::CrdtType::UserStorage => 7u8,
        calimero_primitives::crdt::CrdtType::FrozenStorage => 8u8,
        calimero_primitives::crdt::CrdtType::Custom(_) => 9u8,
    };

    hasher.update([crdt_discriminant]);
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
#[derive(Debug, Default)]
pub struct DigestCache {
    /// Cached digest (None if invalidated).
    cached: Option<StateDigest>,
    /// Entities for digest computation.
    entities: Vec<DigestEntity>,
    /// Whether the entity list is sorted.
    sorted: bool,
}

impl DigestCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or update an entity.
    pub fn upsert(&mut self, entity: DigestEntity) {
        self.cached = None;
        self.sorted = false;

        // Remove existing if present
        self.entities.retain(|e| e.id != entity.id);
        self.entities.push(entity);
    }

    /// Remove an entity.
    pub fn remove(&mut self, id: &EntityId) {
        if self.entities.iter().any(|e| &e.id == id) {
            self.cached = None;
            self.entities.retain(|e| &e.id != id);
        }
    }

    /// Get the state digest (computing if necessary).
    pub fn digest(&mut self) -> StateDigest {
        if let Some(digest) = self.cached {
            return digest;
        }

        let digest = compute_state_digest(&self.entities);
        self.cached = Some(digest);
        digest
    }

    /// Clear the cache.
    pub fn clear(&mut self) {
        self.cached = None;
        self.entities.clear();
        self.sorted = false;
    }

    /// Get entity count.
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Get entity by ID.
    pub fn get(&self, id: &EntityId) -> Option<&DigestEntity> {
        self.entities.iter().find(|e| &e.id == id)
    }

    /// Iterate over entities.
    pub fn iter(&self) -> impl Iterator<Item = &DigestEntity> {
        self.entities.iter()
    }

    /// Get all entities (sorted by ID).
    pub fn entities_sorted(&mut self) -> &[DigestEntity] {
        if !self.sorted {
            self.entities.sort_by_key(|e| e.id);
            self.sorted = true;
        }
        &self.entities
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
}
