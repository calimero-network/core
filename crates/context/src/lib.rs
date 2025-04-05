#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]

use std::collections::BTreeMap;
use std::sync::Arc;

use actix::Actor;
use calimero_context_config::client::config::ClientConfig as ExternalClientConfig;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{Context, ContextId};
use calimero_store::Store;
use tokio::sync::Mutex;

pub mod config;
pub mod handlers;

#[derive(Debug)]
struct ContextMeta {
    meta: Context,
    blob: BlobId,
    lock: Option<Arc<Mutex<ContextId>>>,
}

#[derive(Debug)]
pub struct ContextManager {
    datastore: Store,
    // todo! use LruCache with a task interval for garbage collection
    blobs: BTreeMap<BlobId, Arc<Box<[u8]>>>,
    contexts: BTreeMap<ContextId, ContextMeta>,
    // todo! when runtime let's us compile blobs separate
    // todo! from execution, we can introduce an LruCache here
    // runtimes: Vec<Arc<Mutex<RuntimeInstance>>>,
    node_client: NodeClient,
    context_client: ContextClient,
    external_config: ExternalClientConfig,
}

impl Actor for ContextManager {
    type Context = actix::Context<Self>;
}
