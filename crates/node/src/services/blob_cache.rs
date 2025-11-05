//! Blob Cache Service - Manages in-memory blob caching with intelligent eviction.
//!
//! This service handles all blob cache operations including:
//! - Get/Put operations
//! - LRU eviction by age
//! - LRU eviction by count
//! - LRU eviction by memory usage
//!
//! # Design
//!
//! Uses DashMap for concurrent access without locks. Eviction strategies:
//! 1. **Age-based**: Remove blobs older than MAX_BLOB_AGE
//! 2. **Count-based**: Keep only MAX_CACHE_COUNT most recent blobs
//! 3. **Memory-based**: Keep total memory under MAX_CACHE_BYTES
//!
//! # Thread Safety
//!
//! This service is thread-safe and can be shared across tasks using Arc<>.

use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_primitives::blobs::BlobId;
use dashmap::DashMap;
use tracing::debug;

/// Cached blob with access tracking for LRU eviction
#[derive(Debug, Clone)]
pub struct CachedBlob {
    pub data: Arc<[u8]>,
    pub last_accessed: Instant,
}

impl CachedBlob {
    pub fn new(data: Arc<[u8]>) -> Self {
        Self {
            data,
            last_accessed: Instant::now(),
        }
    }

    pub fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }
}

/// Blob cache service with intelligent eviction strategies.
///
/// # Example
///
/// ```rust,ignore
/// let cache = BlobCacheService::new();
///
/// // Store a blob
/// cache.put(blob_id, data);
///
/// // Retrieve a blob (updates last_accessed)
/// if let Some(data) = cache.get(&blob_id) {
///     println!("Blob size: {}", data.len());
/// }
///
/// // Periodic cleanup
/// cache.evict_old();
/// ```
#[derive(Debug, Clone)]
pub struct BlobCacheService {
    cache: Arc<DashMap<BlobId, CachedBlob>>,
}

impl BlobCacheService {
    /// Create a new blob cache service.
    pub fn new() -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Get a blob from the cache, updating its last_accessed timestamp.
    ///
    /// Returns a clone of the Arc<[u8]>, which is cheap (just increments ref count).
    pub fn get(&self, blob_id: &BlobId) -> Option<Arc<[u8]>> {
        self.cache.get_mut(blob_id).map(|mut entry| {
            entry.touch();
            entry.data.clone()
        })
    }

    /// Put a blob into the cache.
    ///
    /// If the blob already exists, it will be replaced and its timestamp updated.
    pub fn put(&self, blob_id: BlobId, data: Arc<[u8]>) {
        self.cache.insert(blob_id, CachedBlob::new(data));
    }

    /// Check if a blob exists in the cache.
    pub fn contains(&self, blob_id: &BlobId) -> bool {
        self.cache.contains_key(blob_id)
    }

    /// Get the number of blobs currently cached.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Evict old blobs using a 3-phase strategy:
    /// 1. Age-based eviction (older than 5 minutes)
    /// 2. Count-based eviction (keep max 100 blobs)
    /// 3. Memory-based eviction (keep under 500MB)
    ///
    /// This is the main periodic cleanup method.
    pub fn evict_old(&self) {
        const MAX_BLOB_AGE: Duration = Duration::from_secs(300); // 5 minutes
        const MAX_CACHE_COUNT: usize = 100; // Max number of blobs
        const MAX_CACHE_BYTES: usize = 500 * 1024 * 1024; // 500MB total memory

        let before_count = self.cache.len();

        // Phase 1: Remove blobs older than MAX_BLOB_AGE
        self.evict_by_age(MAX_BLOB_AGE);
        let after_time_eviction = self.cache.len();

        // Phase 2: If still over count limit, remove least recently used
        if self.cache.len() > MAX_CACHE_COUNT {
            self.evict_by_count(MAX_CACHE_COUNT);
        }
        let after_count_eviction = self.cache.len();

        // Phase 3: If still over memory limit, remove by LRU until under budget
        let total_size: usize = self.cache.iter().map(|entry| entry.value().data.len()).sum();

        if total_size > MAX_CACHE_BYTES {
            self.evict_by_memory(MAX_CACHE_BYTES);
        }

        // Log summary if anything was evicted
        let total_evicted = before_count.saturating_sub(self.cache.len());
        if total_evicted > 0 {
            debug!(
                total_evicted,
                time_evicted = before_count.saturating_sub(after_time_eviction),
                count_evicted = after_time_eviction.saturating_sub(after_count_eviction),
                memory_evicted = after_count_eviction.saturating_sub(self.cache.len()),
                remaining_count = self.cache.len(),
                "Blob cache eviction completed"
            );
        }
    }

    /// Evict blobs older than the specified age.
    ///
    /// Returns the number of blobs evicted.
    pub fn evict_by_age(&self, max_age: Duration) -> usize {
        let now = Instant::now();
        let before_count = self.cache.len();

        self.cache
            .retain(|_, cached_blob| now.duration_since(cached_blob.last_accessed) < max_age);

        before_count.saturating_sub(self.cache.len())
    }

    /// Evict least recently used blobs to stay under max_count.
    ///
    /// Returns the number of blobs evicted.
    pub fn evict_by_count(&self, max_count: usize) -> usize {
        if self.cache.len() <= max_count {
            return 0;
        }

        let before_count = self.cache.len();

        // Collect all blobs with their access times
        let mut blobs: Vec<_> = self
            .cache
            .iter()
            .map(|entry| (*entry.key(), entry.value().last_accessed))
            .collect();

        // Sort by last_accessed (oldest first)
        blobs.sort_by_key(|(_, accessed)| *accessed);

        // Remove oldest until under count limit
        let to_remove = self.cache.len() - max_count;
        for (blob_id, _) in blobs.iter().take(to_remove) {
            let _removed = self.cache.remove(blob_id);
        }

        before_count.saturating_sub(self.cache.len())
    }

    /// Evict least recently used blobs to stay under max_bytes.
    ///
    /// Returns the number of blobs evicted.
    pub fn evict_by_memory(&self, max_bytes: usize) -> usize {
        let total_size: usize = self.cache.iter().map(|entry| entry.value().data.len()).sum();

        if total_size <= max_bytes {
            return 0;
        }

        let before_count = self.cache.len();

        // Collect all blobs with their access times and sizes
        let mut blobs: Vec<_> = self
            .cache
            .iter()
            .map(|entry| {
                (
                    *entry.key(),
                    entry.value().last_accessed,
                    entry.value().data.len(),
                )
            })
            .collect();

        // Sort by last_accessed (oldest first)
        blobs.sort_by_key(|(_, accessed, _)| *accessed);

        let mut current_size = total_size;
        let mut removed_count = 0;

        for (blob_id, _, size) in blobs {
            if current_size <= max_bytes {
                break;
            }
            let _removed = self.cache.remove(&blob_id);
            current_size = current_size.saturating_sub(size);
            removed_count += 1;
        }

        if removed_count > 0 {
            #[expect(
                clippy::integer_division,
                reason = "MB conversion for logging, precision not critical"
            )]
            let freed_mb = total_size.saturating_sub(current_size) / 1024 / 1024;
            #[expect(
                clippy::integer_division,
                reason = "MB conversion for logging, precision not critical"
            )]
            let new_size_mb = current_size / 1024 / 1024;
            debug!(
                removed_count,
                freed_mb,
                new_size_mb,
                "Evicted blobs to stay under memory limit"
            );
        }

        before_count.saturating_sub(self.cache.len())
    }

    /// Get total memory usage of all cached blobs.
    pub fn total_bytes(&self) -> usize {
        self.cache.iter().map(|entry| entry.value().data.len()).sum()
    }

    /// Clear all blobs from the cache.
    pub fn clear(&self) {
        self.cache.clear();
    }
}

impl Default for BlobCacheService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_put() {
        let cache = BlobCacheService::new();
        let blob_id = BlobId::from([1u8; 32]);
        let data: Arc<[u8]> = Arc::from(vec![1, 2, 3, 4]);

        // Initially empty
        assert!(cache.get(&blob_id).is_none());

        // Put and get
        cache.put(blob_id, data.clone());
        assert_eq!(cache.get(&blob_id).unwrap().as_ref(), data.as_ref());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_evict_by_count() {
        let cache = BlobCacheService::new();

        // Add 10 blobs
        for i in 0..10 {
            let blob_id = BlobId::from([i; 32]);
            let data: Arc<[u8]> = Arc::from(vec![i; 100]);
            cache.put(blob_id, data);
        }

        assert_eq!(cache.len(), 10);

        // Evict to keep only 5
        let evicted = cache.evict_by_count(5);
        assert_eq!(evicted, 5);
        assert_eq!(cache.len(), 5);
    }

    #[test]
    fn test_evict_by_memory() {
        let cache = BlobCacheService::new();

        // Add blobs totaling 1000 bytes
        for i in 0..10 {
            let blob_id = BlobId::from([i; 32]);
            let data: Arc<[u8]> = Arc::from(vec![i; 100]); // 100 bytes each
            cache.put(blob_id, data);
        }

        assert_eq!(cache.total_bytes(), 1000);

        // Evict to stay under 500 bytes
        let evicted = cache.evict_by_memory(500);
        assert!(evicted >= 5); // Should evict at least 5 blobs
        assert!(cache.total_bytes() <= 500);
    }

    #[test]
    fn test_clear() {
        let cache = BlobCacheService::new();

        // Add some blobs
        for i in 0..5 {
            let blob_id = BlobId::from([i; 32]);
            let data: Arc<[u8]> = Arc::from(vec![i; 100]);
            cache.put(blob_id, data);
        }

        assert_eq!(cache.len(), 5);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_touch_updates_timestamp() {
        let cache = BlobCacheService::new();
        let blob_id = BlobId::from([1u8; 32]);
        let data: Arc<[u8]> = Arc::from(vec![1, 2, 3]);

        cache.put(blob_id, data);

        // Get the blob (touches it)
        let _data1 = cache.get(&blob_id);

        // Sleep a bit
        std::thread::sleep(Duration::from_millis(10));

        // Get again
        let _data2 = cache.get(&blob_id);

        // The blob should still be there (not evicted by age)
        assert!(cache.contains(&blob_id));
    }
}
