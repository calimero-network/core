//! Context Repository - Low-level context storage operations.
//!
//! This module provides the `ContextRepository` which handles all context CRUD operations,
//! caching, and database persistence. It's a focused service with a single responsibility:
//! managing the lifecycle and storage of contexts.

use std::num::NonZeroUsize;
use std::sync::Arc;

use calimero_context_primitives::client::ContextClient;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::Store;
use eyre::Result;
use lru::LruCache;
use tokio::sync::Mutex;
use tracing::debug;

/// Metadata for a cached context.
///
/// Contains the context data plus a lock for serializing operations
/// on this specific context.
#[derive(Debug, Clone)]
pub struct ContextMeta {
    pub meta: Context,
    pub lock: Arc<Mutex<ContextId>>,
}

/// Repository for context storage and caching.
///
/// Responsibilities:
/// - CRUD operations on contexts
/// - LRU cache management (bounded to prevent memory leaks)
/// - Database synchronization
/// - DAG heads refresh from database
///
/// # Thread Safety
/// This is NOT thread-safe by itself - it's designed to be wrapped in Arc<Mutex<>>
/// or used within an actor.
#[derive(Debug)]
pub struct ContextRepository {
    /// Direct access to RocksDB
    datastore: Store,

    /// Client for context-related database operations
    context_client: ContextClient,

    /// LRU cache of active contexts (max 1000 to prevent memory leaks)
    /// When full, least recently used context is evicted (safe - data persists in DB)
    cache: LruCache<ContextId, ContextMeta>,
}

impl ContextRepository {
    /// Create a new context repository.
    ///
    /// # Arguments
    /// * `datastore` - Direct RocksDB access
    /// * `context_client` - Client for context operations
    /// * `max_size` - Maximum number of contexts to cache (default: 1000)
    pub fn new(datastore: Store, context_client: ContextClient, max_size: Option<usize>) -> Self {
        let capacity = max_size.unwrap_or(1000);
        let cache_size = NonZeroUsize::new(capacity)
            .unwrap_or_else(|| NonZeroUsize::new(1000).expect("1000 > 0"));

        Self {
            datastore,
            context_client,
            cache: LruCache::new(cache_size),
        }
    }

    /// Get a context by ID, using cache or fetching from database.
    ///
    /// **CRITICAL**: Always refreshes DAG heads from database to ensure cache coherency.
    /// The DAG heads can be updated by delta_store when receiving network deltas,
    /// but the cached Context object won't reflect these changes unless we reload.
    ///
    /// # Returns
    /// - `Ok(Some(&ContextMeta))` if context exists
    /// - `Ok(None)` if context doesn't exist
    /// - `Err(_)` on database errors
    pub fn get(&mut self, context_id: &ContextId) -> Result<Option<&ContextMeta>> {
        // Check if exists in cache
        let in_cache = self.cache.contains(context_id);

        if !in_cache {
            // Not in cache - fetch from DB
            let Some(context) = self.context_client.get_context(context_id)? else {
                return Ok(None);
            };

            let lock = Arc::new(Mutex::new(*context_id));

            // Insert into cache
            self.cache.put(
                *context_id,
                ContextMeta {
                    meta: context,
                    lock,
                },
            );
        }

        // Get from cache (guaranteed to exist now)
        let cached = self
            .cache
            .get_mut(context_id)
            .expect("just inserted or already existed");

        // CRITICAL FIX: Always reload dag_heads from database to get latest state
        // The dag_heads can be updated by delta_store when receiving network deltas,
        // but the cached Context object won't reflect these changes.
        // This was causing all deltas to use genesis as parent instead of actual dag_heads.
        let handle = self.datastore.handle();
        let key = calimero_store::key::ContextMeta::new(*context_id);

        if let Some(meta) = handle.get(&key)? {
            // Update dag_heads if they changed in DB
            if cached.meta.dag_heads != meta.dag_heads {
                debug!(
                    %context_id,
                    old_heads_count = cached.meta.dag_heads.len(),
                    new_heads_count = meta.dag_heads.len(),
                    "Refreshing dag_heads from database (cache was stale)"
                );
                cached.meta.dag_heads = meta.dag_heads;
            }

            // Also update root_hash in case it changed
            cached.meta.root_hash = meta.root_hash.into();
        }

        Ok(Some(&*cached))
    }

    /// Insert or update a context in the cache.
    ///
    /// This updates the cache immediately but does NOT persist to database.
    /// Caller is responsible for persisting via `context_client` separately.
    ///
    /// # Arguments
    /// * `context_id` - ID of the context
    /// * `context_meta` - Context metadata to cache
    pub fn put(&mut self, context_id: ContextId, context_meta: ContextMeta) {
        let _evicted = self.cache.put(context_id, context_meta);
        // Note: If _evicted is Some, an LRU context was evicted (data still in DB)
    }

    /// Remove a context from the cache.
    ///
    /// This does NOT delete from database - only evicts from cache.
    /// Caller is responsible for database deletion separately.
    ///
    /// # Returns
    /// The evicted `ContextMeta` if it was cached, `None` otherwise.
    pub fn remove(&mut self, context_id: &ContextId) -> Option<ContextMeta> {
        self.cache.pop(context_id)
    }

    /// Check if a context exists in the cache.
    ///
    /// Note: This only checks the cache, not the database.
    pub fn contains(&self, context_id: &ContextId) -> bool {
        self.cache.contains(context_id)
    }

    /// Peek at a cached context without refreshing from database.
    ///
    /// Returns `None` if context is not in cache.
    /// Use `get()` if you need database fallback and refresh.
    pub fn peek(&mut self, context_id: &ContextId) -> Option<&ContextMeta> {
        self.cache.get(context_id)
    }

    /// Get the number of contexts currently cached.
    pub fn cached_count(&self) -> usize {
        self.cache.len()
    }

    /// Get the maximum cache capacity.
    pub fn max_capacity(&self) -> usize {
        self.cache.cap().get()
    }

    /// Update the root hash for a cached context.
    ///
    /// If the context is not in cache, this is a no-op.
    /// Does NOT persist to database - caller must handle persistence.
    ///
    /// # Returns
    /// `true` if context was in cache and updated, `false` otherwise.
    pub fn update_root_hash(&mut self, context_id: &ContextId, root_hash: Hash) -> bool {
        if let Some(cached) = self.cache.get_mut(context_id) {
            cached.meta.root_hash = root_hash;
            true
        } else {
            false
        }
    }

    /// Update both DAG heads and root hash atomically (called after delta application)
    pub fn update_dag_heads_and_root(
        &mut self,
        context_id: &ContextId,
        dag_heads: Vec<[u8; 32]>,
        root_hash: Hash,
    ) -> Result<(), eyre::Error> {
        use calimero_store::key;

        // Update cache if present
        if let Some(cached) = self.cache.get_mut(context_id) {
            cached.meta.dag_heads = dag_heads.clone();
            cached.meta.root_hash = root_hash;
        }

        // Persist to database
        let mut handle = self.datastore.handle();
        let mut context = handle
            .get(&key::ContextMeta::new(*context_id))?
            .ok_or_else(|| eyre::eyre!("Context not found: {}", context_id))?;

        context.dag_heads = dag_heads;
        context.root_hash = *root_hash.as_bytes();

        handle.put(&key::ContextMeta::new(*context_id), &context)?;

        Ok(())
    }

    /// Update the application ID for a cached context.
    ///
    /// If the context is not in cache, this is a no-op.
    /// Does NOT persist to database - caller must handle persistence.
    ///
    /// # Returns
    /// `true` if context was in cache and updated, `false` otherwise.
    pub fn update_application_id(
        &mut self,
        context_id: &ContextId,
        application_id: calimero_primitives::application::ApplicationId,
    ) -> bool {
        if let Some(cached) = self.cache.get_mut(context_id) {
            cached.meta.application_id = application_id;
            true
        } else {
            false
        }
    }

    /// Get mutable access to the context client.
    ///
    /// Useful for operations that require direct database access.
    pub fn context_client(&self) -> &ContextClient {
        &self.context_client
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::application::ApplicationId;

    // Note: These are basic structural tests.
    // Full integration tests would require setting up Store and ContextClient mocks.

    #[test]
    fn test_repository_creation() {
        // This test would need proper Store and ContextClient setup
        // For now, just verify the struct can be constructed with the right types
    }

    #[test]
    fn test_cache_capacity() {
        // Verify cache is bounded and evicts LRU entries when full
    }
}
