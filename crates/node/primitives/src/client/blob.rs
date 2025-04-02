use calimero_blobstore::{Blob, Size};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::hash::Hash;
use eyre::bail;
use futures_util::AsyncRead;

use super::NodeClient;

impl NodeClient {
    pub async fn add_blob<S: AsyncRead>(
        &self,
        stream: S,
        expected_size: Option<u64>,
        expected_hash: Option<Hash>,
    ) -> eyre::Result<(BlobId, u64)> {
        let (blob_id, hash, size) = self
            .blobstore
            .put_sized(expected_size.map(Size::Exact), stream)
            .await?;

        if matches!(expected_hash, Some(expected_hash) if hash != expected_hash) {
            bail!("fatal: blob hash mismatch");
        }

        if matches!(expected_size, Some(expected_size) if size != expected_size) {
            bail!("fatal: blob size mismatch");
        }

        Ok((blob_id, size))
    }

    pub fn get_blob(&self, blob_id: BlobId) -> eyre::Result<Option<Blob>> {
        let Some(stream) = self.blobstore.get(blob_id)? else {
            return Ok(None);
        };

        Ok(Some(stream))
    }

    pub fn has_blob(&self, blob_id: BlobId) -> eyre::Result<bool> {
        self.blobstore.has(blob_id)
    }
}
