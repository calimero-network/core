//! The peer route: blob share, authorized by context membership.

use std::fmt::Debug;

use std::sync::Arc;

use async_trait::async_trait;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use tracing::debug;

use crate::source::{AppRequest, AppSource};

/// The peer route's one capability, kept out of `ApplicationStore` so an http
/// node holds no handle that could reach a peer at all.
#[async_trait]
pub trait PeerBlobs: Debug + Send + Sync + 'static {
    /// Fetch from peers, authorized by context membership, storing what
    /// arrives. `None` means no peer had the bytes yet - not a fault.
    async fn fetch_bytecode_from_peers(
        &self,
        bytecode_id: &BlobId,
        context_id: &ContextId,
    ) -> eyre::Result<Option<Arc<[u8]>>>;
}

/// Peers, reached through the node's own blob share.
#[derive(Clone, Debug)]
pub struct DhtRegistry<P> {
    peers: P,
}

impl<P> DhtRegistry<P> {
    pub const fn new(peers: P) -> Self {
        Self { peers }
    }
}

#[async_trait]
impl<P: PeerBlobs> AppSource for DhtRegistry<P> {
    async fn fetch(&self, req: &AppRequest<'_>) -> eyre::Result<Option<Arc<[u8]>>> {
        // A context is required: this route authorizes by context membership
        // and has nothing to ask without one.
        let (Some(bytecode_id), Some(context_id)) = (req.bytecode_id, req.context_id) else {
            debug!(bytecode_id = ?req.bytecode_id, "no context for the peer route");
            return Ok(None);
        };
        self.peers
            .fetch_bytecode_from_peers(&bytecode_id, context_id)
            .await
    }
}
