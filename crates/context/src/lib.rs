#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use actix::Actor;
use calimero_context_config::client::config::ClientConfig as ExternalClientConfig;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{Context, ContextId};
use calimero_store::Store;
use either::Either;
use tokio::sync::{Mutex, OwnedMutexGuard};

pub mod config;
pub mod handlers;

#[derive(Debug)]
struct ContextMeta {
    meta: Context,
    blob: BlobId,
    lock: Arc<Mutex<ContextId>>,
}

#[derive(Debug)]
pub struct ContextManager {
    datastore: Store,

    node_client: NodeClient,
    context_client: ContextClient,

    runtime_engine: calimero_runtime::Engine,

    external_config: ExternalClientConfig,

    // -- contexts --
    // todo! potentially make this a dashmap::DashMap
    // todo! use cached::TimedSizedCache with a gc task
    contexts: BTreeMap<ContextId, ContextMeta>,
    //
    // todo! when runtime let's us compile blobs separate from its
    // todo! execution, we can introduce a cached::TimedSizedCache
    // runtimes: TimedSizedCache<Exclusive<RuntimeInstance>>,
}

impl Actor for ContextManager {
    type Context = actix::Context<Self>;
}

impl ContextManager {
    fn get_context_exclusive(
        &mut self,
        context_id: &ContextId,
    ) -> Option<Either<OwnedMutexGuard<ContextId>, impl Future<Output = OwnedMutexGuard<ContextId>>>>
    {
        let context = self.contexts.get(&context_id)?;

        let Ok(guard) = context.lock.clone().try_lock_owned() else {
            return Some(Either::Right(context.lock.clone().lock_owned()));
        };

        Some(Either::Left(guard))
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
