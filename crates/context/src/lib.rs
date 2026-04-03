#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use std::collections::{btree_map, BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;

use actix::{Actor, AsyncContext};
use calimero_context_config::types::ContextGroupId;
use calimero_context_client::client::ContextClient;
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_dag::DagStore;
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
pub mod governance_dag;
pub mod group_store;
pub mod handlers;
mod lifecycle;
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

    /// Groups that currently have a running upgrade propagator. Prevents the
    /// manual retry handler from spawning a second propagator while an
    /// existing one is still active (e.g. sleeping in its backoff delay).
    active_propagators: HashSet<ContextGroupId>,

    /// Per-namespace governance DAG. Single DAG per namespace containing both
    /// root ops and encrypted group-scoped ops.
    namespace_dags: HashMap<[u8; 32], Arc<tokio::sync::Mutex<DagStore<SignedNamespaceOp>>>>,
}

/// Creates a new `ContextManager`.
///
/// # Arguments
///
/// * `datastore` - The persistent storage backend.
/// * `node_client` - Client for interacting with the underlying Calimero node.
/// * `context_client` - The context client facade.
/// * `prometheus_registry` - A mutable reference to a Prometheus registry for registering metrics.
impl ContextManager {
    pub fn new(
        datastore: Store,
        node_client: NodeClient,
        context_client: ContextClient,
        prometheus_registry: Option<&mut Registry>,
    ) -> Self {
        Self {
            datastore,
            node_client,
            context_client,

            contexts: BTreeMap::new(),
            applications: BTreeMap::new(),

            metrics: prometheus_registry.map(Metrics::new),
            active_propagators: HashSet::new(),
            namespace_dags: HashMap::new(),
        }
    }

    /// Get this node's identity for the namespace (root group) that contains `group_id`.
    /// Returns `None` if no identity has been stored for that namespace yet.
    pub fn node_namespace_identity(
        &self,
        group_id: &ContextGroupId,
    ) -> Option<(calimero_primitives::identity::PublicKey, [u8; 32])> {
        match group_store::resolve_namespace_identity(&self.datastore, group_id) {
            Ok(Some((pk, sk, _sender))) => Some((pk, sk)),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(?group_id, error=?e, "failed to resolve namespace identity");
                None
            }
        }
    }

    /// Get or create this node's identity for the namespace containing `group_id`.
    /// Generates a new keypair if none exists. Returns (namespace_id, public_key, private_key, sender_key).
    pub fn get_or_create_namespace_identity(
        &self,
        group_id: &ContextGroupId,
    ) -> eyre::Result<(
        ContextGroupId,
        calimero_primitives::identity::PublicKey,
        [u8; 32],
        [u8; 32],
    )> {
        group_store::get_or_create_namespace_identity(&self.datastore, group_id)
    }
}

/// Result of the governance preflight check, containing everything needed
/// to sign and publish a governance op.
pub struct GovernancePreflight {
    /// The resolved requester public key.
    pub requester: calimero_primitives::identity::PublicKey,
    /// The signing private key (as raw bytes).
    pub signing_key: [u8; 32],
    /// Cloned datastore for use in async blocks.
    pub datastore: Store,
    /// Cloned node client for use in async blocks.
    pub node_client: calimero_node_primitives::client::NodeClient,
}

impl GovernancePreflight {
    /// Convenience: build a `PrivateKey` from the stored signing key bytes.
    pub fn signer_sk(&self) -> calimero_primitives::identity::PrivateKey {
        calimero_primitives::identity::PrivateKey::from(self.signing_key)
    }
}

impl ContextManager {
    /// Common preflight for governance mutation handlers.
    ///
    /// Resolves the requester identity, loads group metadata, checks admin
    /// authorization, resolves or stores the signing key, and returns
    /// everything needed for `sign_apply_and_publish`.
    ///
    /// Returns `Err` if the group doesn't exist, the requester isn't authorized,
    /// or no signing key is available.
    pub fn governance_preflight(
        &self,
        group_id: &ContextGroupId,
        requester: Option<calimero_primitives::identity::PublicKey>,
        require_admin: bool,
    ) -> eyre::Result<GovernancePreflight> {
        let node_identity = self.node_namespace_identity(group_id);

        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    eyre::bail!(
                        "requester not provided and node has no configured group identity"
                    )
                }
            },
        };

        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        if group_store::load_group_meta(&self.datastore, group_id)?.is_none() {
            eyre::bail!("group '{group_id:?}' not found");
        }
        if require_admin {
            group_store::require_group_admin(&self.datastore, group_id, &requester)?;
        }
        if signing_key.is_none() {
            group_store::require_group_signing_key(&self.datastore, group_id, &requester)?;
        }

        if let Some(ref sk) = signing_key {
            let _ =
                group_store::store_group_signing_key(&self.datastore, group_id, &requester, sk);
        }

        let effective_signing_key = signing_key.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, group_id, &requester)
                .ok()
                .flatten()
        });

        let sk_bytes = effective_signing_key.ok_or_else(|| {
            eyre::eyre!("local group governance requires a signing key for the requester")
        })?;

        Ok(GovernancePreflight {
            requester,
            signing_key: sk_bytes,
            datastore: self.datastore.clone(),
            node_client: self.node_client.clone(),
        })
    }
}

impl ContextManager {
    fn get_or_create_namespace_dag(
        &mut self,
        namespace_id: &[u8; 32],
    ) -> Arc<tokio::sync::Mutex<DagStore<SignedNamespaceOp>>> {
        self.namespace_dags
            .entry(*namespace_id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(DagStore::new([0u8; 32]))))
            .clone()
    }
}

/// Implements the `Actor` trait for `ContextManager`, allowing it to run within the Actix framework.
///
/// By implementing `Actor`, `ContextManager` gains a "Context" (an execution environment) and a mailbox.
/// Messages sent to the manager are queued in its mailbox and processed one at a time in the order
/// they are received, which is the core of the actor model's safety guarantee for its internal state.
impl Actor for ContextManager {
    type Context = actix::Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.recover_in_progress_upgrades(ctx);
        self.start_namespace_heartbeat(ctx);
    }
}

// Lifecycle methods (recover_in_progress_upgrades, start_namespace_heartbeat)
// are in lifecycle.rs

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
