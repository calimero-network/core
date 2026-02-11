//! BloomFilter sync types (CIP Appendix B - Protocol Selection Matrix).
//!
//! Types for Bloom filter-based synchronization for large trees with small divergence.

use borsh::{BorshDeserialize, BorshSerialize};

use super::hash_comparison::TreeLeafData;

// =============================================================================
// Constants
// =============================================================================

/// Default false positive rate for Bloom filters.
/// Uses f64 for consistency with wire protocol (SyncProtocol::BloomFilter).
pub const DEFAULT_BLOOM_FP_RATE: f64 = 0.01; // 1%

/// Minimum bits per element for reasonable FP rate.
const MIN_BITS_PER_ELEMENT: usize = 8;

/// Minimum allowed false positive rate (prevents ln(0) = -inf).
const MIN_FP_RATE: f64 = 0.0001;

/// Maximum allowed false positive rate (above this a bloom filter is pointless).
const MAX_FP_RATE: f64 = 0.5;

/// Minimum number of bits (prevents division by zero and ensures some utility).
const MIN_NUM_BITS: usize = 64;

/// Minimum number of hash functions (0 hashes = always returns true).
const MIN_NUM_HASHES: u8 = 1;

/// Maximum number of hash functions (diminishing returns beyond this).
const MAX_NUM_HASHES: u8 = 16;

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
    /// * `fp_rate` - Desired false positive rate (clamped to 0.0001..0.5)
    ///
    /// # Notes
    /// The `fp_rate` is clamped to avoid mathematical errors:
    /// - Values <= 0 would cause `ln()` to produce `-inf` or `NaN`
    /// - Values >= 0.5 make the bloom filter nearly useless
    #[must_use]
    pub fn new(expected_items: usize, fp_rate: f64) -> Self {
        // Handle NaN explicitly, then clamp to valid range
        // NaN.clamp() returns NaN, which would propagate through calculations
        let fp_rate = if fp_rate.is_nan() {
            DEFAULT_BLOOM_FP_RATE
        } else {
            fp_rate.clamp(MIN_FP_RATE, MAX_FP_RATE)
        };

        // Calculate optimal number of bits: m = -n * ln(p) / (ln(2)^2)
        let ln2_sq = std::f64::consts::LN_2 * std::f64::consts::LN_2;
        let num_bits = if expected_items == 0 {
            // Use minimum size with reasonable defaults for empty filter
            // 64 bits with 4 hashes gives ~6% FP rate for 1 item
            MIN_NUM_BITS
        } else {
            let m = -(expected_items as f64) * fp_rate.ln() / ln2_sq;
            // Use saturating_mul to prevent overflow for huge expected_items
            (m.ceil() as usize).max(expected_items.saturating_mul(MIN_BITS_PER_ELEMENT))
        };

        // Calculate optimal number of hashes: k = (m/n) * ln(2)
        let num_hashes = if expected_items == 0 {
            4 // Reasonable default for empty/small filters
        } else {
            let k = (num_bits as f64 / expected_items as f64) * std::f64::consts::LN_2;
            (k.ceil() as u8).clamp(MIN_NUM_HASHES, MAX_NUM_HASHES)
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
    ///
    /// # Arguments
    /// * `num_bits` - Number of bits (clamped to minimum 64 to prevent division by zero)
    /// * `num_hashes` - Number of hash functions (clamped to 1..16)
    #[must_use]
    pub fn with_params(num_bits: usize, num_hashes: u8) -> Self {
        // Clamp to prevent division by zero and ensure filter has some utility
        let num_bits = num_bits.max(MIN_NUM_BITS);
        let num_hashes = num_hashes.clamp(MIN_NUM_HASHES, MAX_NUM_HASHES);

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

    /// Compute the two base hashes for double hashing.
    ///
    /// Returns (h1, h2) where positions are computed as: h1 + i * h2 (mod num_bits)
    /// Uses stack-allocated buffer for h2 to avoid heap allocation.
    #[inline]
    fn compute_hashes(id: &[u8; 32]) -> (u64, u64) {
        let h1 = Self::hash_fnv1a(id);

        // Use stack-allocated buffer for h2 computation
        let mut buf = [0u8; 33];
        buf[..32].copy_from_slice(id);
        buf[32] = 0xFF;
        let h2 = Self::hash_fnv1a(&buf);

        (h1, h2)
    }

    /// Compute a single hash position using double hashing.
    #[inline]
    fn position_at(&self, h1: u64, h2: u64, i: u64) -> usize {
        let combined = h1.wrapping_add(i.wrapping_mul(h2));
        (combined as usize) % self.num_bits
    }

    /// Insert an ID into the filter.
    ///
    /// Computes hash positions inline to avoid Vec allocation.
    /// Silently returns if filter is invalid (use `is_valid()` to check).
    pub fn insert(&mut self, id: &[u8; 32]) {
        // Guard against invalid filter state (including malicious deserialization)
        if !self.is_valid() {
            return;
        }

        let (h1, h2) = Self::compute_hashes(id);

        for i in 0..self.num_hashes as u64 {
            let pos = self.position_at(h1, h2, i);
            let byte_idx = pos / 8;
            let bit_idx = pos % 8;
            // Defense-in-depth: bounds check (should never fail if is_valid() passed)
            if byte_idx < self.bits.len() {
                self.bits[byte_idx] |= 1 << bit_idx;
            }
        }
        self.item_count += 1;
    }

    /// Check if an ID might be in the filter.
    ///
    /// Returns `true` if the ID is possibly in the set (may be false positive).
    /// Returns `false` if the ID is definitely not in the set.
    ///
    /// Computes hash positions inline to avoid Vec allocation.
    /// Returns `false` if filter is invalid (use `is_valid()` to check).
    #[must_use]
    pub fn contains(&self, id: &[u8; 32]) -> bool {
        // Guard against invalid filter state (including malicious deserialization)
        if !self.is_valid() {
            return false;
        }

        let (h1, h2) = Self::compute_hashes(id);

        for i in 0..self.num_hashes as u64 {
            let pos = self.position_at(h1, h2, i);
            let byte_idx = pos / 8;
            let bit_idx = pos % 8;
            // Defense-in-depth: bounds check (should never fail if is_valid() passed)
            if byte_idx >= self.bits.len() {
                return false;
            }
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

    /// Check if the filter is valid.
    ///
    /// Call this after deserializing from untrusted sources.
    /// Constructors (`new()`, `with_params()`) always create valid filters,
    /// but deserialization can produce invalid state.
    ///
    /// Returns `false` if:
    /// - `num_bits` is 0 (would cause division by zero)
    /// - `num_hashes` is 0 (filter would always return true)
    /// - `bits.len() * 8 < num_bits` (would cause out-of-bounds access)
    #[must_use]
    pub fn is_valid(&self) -> bool {
        if self.num_bits == 0 || self.num_hashes == 0 {
            return false;
        }
        // Check structural consistency: bits array must be large enough
        // to hold num_bits bits. Required bytes = (num_bits + 7) / 8
        let required_bytes = (self.num_bits + 7) / 8;
        self.bits.len() >= required_bytes
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
    /// Uses f64 for consistency with wire protocol.
    pub false_positive_rate: f64,
}

impl BloomFilterRequest {
    /// Create a new Bloom filter request.
    #[must_use]
    pub fn new(filter: DeltaIdBloomFilter, false_positive_rate: f64) -> Self {
        Self {
            filter,
            false_positive_rate,
        }
    }

    /// Create a request by building a filter from entity IDs.
    #[must_use]
    pub fn from_ids(ids: &[[u8; 32]], fp_rate: f64) -> Self {
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
    fn test_bloom_filter_fp_rate_clamping_zero() {
        // fp_rate = 0 would cause ln(0) = -inf, should be clamped to MIN_FP_RATE
        let filter = DeltaIdBloomFilter::new(100, 0.0);

        // Filter should be created successfully without panic
        assert!(filter.bit_count() > 0);
        assert!(filter.hash_count() > 0);

        // Should still work correctly
        let id = [42u8; 32];
        assert!(!filter.contains(&id));
    }

    #[test]
    fn test_bloom_filter_fp_rate_clamping_negative() {
        // Negative fp_rate would cause ln(negative) = NaN, should be clamped
        let filter = DeltaIdBloomFilter::new(100, -0.5);

        // Filter should be created successfully without panic
        assert!(filter.bit_count() > 0);
        assert!(filter.hash_count() > 0);
    }

    #[test]
    fn test_bloom_filter_fp_rate_clamping_too_high() {
        // fp_rate > 0.5 makes bloom filter nearly useless, should be clamped
        let filter = DeltaIdBloomFilter::new(100, 0.99);

        // Filter should be created with reasonable parameters
        assert!(filter.bit_count() > 0);
        assert!(filter.hash_count() > 0);

        // With clamped fp_rate of 0.5, filter should still have some utility
        let mut filter = filter;
        let id = [1u8; 32];
        filter.insert(&id);
        assert!(filter.contains(&id));
    }

    #[test]
    fn test_bloom_filter_fp_rate_edge_cases() {
        // Test various edge case fp_rates
        let test_cases = [
            (f64::NEG_INFINITY, "negative infinity"),
            (f64::INFINITY, "positive infinity"),
            (f64::NAN, "NaN"),
            (-1.0_f64, "negative one"),
            (0.0_f64, "zero"),
            (1.0_f64, "one"),
            (2.0_f64, "greater than one"),
        ];

        for (fp_rate, description) in test_cases {
            let filter = DeltaIdBloomFilter::new(100, fp_rate);
            assert!(
                filter.bit_count() > 0,
                "Filter with fp_rate {} ({}) should have positive bit count",
                fp_rate,
                description
            );
        }
    }

    #[test]
    fn test_bloom_filter_nan_uses_default() {
        // NaN fp_rate should use DEFAULT_BLOOM_FP_RATE
        let nan_filter = DeltaIdBloomFilter::new(100, f64::NAN);
        let default_filter = DeltaIdBloomFilter::new(100, DEFAULT_BLOOM_FP_RATE);

        // Both should produce the same sized filter
        assert_eq!(nan_filter.bit_count(), default_filter.bit_count());
        assert_eq!(nan_filter.hash_count(), default_filter.hash_count());
    }

    #[test]
    fn test_bloom_filter_hashing_deterministic() {
        // Verify that hash computation is deterministic
        let id = [0xAB; 32];

        let (h1_a, h2_a) = DeltaIdBloomFilter::compute_hashes(&id);
        let (h1_b, h2_b) = DeltaIdBloomFilter::compute_hashes(&id);

        assert_eq!(h1_a, h1_b);
        assert_eq!(h2_a, h2_b);

        // Different IDs should produce different hashes
        let other_id = [0xCD; 32];
        let (h1_other, h2_other) = DeltaIdBloomFilter::compute_hashes(&other_id);
        assert_ne!(h1_a, h1_other);
    }

    #[test]
    fn test_bloom_filter_insert_contains_deterministic() {
        // Verify that insert/contains are deterministic across filter instances
        let mut filter1 = DeltaIdBloomFilter::new(100, 0.01);
        let mut filter2 = DeltaIdBloomFilter::new(100, 0.01);

        let id = [0xAB; 32];
        filter1.insert(&id);
        filter2.insert(&id);

        // Both filters should have the same bits set
        assert_eq!(filter1.bits(), filter2.bits());
        assert!(filter1.contains(&id));
        assert!(filter2.contains(&id));
    }

    #[test]
    fn test_bloom_filter_with_params_clamps_num_bits() {
        // num_bits=0 should be clamped to MIN_NUM_BITS (64)
        let filter = DeltaIdBloomFilter::with_params(0, 4);
        assert_eq!(filter.bit_count(), 64);
        assert!(filter.is_valid());

        // Should work correctly after clamping
        let id = [1u8; 32];
        assert!(!filter.contains(&id));
    }

    #[test]
    fn test_bloom_filter_with_params_clamps_num_hashes() {
        // num_hashes=0 should be clamped to 1
        let filter = DeltaIdBloomFilter::with_params(128, 0);
        assert_eq!(filter.hash_count(), 1);
        assert!(filter.is_valid());

        // num_hashes > 16 should be clamped to 16
        let filter_high = DeltaIdBloomFilter::with_params(128, 255);
        assert_eq!(filter_high.hash_count(), 16);
    }

    #[test]
    fn test_bloom_filter_is_valid() {
        // Valid filter created via new()
        let valid_new = DeltaIdBloomFilter::new(100, 0.01);
        assert!(valid_new.is_valid());

        // Valid filter created via with_params() (clamped)
        let valid_params = DeltaIdBloomFilter::with_params(0, 0);
        assert!(valid_params.is_valid()); // Clamped to valid values
    }

    #[test]
    fn test_bloom_filter_malicious_deserialization_num_bits_zero() {
        // Simulate deserializing a malicious bloom filter with num_bits=0
        // This could come from an attacker trying to cause division by zero

        // Manually construct serialized bytes with num_bits=0
        let mut bytes = Vec::new();
        // bits: Vec<u8> - length (4 bytes little-endian) + data
        bytes.extend_from_slice(&0u32.to_le_bytes()); // empty vec
                                                      // num_bits: usize (8 bytes on 64-bit)
        bytes.extend_from_slice(&0usize.to_le_bytes()); // MALICIOUS: 0
                                                        // num_hashes: u8
        bytes.push(4);
        // item_count: usize
        bytes.extend_from_slice(&0usize.to_le_bytes());

        let filter: DeltaIdBloomFilter = borsh::from_slice(&bytes).expect("deserialize");

        // is_valid() should detect the invalid state
        assert!(
            !filter.is_valid(),
            "Filter with num_bits=0 should be invalid"
        );

        // Operations should not panic, but return safe defaults
        let id = [1u8; 32];
        // contains() should return false (no positions to check)
        assert!(!filter.contains(&id));
    }

    #[test]
    fn test_bloom_filter_malicious_deserialization_num_hashes_zero() {
        // Simulate deserializing a filter with num_hashes=0

        let mut bytes = Vec::new();
        // bits: Vec<u8>
        bytes.extend_from_slice(&8u32.to_le_bytes()); // 8 bytes
        bytes.extend_from_slice(&[0u8; 8]); // 64 bits
                                            // num_bits: usize
        bytes.extend_from_slice(&64usize.to_le_bytes());
        // num_hashes: u8
        bytes.push(0); // MALICIOUS: 0
                       // item_count: usize
        bytes.extend_from_slice(&0usize.to_le_bytes());

        let filter: DeltaIdBloomFilter = borsh::from_slice(&bytes).expect("deserialize");

        // is_valid() should detect the invalid state
        assert!(
            !filter.is_valid(),
            "Filter with num_hashes=0 should be invalid"
        );
    }

    #[test]
    fn test_bloom_filter_malicious_deserialization_bits_too_small() {
        // HIGH SEVERITY: Simulate attack where num_bits exceeds bits.len() * 8
        // This would cause out-of-bounds panic without proper validation

        let mut bytes = Vec::new();
        // bits: Vec<u8> - only 1 byte (8 bits of actual storage)
        bytes.extend_from_slice(&1u32.to_le_bytes()); // length = 1
        bytes.push(0u8); // only 1 byte = 8 bits
                         // num_bits: usize - MALICIOUS: claims 1 million bits!
        bytes.extend_from_slice(&1_000_000usize.to_le_bytes());
        // num_hashes: u8
        bytes.push(4);
        // item_count: usize
        bytes.extend_from_slice(&0usize.to_le_bytes());

        let filter: DeltaIdBloomFilter = borsh::from_slice(&bytes).expect("deserialize");

        // is_valid() MUST detect this attack
        assert!(
            !filter.is_valid(),
            "Filter with bits.len()=1 but num_bits=1000000 should be invalid"
        );

        // Operations MUST NOT panic, even on invalid filter
        let id = [0xAB; 32];
        // contains() should safely return false without panicking
        assert!(!filter.contains(&id));

        // insert() should safely do nothing without panicking
        let mut filter_mut = filter;
        filter_mut.insert(&id); // Should not panic!
    }

    #[test]
    fn test_bloom_filter_structural_consistency() {
        // Test that valid filters pass structural consistency check
        let filter = DeltaIdBloomFilter::new(100, 0.01);
        assert!(filter.is_valid());

        // bits.len() should be >= (num_bits + 7) / 8
        let required_bytes = (filter.bit_count() + 7) / 8;
        assert!(filter.bits().len() >= required_bytes);
    }
}
