//! Server-side functionality for Calimero contexts

// TODO: inline handlers/manager modules here during consolidation

use std::collections::{btree_map, BTreeMap};
use std::future::Future;
use std::sync::Arc;

use actix::Actor;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId};
use calimero_store::Store;
use either::Either;
use prometheus_client::registry::Registry;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::client::ContextClient;
use crate::client::config::ClientConfig;
use crate::metrics::Metrics;

#[derive(Debug)]
struct ContextMeta {
    meta: Context,
    lock: Arc<Mutex<ContextId>>,
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

#[derive(Debug)]
pub struct ContextManager {
    datastore: Store,
    node_client: NodeClient,
    context_client: ContextClient,
    external_config: ClientConfig,
    contexts: BTreeMap<ContextId, ContextMeta>,
    applications: BTreeMap<ApplicationId, Application>,
    metrics: Metrics,
}

impl ContextManager {
    pub fn new(
        datastore: Store,
        node_client: NodeClient,
        context_client: ContextClient,
        external_config: ClientConfig,
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

impl Actor for ContextManager {
    type Context = actix::Context<Self>;
}
