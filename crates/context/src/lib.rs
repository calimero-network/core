#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use std::collections::{btree_map, BTreeMap};
use std::future::Future;
use std::sync::Arc;

use actix::Actor;
use calimero_context_config::client::config::ClientConfig as ExternalClientConfig;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId};
use calimero_store::Store;
use either::Either;
use prometheus_client::registry::Registry;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::metrics::Metrics;

pub mod config;
pub mod error;
pub mod handlers;
mod metrics;

/// A metadata container for a single, in-memory context.
///
/// It holds the context's core properties and an asynchronous mutex (`lock`).
/// This lock is crucial for serializing operations on this specific context,
/// allowing the `ContextManager` to process requests for different contexts in parallel
/// while ensuring data consistency for any single context.
#[derive(Debug)]
struct ContextMeta {
    meta: Context,
    lock: Arc<Mutex<ContextId>>,
}

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
    /// The public-facing client API, also used internally to access convenience methods
    /// for interacting with the datastore.
    context_client: ContextClient,

    /// Configuration for interacting with external blockchain contracts (e.g., NEAR).
    external_config: ExternalClientConfig,

    /// An in-memory cache of active contexts (`ContextId` -> `ContextMeta`).
    /// This serves as a hot cache to avoid expensive disk I/O for frequently accessed contexts.
    // todo! potentially make this a dashmap::DashMap
    // todo! use cached::TimedSizedCache with a gc task
    contexts: BTreeMap<ContextId, ContextMeta>,
    /// An in-memory cache of application metadata (`ApplicationId` -> `Application`).
    /// Caching this prevents repeated fetching and parsing of application details.
    ///
    /// # Note
    /// Even when 2 applications point to the same bytecode,
    /// the application's metadata may include information
    /// that might be relevant in the compilation process,
    /// so we cannot blindly reuse compiled blobs across apps.
    applications: BTreeMap<ApplicationId, Application>,

    /// Prometheus metrics for monitoring the health and performance of the manager,
    /// such as number of active contexts, message processing latency, etc.
    metrics: Option<Metrics>,
    //
    // todo! when runtime let's us compile blobs separate from its
    // todo! execution, we can introduce a cached::TimedSizedCache
    // runtimes: TimedSizedCache<Exclusive<calimero_runtime::Engine>>,
    //
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
        Self {
            datastore,
            node_client,
            context_client,
            external_config,

            contexts: BTreeMap::new(),
            applications: BTreeMap::new(),

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
        let entry = self.contexts.entry(*context_id);

        match entry {
            btree_map::Entry::Occupied(mut occupied) => {
                // CRITICAL FIX: Always reload dag_heads from database to get latest state
                // The dag_heads can be updated by delta_store when receiving network deltas,
                // but the cached Context object won't reflect these changes.
                // This was causing all deltas to use genesis as parent instead of actual dag_heads.
                let handle = self.datastore.handle();
                let key = calimero_store::key::ContextMeta::new(*context_id);

                if let Some(meta) = handle.get(&key)? {
                    let cached = occupied.get_mut();

                    // Update dag_heads if they changed in DB
                    if cached.meta.dag_heads != meta.dag_heads {
                        tracing::debug!(
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

                Ok(Some(occupied.into_mut()))
            }
            btree_map::Entry::Vacant(vacant) => {
                let Some(context) = self.context_client.get_context(context_id)? else {
                    return Ok(None);
                };

                let lock = Arc::new(Mutex::new(*context_id));

                let item = vacant.insert(ContextMeta {
                    meta: context,
                    lock,
                });

                Ok(Some(item))
            }
        }
    }
}

// objectives:
//   keep up to N items, refresh entries as they are used
//   garbage collect entries as they expire, or as needed
//   share across tasks efficiently, not prolonging locks
//   managed mutation, so guards aren't held for too long
//
// result: this should help us share data between clients
//         and their actors,
//
// pub struct SharedCache<K, V> {
//     cache: DashMap<Key<K>, V>,
//     index: ArcTimedSizedCache<K, Key<K>>,
// }
//
// struct Key<K>(K);
// struct Cached<V: Copy>(..);
//        ^- aids read without locking
//           downside: Copy on every write
