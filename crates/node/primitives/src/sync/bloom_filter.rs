//! BloomFilter sync types (CIP Appendix B - Protocol Selection Matrix).
//!
//! Types for Bloom filter-based synchronization for large trees with small divergence.

use borsh::{BorshDeserialize, BorshSerialize};

use super::hash_comparison::TreeLeafData;

// =============================================================================
// Constants
// =============================================================================

/// Default false positive rate for Bloom filters.
pub const DEFAULT_BLOOM_FP_RATE: f32 = 0.01; // 1%

/// Minimum bits per element for reasonable FP rate.
const MIN_BITS_PER_ELEMENT: usize = 8;

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;

/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x100000001b3;

// =============================================================================
// Bloom Filter
// =============================================================================

/// A Bloom filter for delta/entity ID membership testing.
///
/// CRITICAL: Uses FNV-1a hash for consistency across nodes.
/// POC Bug 5: Hash mismatch when one node used SipHash.
///
/// Use this for sync when:
/// - entity_count > 50
/// - divergence < 10%
/// - Want to minimize round trips (O(1) diff detection)
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeltaIdBloomFilter {
    /// Bit array (packed as bytes).
    bits: Vec<u8>,
    /// Number of bits in the filter.
    num_bits: usize,
    /// Number of hash functions to use.
    num_hashes: u8,
    /// Number of items inserted.
    item_count: usize,
}

impl DeltaIdBloomFilter {
    /// Create a new Bloom filter sized for expected items and FP rate.
    ///
    /// # Arguments
    /// * `expected_items` - Expected number of items to insert
    /// * `fp_rate` - Desired false positive rate (0.0 to 1.0)
    #[must_use]
    pub fn new(expected_items: usize, fp_rate: f32) -> Self {
        // Calculate optimal number of bits: m = -n * ln(p) / (ln(2)^2)
        let ln2_sq = std::f64::consts::LN_2 * std::f64::consts::LN_2;
        let num_bits = if expected_items == 0 {
            64 // Minimum size
        } else {
            let m = -(expected_items as f64) * (fp_rate as f64).ln() / ln2_sq;
            (m.ceil() as usize).max(expected_items * MIN_BITS_PER_ELEMENT)
        };

        // Calculate optimal number of hashes: k = (m/n) * ln(2)
        let num_hashes = if expected_items == 0 {
            4
        } else {
            let k = (num_bits as f64 / expected_items as f64) * std::f64::consts::LN_2;
            (k.ceil() as u8).clamp(1, 16)
        };

        let num_bytes = (num_bits + 7) / 8;

        Self {
            bits: vec![0; num_bytes],
            num_bits,
            num_hashes,
            item_count: 0,
        }
    }

    /// Create a filter with explicit parameters.
    #[must_use]
    pub fn with_params(num_bits: usize, num_hashes: u8) -> Self {
        let num_bytes = (num_bits + 7) / 8;
        Self {
            bits: vec![0; num_bytes],
            num_bits,
            num_hashes,
            item_count: 0,
        }
    }

    /// FNV-1a hash function.
    ///
    /// CRITICAL: This MUST be used by all nodes for consistency.
    /// Do NOT use DefaultHasher (SipHash) or other hash functions.
    #[must_use]
    pub fn hash_fnv1a(data: &[u8]) -> u64 {
        let mut hash: u64 = FNV_OFFSET_BASIS;
        for byte in data {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }

    /// Compute hash positions using double hashing technique.
    fn compute_positions(&self, id: &[u8; 32]) -> Vec<usize> {
        let h1 = Self::hash_fnv1a(id);
        let h2 = Self::hash_fnv1a(&[id.as_slice(), &[0xFF]].concat());

        (0..self.num_hashes as u64)
            .map(|i| {
                let combined = h1.wrapping_add(i.wrapping_mul(h2));
                (combined as usize) % self.num_bits
            })
            .collect()
    }

    /// Insert an ID into the filter.
    pub fn insert(&mut self, id: &[u8; 32]) {
        let positions = self.compute_positions(id);
        for pos in positions {
            let byte_idx = pos / 8;
            let bit_idx = pos % 8;
            self.bits[byte_idx] |= 1 << bit_idx;
        }
        self.item_count += 1;
    }

    /// Check if an ID might be in the filter.
    ///
    /// Returns `true` if the ID is possibly in the set (may be false positive).
    /// Returns `false` if the ID is definitely not in the set.
    #[must_use]
    pub fn contains(&self, id: &[u8; 32]) -> bool {
        let positions = self.compute_positions(id);
        for pos in positions {
            let byte_idx = pos / 8;
            let bit_idx = pos % 8;
            if self.bits[byte_idx] & (1 << bit_idx) == 0 {
                return false;
            }
        }
        true
    }

    /// Get the number of items inserted.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.item_count
    }

    /// Get the filter size in bits.
    #[must_use]
    pub fn bit_count(&self) -> usize {
        self.num_bits
    }

    /// Get the number of hash functions.
    #[must_use]
    pub fn hash_count(&self) -> u8 {
        self.num_hashes
    }

    /// Estimate current false positive rate.
    #[must_use]
    pub fn estimated_fp_rate(&self) -> f64 {
        if self.item_count == 0 {
            return 0.0;
        }
        // FP rate ≈ (1 - e^(-k*n/m))^k
        let k = self.num_hashes as f64;
        let n = self.item_count as f64;
        let m = self.num_bits as f64;
        (1.0 - (-k * n / m).exp()).powf(k)
    }

    /// Get the raw bits (for serialization/debugging).
    #[must_use]
    pub fn bits(&self) -> &[u8] {
        &self.bits
    }

    /// Check if filter is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources to prevent
    /// division-by-zero panics. Validates that num_bits > 0.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.num_bits > 0
    }
}

// =============================================================================
// Request/Response
// =============================================================================

/// Request for Bloom filter-based sync.
///
/// Initiator sends their Bloom filter of known entity IDs.
/// Responder returns entities not in the filter.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct BloomFilterRequest {
    /// Bloom filter containing initiator's entity IDs.
    pub filter: DeltaIdBloomFilter,

    /// False positive rate used to build the filter.
    pub false_positive_rate: f32,
}

impl BloomFilterRequest {
    /// Create a new Bloom filter request.
    #[must_use]
    pub fn new(filter: DeltaIdBloomFilter, false_positive_rate: f32) -> Self {
        Self {
            filter,
            false_positive_rate,
        }
    }

    /// Create a request by building a filter from entity IDs.
    #[must_use]
    pub fn from_ids(ids: &[[u8; 32]], fp_rate: f32) -> Self {
        let mut filter = DeltaIdBloomFilter::new(ids.len(), fp_rate);
        for id in ids {
            filter.insert(id);
        }
        Self::new(filter, fp_rate)
    }
}

/// Response to a Bloom filter sync request.
///
/// Contains entities that the responder has but were not in the filter.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct BloomFilterResponse {
    /// Entities missing from the initiator.
    /// Includes full data and metadata for CRDT merge.
    pub missing_entities: Vec<TreeLeafData>,

    /// Number of entities scanned.
    pub scanned_count: usize,
}

impl BloomFilterResponse {
    /// Create a new response.
    #[must_use]
    pub fn new(missing_entities: Vec<TreeLeafData>, scanned_count: usize) -> Self {
        Self {
            missing_entities,
            scanned_count,
        }
    }

    /// Create an empty response (no missing entities).
    #[must_use]
    pub fn empty(scanned_count: usize) -> Self {
        Self {
            missing_entities: vec![],
            scanned_count,
        }
    }

    /// Check if there are missing entities.
    #[must_use]
    pub fn has_missing(&self) -> bool {
        !self.missing_entities.is_empty()
    }

    /// Get count of missing entities.
    #[must_use]
    pub fn missing_count(&self) -> usize {
        self.missing_entities.len()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::hash_comparison::{CrdtType, LeafMetadata};

    #[test]
    fn test_bloom_filter_fnv1a_consistency() {
        let data = [1u8; 32];
        let hash1 = DeltaIdBloomFilter::hash_fnv1a(&data);
        let hash2 = DeltaIdBloomFilter::hash_fnv1a(&data);
        assert_eq!(hash1, hash2);

        let other_data = [2u8; 32];
        let other_hash = DeltaIdBloomFilter::hash_fnv1a(&other_data);
        assert_ne!(hash1, other_hash);
    }

    #[test]
    fn test_bloom_filter_insert_contains() {
        let mut filter = DeltaIdBloomFilter::new(100, 0.01);

        let id1 = [1u8; 32];
        let id2 = [2u8; 32];
        let id3 = [3u8; 32];

        assert!(!filter.contains(&id1));
        assert!(!filter.contains(&id2));

        filter.insert(&id1);
        filter.insert(&id2);

        assert!(filter.contains(&id1));
        assert!(filter.contains(&id2));
        assert!(!filter.contains(&id3));
    }

    #[test]
    fn test_bloom_filter_item_count() {
        let mut filter = DeltaIdBloomFilter::new(100, 0.01);
        assert_eq!(filter.item_count(), 0);

        filter.insert(&[1u8; 32]);
        assert_eq!(filter.item_count(), 1);

        filter.insert(&[2u8; 32]);
        filter.insert(&[3u8; 32]);
        assert_eq!(filter.item_count(), 3);
    }

    #[test]
    fn test_bloom_filter_roundtrip() {
        let mut filter = DeltaIdBloomFilter::new(50, 0.01);
        filter.insert(&[1u8; 32]);
        filter.insert(&[2u8; 32]);
        filter.insert(&[3u8; 32]);

        let encoded = borsh::to_vec(&filter).expect("serialize");
        let decoded: DeltaIdBloomFilter = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(filter, decoded);
        assert!(decoded.contains(&[1u8; 32]));
        assert!(decoded.contains(&[2u8; 32]));
        assert!(decoded.contains(&[3u8; 32]));
        assert!(!decoded.contains(&[4u8; 32]));
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        let num_items = 1000;
        let target_fp_rate = 0.01;
        let mut filter = DeltaIdBloomFilter::new(num_items, target_fp_rate);

        for i in 0..num_items {
            let mut id = [0u8; 32];
            id[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            filter.insert(&id);
        }

        let test_count = 10000;
        let mut false_positives = 0;
        for i in num_items..(num_items + test_count) {
            let mut id = [0u8; 32];
            id[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            if filter.contains(&id) {
                false_positives += 1;
            }
        }

        let actual_fp_rate = false_positives as f64 / test_count as f64;
        assert!(
            actual_fp_rate < target_fp_rate as f64 * 3.0,
            "FP rate {} too high (target {})",
            actual_fp_rate,
            target_fp_rate
        );
    }

    #[test]
    fn test_bloom_filter_estimated_fp_rate() {
        let mut filter = DeltaIdBloomFilter::new(100, 0.01);

        assert_eq!(filter.estimated_fp_rate(), 0.0);

        for i in 0..50 {
            let mut id = [0u8; 32];
            id[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            filter.insert(&id);
        }

        let estimated = filter.estimated_fp_rate();
        assert!(estimated > 0.0);
        assert!(estimated < 0.1);
    }

    #[test]
    fn test_bloom_filter_request_from_ids() {
        let ids = [[1u8; 32], [2u8; 32], [3u8; 32]];
        let request = BloomFilterRequest::from_ids(&ids, 0.01);

        assert!(request.filter.contains(&[1u8; 32]));
        assert!(request.filter.contains(&[2u8; 32]));
        assert!(request.filter.contains(&[3u8; 32]));
        assert!(!request.filter.contains(&[4u8; 32]));
        assert_eq!(request.false_positive_rate, 0.01);
    }

    #[test]
    fn test_bloom_filter_request_roundtrip() {
        let ids = [[1u8; 32], [2u8; 32]];
        let request = BloomFilterRequest::from_ids(&ids, 0.02);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: BloomFilterRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
    }

    #[test]
    fn test_bloom_filter_response() {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [5; 32]);
        let leaf = TreeLeafData::new([1; 32], vec![1, 2, 3], metadata);

        let response = BloomFilterResponse::new(vec![leaf], 100);

        assert!(response.has_missing());
        assert_eq!(response.missing_count(), 1);
        assert_eq!(response.scanned_count, 100);
    }

    #[test]
    fn test_bloom_filter_response_empty() {
        let response = BloomFilterResponse::empty(50);

        assert!(!response.has_missing());
        assert_eq!(response.missing_count(), 0);
        assert_eq!(response.scanned_count, 50);
    }

    #[test]
    fn test_bloom_filter_response_roundtrip() {
        let metadata = LeafMetadata::new(CrdtType::UnorderedMap, 200, [6; 32]);
        let leaf = TreeLeafData::new([2; 32], vec![4, 5, 6], metadata);

        let response = BloomFilterResponse::new(vec![leaf], 75);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: BloomFilterResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
    }

    #[test]
    fn test_bloom_filter_with_params() {
        let filter = DeltaIdBloomFilter::with_params(1024, 7);

        assert_eq!(filter.bit_count(), 1024);
        assert_eq!(filter.hash_count(), 7);
        assert_eq!(filter.item_count(), 0);
    }

    #[test]
    fn test_bloom_filter_is_valid() {
        // Valid filter created via new()
        let valid_filter = DeltaIdBloomFilter::new(100, 0.01);
        assert!(valid_filter.is_valid());

        // Valid filter created via with_params()
        let valid_params = DeltaIdBloomFilter::with_params(64, 4);
        assert!(valid_params.is_valid());

        // Invalid filter with zero bits (could come from malicious deserialization)
        let invalid_filter = DeltaIdBloomFilter::with_params(0, 4);
        assert!(!invalid_filter.is_valid());
    }
}
