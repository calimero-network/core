//! Where an application's bytes come from. One seam per route: bytes in,
//! verification and install left to the downloader.

use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;

use crate::registry::{RegistryConfig, RegistryMode};
use crate::source::dht::{DhtRegistry, PeerBlobs};
use crate::source::http::HttpRegistry;

pub mod dht;
pub mod http;

/// What a source is asked for. The coordinates are the route; `bytecode_id` is
/// the authority the downloader verifies whatever arrives against.
#[derive(Clone, Copy, Debug)]
pub struct AppRequest<'a> {
    pub application_id: Option<ApplicationId>, // absent on a bare install: the manifest decides
    pub package: &'a str,
    pub version: &'a str,
    pub bytecode_id: Option<BlobId>,
    pub context_id: Option<&'a ContextId>, // the dht source authorizes by membership
}

#[async_trait]
pub trait AppSource: Debug + Send + Sync + 'static {
    /// Unverified bundle bytes for this request; `Ok(None)` = the source had
    /// none yet (retryable), `Err` = real fault. Verification is download()'s.
    async fn fetch(&self, req: &AppRequest<'_>) -> eyre::Result<Option<Arc<[u8]>>>;
}

/// The one source this node resolves applications from. There is no second
/// route behind it: whichever mode is configured is the whole answer.
pub fn app_source<P: PeerBlobs>(
    cfg: &RegistryConfig,
    peers: P,
) -> eyre::Result<Arc<dyn AppSource>> {
    match cfg.mode {
        RegistryMode::Http => Ok(Arc::new(HttpRegistry::new(cfg.http_base()?.clone())?)),
        RegistryMode::Dht => Ok(Arc::new(DhtRegistry::new(peers))),
    }
}
