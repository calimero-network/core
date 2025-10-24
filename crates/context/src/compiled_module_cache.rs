use std::num::NonZeroUsize;

use calimero_primitives::blobs::BlobId;
use lru::LruCache;
use parking_lot::Mutex;
use tracing::{debug, trace};

/// A thread-safe LRU cache for compiled WASM module bytes to avoid RocksDB reads.
///
/// This cache stores compiled module bytes by their blob ID,
/// avoiding the need to fetch from RocksDB on every execution.
/// Deserialization is still required but is much faster than disk I/O.
///
/// # Cache Key
///
/// The cache key is the blob ID of the compiled module in blob storage.
///
/// # Cache Value
///
/// The cached value is the serialized compiled module bytes.
///
/// # Performance
///
/// - Cache hit: ~0.01ms (memory lookup) + ~0.1-1ms (deserialize)
/// - Cache miss → RocksDB: ~1-5ms (disk I/O) + ~0.1-1ms (deserialize)
/// - Cache miss → Compile: ~10-100ms (full compilation)
#[derive(Debug)]
pub struct CompiledModuleCache {
    cache: Mutex<LruCache<BlobId, Box<[u8]>>>,
}

impl CompiledModuleCache {
    /// Creates a new compiled module cache with the specified capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of compiled modules to cache.
    ///                Typical values: 10-100 depending on available memory.
    ///                Each cached module can be several MB.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = NonZeroUsize::new(capacity).expect("cache capacity must be non-zero");

        Self {
            cache: Mutex::new(LruCache::new(capacity)),
        }
    }

    /// Attempts to retrieve cached compiled module bytes.
    ///
    /// # Arguments
    ///
    /// * `blob_id` - The blob ID of the compiled module.
    ///
    /// # Returns
    ///
    /// * `Some(bytes)` if the module bytes are in cache (cache hit)
    /// * `None` if the module bytes are not in cache (cache miss)
    pub fn get(&self, blob_id: &BlobId) -> Option<Box<[u8]>> {
        let result = self.cache.lock().get(blob_id).cloned();

        match &result {
            Some(bytes) => {
                debug!(
                    blob_id = %blob_id,
                    size_bytes = bytes.len(),
                    "Compiled module cache hit"
                );
            }
            None => {
                trace!(
                    blob_id = %blob_id,
                    "Compiled module cache miss"
                );
            }
        }

        result
    }

    /// Stores compiled module bytes in the cache.
    ///
    /// If the cache is full, the least recently used entry will be evicted.
    ///
    /// # Arguments
    ///
    /// * `blob_id` - The blob ID of the compiled module.
    /// * `bytes` - The compiled module bytes to cache.
    pub fn put(&self, blob_id: BlobId, bytes: Box<[u8]>) {
        let size = bytes.len();
        let evicted = self.cache.lock().push(blob_id, bytes);

        debug!(
            blob_id = %blob_id,
            size_bytes = size,
            evicted = evicted.is_some(),
            "Cached compiled module bytes"
        );
    }

    /// Returns the current number of cached modules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    /// Returns true if the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cache.lock().is_empty()
    }

    /// Clears all cached modules.
    pub fn clear(&self) {
        let len = self.len();
        self.cache.lock().clear();
        debug!(cleared_count = len, "Cleared compiled module cache");
    }
}

impl Default for CompiledModuleCache {
    /// Creates a cache with a default capacity of 32 modules.
    ///
    /// This default assumes:
    /// - Average usage: moderate number of hot contracts
    /// - Cache memory usage: varies by module size
    fn default() -> Self {
        Self::new(32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests use mock modules since creating real compiled modules
    // requires actual WASM compilation which is expensive for tests.
}
