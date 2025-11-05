//! Delta Store Service - Manages DeltaStore lifecycle and cleanup.
//!
//! This service handles all delta store operations including:
//! - Creating and retrieving delta stores for contexts
//! - Periodic cleanup of stale pending deltas
//! - Statistics collection for monitoring
//!
//! # Design
//!
//! Uses DashMap for concurrent access. Each context has its own DeltaStore.
//! The service provides centralized management of all delta stores.
//!
//! # Thread Safety
//!
//! This service is thread-safe and can be shared across tasks using Arc<>.

use std::sync::Arc;
use std::time::Duration;

use calimero_dag::PendingStats;
use calimero_primitives::context::ContextId;
use dashmap::DashMap;
use tracing::{debug, warn};

use crate::delta_store::DeltaStore;

/// Delta store service with centralized lifecycle management.
///
/// # Example
///
/// ```rust,ignore
/// let service = DeltaStoreService::new();
///
/// // Get or create a delta store for a context
/// let delta_store = service.get_or_create(&context_id);
///
/// // Periodic cleanup
/// let evicted = service.cleanup_all_stale(Duration::from_secs(300)).await;
/// ```
#[derive(Debug, Clone)]
pub struct DeltaStoreService {
    stores: Arc<DashMap<ContextId, DeltaStore>>,
}

impl DeltaStoreService {
    /// Create a new delta store service.
    pub fn new() -> Self {
        Self {
            stores: Arc::new(DashMap::new()),
        }
    }

    /// Get an existing delta store for a context, returning None if it doesn't exist.
    ///
    /// Note: DeltaStore instances need specific initialization parameters (context_client, our_identity)
    /// that vary by caller. Use this to check existence, and handle creation explicitly where needed.
    ///
    /// For async handlers that need get-or-create semantics, see the pattern in state_delta.rs.
    pub fn get_or_create_with<F>(&self, context_id: &ContextId, create_fn: F) -> (dashmap::mapref::one::Ref<'_, ContextId, DeltaStore>, bool)
    where
        F: FnOnce() -> DeltaStore,
    {
        let mut is_new = false;
        
        // Try to get existing first
        if let Some(store) = self.stores.get(context_id) {
            return (store, false);
        }
        
        // Need to create - use entry API to ensure atomic insert
        self.stores.entry(*context_id).or_insert_with(|| {
            is_new = true;
            create_fn()
        });
        
        (self.stores.get(context_id).expect("just inserted"), is_new)
    }

    /// Get an existing delta store for a context without creating one.
    ///
    /// Returns None if no store exists for this context.
    pub fn get(&self, context_id: &ContextId) -> Option<dashmap::mapref::one::Ref<'_, ContextId, DeltaStore>> {
        self.stores.get(context_id)
    }

    /// Check if a delta store exists for a context.
    pub fn contains(&self, context_id: &ContextId) -> bool {
        self.stores.contains_key(context_id)
    }

    /// Get the number of delta stores currently managed.
    pub fn len(&self) -> usize {
        self.stores.len()
    }

    /// Check if the service is managing any delta stores.
    pub fn is_empty(&self) -> bool {
        self.stores.is_empty()
    }

    /// Cleanup stale pending deltas across ALL contexts.
    ///
    /// This iterates over all delta stores and removes deltas older than max_age.
    /// Also logs statistics for monitoring.
    ///
    /// # Arguments
    /// * `max_age` - Maximum age for pending deltas before eviction
    ///
    /// # Returns
    /// Total number of deltas evicted across all contexts
    pub async fn cleanup_all_stale(&self, max_age: Duration) -> usize {
        const SNAPSHOT_THRESHOLD: usize = 100;
        let mut total_evicted = 0;

        for entry in self.stores.iter() {
            let context_id = *entry.key();
            let delta_store = entry.value();

            // Evict stale deltas
            let evicted = delta_store.cleanup_stale(max_age).await;
            total_evicted += evicted;

            if evicted > 0 {
                warn!(
                    %context_id,
                    evicted_count = evicted,
                    "Evicted stale pending deltas (timed out after {:?})",
                    max_age
                );
            }

            // Log stats for monitoring
            let stats = delta_store.pending_stats().await;
            if stats.count > 0 {
                debug!(
                    %context_id,
                    pending_count = stats.count,
                    oldest_age_secs = stats.oldest_age_secs,
                    missing_parents = stats.total_missing_parents,
                    "Pending delta statistics"
                );

                // Trigger snapshot fallback if too many pending
                if stats.count > SNAPSHOT_THRESHOLD {
                    warn!(
                        %context_id,
                        pending_count = stats.count,
                        threshold = SNAPSHOT_THRESHOLD,
                        "Too many pending deltas - state sync will recover on next periodic sync"
                    );
                }
            }
        }

        total_evicted
    }

    /// Cleanup stale pending deltas for a specific context.
    ///
    /// # Arguments
    /// * `context_id` - The context to cleanup
    /// * `max_age` - Maximum age for pending deltas before eviction
    ///
    /// # Returns
    /// Number of deltas evicted, or 0 if context doesn't exist
    pub async fn cleanup_stale(&self, context_id: &ContextId, max_age: Duration) -> usize {
        if let Some(delta_store) = self.get(context_id) {
            delta_store.cleanup_stale(max_age).await
        } else {
            0
        }
    }

    /// Get pending delta statistics for a specific context.
    ///
    /// # Returns
    /// Statistics about pending deltas, or default stats if context doesn't exist
    pub async fn pending_stats(&self, context_id: &ContextId) -> PendingStats {
        if let Some(delta_store) = self.get(context_id) {
            delta_store.pending_stats().await
        } else {
            PendingStats::default()
        }
    }

    /// Get pending delta statistics for all contexts.
    ///
    /// # Returns
    /// Vec of (ContextId, PendingStats) tuples
    pub async fn all_pending_stats(&self) -> Vec<(ContextId, PendingStats)> {
        let mut results = Vec::new();

        for entry in self.stores.iter() {
            let context_id = *entry.key();
            let delta_store = entry.value();
            let stats = delta_store.pending_stats().await;

            if stats.count > 0 {
                results.push((context_id, stats));
            }
        }

        results
    }

    /// Remove a delta store for a context.
    ///
    /// This is useful when a context is deleted or no longer needed.
    ///
    /// # Returns
    /// The removed DeltaStore if it existed, None otherwise
    pub fn remove(&self, context_id: &ContextId) -> Option<(ContextId, DeltaStore)> {
        self.stores.remove(context_id)
    }

    /// Clear all delta stores.
    ///
    /// This is primarily useful for testing or shutdown scenarios.
    pub fn clear(&self) {
        self.stores.clear();
    }
}

impl Default for DeltaStoreService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Full test of get_or_create_with requires ContextClient and identity setup.
    // Integration tests in state_delta.rs and sync/manager.rs verify real usage.
    // Here we test the service's storage/retrieval mechanics.

    #[tokio::test]
    async fn test_get_nonexistent() {
        let service = DeltaStoreService::new();
        let context_id = ContextId::from([1u8; 32]);

        // Get non-existent store should return None
        assert!(service.get(&context_id).is_none());
        assert!(!service.contains(&context_id));
    }

    #[tokio::test]
    async fn test_len_and_empty() {
        let service = DeltaStoreService::new();

        // Initially empty
        assert_eq!(service.len(), 0);
        assert!(service.is_empty());
    }

    #[tokio::test]
    async fn test_pending_stats_nonexistent() {
        let service = DeltaStoreService::new();
        let context_id = ContextId::from([1u8; 32]);

        // Stats for non-existent context should return default
        let stats = service.pending_stats(&context_id).await;
        assert_eq!(stats.count, 0);
    }

    #[tokio::test]
    async fn test_cleanup_stale_nonexistent() {
        let service = DeltaStoreService::new();
        let context_id = ContextId::from([1u8; 32]);

        // Cleanup for non-existent context should return 0
        let evicted = service.cleanup_stale(&context_id, Duration::from_secs(300)).await;
        assert_eq!(evicted, 0);
    }

    #[tokio::test]
    async fn test_cleanup_all_stale_empty() {
        let service = DeltaStoreService::new();

        // Cleanup on empty service should return 0
        let evicted = service.cleanup_all_stale(Duration::from_secs(300)).await;
        assert_eq!(evicted, 0);
    }

    #[tokio::test]
    async fn test_all_pending_stats_empty() {
        let service = DeltaStoreService::new();

        // Get all stats on empty service
        let all_stats = service.all_pending_stats().await;
        assert_eq!(all_stats.len(), 0);
    }
}

