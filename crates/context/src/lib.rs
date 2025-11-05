#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use std::future::Future;
use std::sync::Arc;

use actix::Actor;
use calimero_context_config::client::config::ClientConfig as ExternalClientConfig;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_store::Store;
use either::Either;
use prometheus_client::registry::Registry;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::application_manager::ApplicationManager;
use crate::metrics::Metrics;
use crate::repository::ContextRepository;

mod application_manager;
pub mod config;
pub mod handlers;
mod metrics;
mod repository;

// Re-export ContextMeta from repository module
pub(crate) use repository::ContextMeta;

/// The central actor responsible for managing the lifecycle of all contexts.
///
/// As an actor, it maintains its own state and processes incoming messages
/// sequentially from a mailbox.
#[derive(Debug)]
pub struct ContextManager {
    /// Handle to the persistent key-value store. Used for fetching context data on cache misses.
    datastore: Store,

    /// Client for interacting with the underlying Calimero node.
    node_client: NodeClient,

    /// Context repository for storage and caching operations.
    /// Encapsulates all context CRUD and cache management logic.
    repository: ContextRepository,

    /// Application manager for application and module lifecycle.
    /// Handles application metadata caching and WASM module compilation.
    app_manager: ApplicationManager,

    /// Configuration for interacting with external blockchain contracts (e.g., NEAR).
    external_config: ExternalClientConfig,

    /// Prometheus metrics for monitoring the health and performance of the manager,
    /// such as number of active contexts, message processing latency, etc.
    metrics: Option<Metrics>,
}

/// Creates a new `ContextManager`.
///
/// # Arguments
///
/// * `datastore` - The persistent storage backend.
/// * `node_client` - Client for interacting with the underlying Calimero node.
/// * `context_client` - The context client facade.
/// * `external_config` - Configuration for interacting with external blockchain contracts (e.g.,
/// NEAR).
/// * `prometheus_registry` - A mutable reference to a Prometheus registry for registering metrics.
impl ContextManager {
    pub fn new(
        datastore: Store,
        node_client: NodeClient,
        context_client: ContextClient,
        external_config: ExternalClientConfig,
        prometheus_registry: Option<&mut Registry>,
    ) -> Self {
        // Create repository with default cache size (1000)
        let repository = ContextRepository::new(
            datastore.clone(),
            context_client,
            None, // Use default cache size
        );

        // Create application manager
        let app_manager = ApplicationManager::new(node_client.clone());

        Self {
            datastore,
            node_client,
            repository,
            app_manager,
            external_config,

            metrics: prometheus_registry.map(Metrics::new),
        }
    }
}

/// Implements the `Actor` trait for `ContextManager`, allowing it to run within the Actix framework.
///
/// By implementing `Actor`, `ContextManager` gains a "Context" (an execution environment) and a mailbox.
/// Messages sent to the manager are queued in its mailbox and processed one at a time in the order
/// they are received, which is the core of the actor model's safety guarantee for its internal state.
impl Actor for ContextManager {
    type Context = actix::Context<Self>;
}

impl ContextMeta {
    /// Acquires an asynchronous lock for this specific context.
    ///
    /// This is a performance-optimized lock acquisition strategy. It first attempts an
    /// optimistic, non-blocking `try_lock_owned()`. This is very fast if the lock is not contended.
    ///
    /// # Returns
    ///
    /// An `Either` enum containing one of two possibilities:
    /// - `Either::Left(OwnedMutexGuard)`: If the lock was acquired immediately without waiting.
    /// - `Either::Right(impl Future)`: If the lock was contended. This future will resolve
    ///    to an `OwnedMutexGuard` once the lock becomes available. The caller must `.await` this future.
    fn lock(
        &self,
    ) -> Either<OwnedMutexGuard<ContextId>, impl Future<Output = OwnedMutexGuard<ContextId>>> {
        let Ok(guard) = self.lock.clone().try_lock_owned() else {
            return Either::Right(self.lock.clone().lock_owned());
        };

        Either::Left(guard)
    }
}

impl ContextManager {
    /// Get access to the context client (via repository).
    ///
    /// This provides access to the underlying `ContextClient` for database operations.
    pub(crate) fn context_client(&self) -> &ContextClient {
        self.repository.context_client()
    }

    /// Retrieves context metadata, fetching from the datastore if not present in the cache.
    ///
    /// This function implements the "cache-aside" pattern. It first checks the in-memory
    /// `contexts` BTreeMap. On a cache miss, it falls back to querying the persistent
    /// `datastore` via the `context_client`, populates the cache with the result,
    /// and then returns the data.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The unique identifier of the context to retrieve.
    ///
    /// # Returns
    ///
    /// A `Result` containing an `Option` with a reference to the `ContextMeta`.
    /// Returns `Ok(Some(&ContextMeta))` if the context is found in the cache or datastore.
    /// Returns `Ok(None)` if the context does not exist in the datastore.
    /// Returns `Err` if a datastore error occurs.
    fn get_or_fetch_context(
        &mut self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<&ContextMeta>> {
        // Delegate to repository
        self.repository.get(context_id)
    }

    /// Invalidate a context's cache entry (called after external metadata updates).
    pub(crate) fn invalidate_context_cache(&mut self, context_id: &ContextId) {
        self.repository.invalidate(context_id);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Module Boundaries & Architecture Notes
// ═══════════════════════════════════════════════════════════════════════════
//
// As of Nov 2024 refactoring, ContextManager is a COORDINATOR, not a god object.
// It delegates to specialized services:
//
// 1. ContextRepository (repository.rs)
//    - Context CRUD operations
//    - LRU cache management (max 1000 contexts)
//    - Database synchronization
//    - DAG heads refresh
//
// 2. ApplicationManager (application_manager.rs)
//    - Application metadata caching
//    - Application lifecycle management
//    - Future: Module compilation caching (when Module implements Clone)
//
// 3. ContextManager (this file)
//    - Actor coordination
//    - Message routing
//    - External config management
//    - Thin wrapper delegating to services
//
// OLD ARCHITECTURE (removed):
// - Direct BTreeMap<ContextId, ContextMeta> (now in ContextRepository)
// - Direct BTreeMap<ApplicationId, Application> (now in ApplicationManager)
// - ~300 lines of cache management logic (now in services)
//
// ═══════════════════════════════════════════════════════════════════════════
