//! The one seam between the downloader and the node it runs inside.

use std::sync::Arc;

use async_trait::async_trait;
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;

/// What an installed application row says, for the two questions the
/// downloader asks of it.
#[derive(Clone, Debug)]
pub struct InstalledApplication {
    pub bytecode_id: BlobId,
    pub source: String,
}

/// What the downloader needs from the node. One impl; it exists only to keep
/// this crate a leaf, so do not grow it into a general node facade.
#[async_trait]
pub trait ApplicationStore {
    /// Whether these bytes are already in the local blobstore.
    fn has_bytecode(&self, bytecode_id: &BlobId) -> eyre::Result<bool>;

    /// The row stored under `application_id`, if one exists.
    fn installed_application(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<InstalledApplication>>;

    /// Read bytes already held locally.
    async fn read_local_bytecode(&self, bytecode_id: &BlobId) -> eyre::Result<Option<Arc<[u8]>>>;

    /// Store `bytes`, returning the blob id they hash to and their stored size.
    async fn store_bytecode(&self, bytes: &[u8]) -> eyre::Result<(BlobId, u64)>;

    /// Release a blob this download stored. Nothing else reclaims one, so a
    /// rejected artifact would otherwise sit on disk forever.
    async fn release_bytecode(&self, bytecode_id: BlobId) -> eyre::Result<()>;

    /// Install `bytes` under `application_id`: only a signed bundle deriving this
    /// id binds, and it never rolls the row back. Raw wasm derives no id, so it is refused.
    async fn bind_application(
        &self,
        application_id: &ApplicationId,
        bytecode_id: BlobId,
        size: u64,
        source: &ApplicationSource,
        bytes: &[u8],
    ) -> eyre::Result<()>;
}
