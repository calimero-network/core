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

pub mod api;
pub mod auth;
pub mod config;
pub mod handlers;
pub mod performance;
pub mod providers;
pub mod storage;

#[derive(Debug)]
struct ContextMeta {
    meta: Context,
    lock: Arc<Mutex<ContextId>>,
}

#[derive(Debug)]
pub struct ContextManager {
    datastore: Store,

    node_client: NodeClient,
    context_client: ContextClient,

    external_config: ExternalClientConfig,

    // todo! potentially make this a dashmap::DashMap
    // todo! use cached::TimedSizedCache with a gc task
    contexts: BTreeMap<ContextId, ContextMeta>,
    // even when 2 applications point to the same bytecode,
    // the application's metadata may include information
    // that might be relevant in the compilation process,
    // so we cannot blindly reuse compiled blobs across apps.
    applications: BTreeMap<ApplicationId, Application>,
    //
    // todo! when runtime let's us compile blobs separate from its
    // todo! execution, we can introduce a cached::TimedSizedCache
    // runtimes: TimedSizedCache<Exclusive<calimero_runtime::Engine>>,
    //
    metrics: Metrics,
}

impl ContextManager {
    pub fn new(
        datastore: Store,
        node_client: NodeClient,
        context_client: ContextClient,
        external_config: ExternalClientConfig,
        prom_registry: &mut Registry,
    ) -> Self {
        Self {
            datastore,
            node_client,
            context_client,
            external_config,

            contexts: BTreeMap::new(),
            applications: BTreeMap::new(),

            metrics: Metrics::new(prom_registry),
        }
    }
}

impl Actor for ContextManager {
    type Context = actix::Context<Self>;
}

impl ContextMeta {
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
    fn get_or_fetch_context(
        &mut self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<&ContextMeta>> {
        let entry = self.contexts.entry(*context_id);

        match entry {
            btree_map::Entry::Occupied(occupied) => Ok(Some(occupied.into_mut())),
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
