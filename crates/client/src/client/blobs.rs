//! Blob API methods for the Calimero client.

use calimero_primitives::blobs::{BlobId, BlobInfo, BlobMetadata};
use calimero_primitives::context::ContextId;
use calimero_server_primitives::blob::{BlobDeleteResponse, BlobInfoResponse, BlobListResponse};
use eyre::Result;

use super::Client;
use crate::traits::{ClientAuthenticator, ClientStorage};

impl<A, S> Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    fn blobs_path(context_id: Option<&ContextId>) -> String {
        match context_id {
            Some(ctx_id) => format!("admin-api/blobs?context_id={ctx_id}"),
            None => "admin-api/blobs".to_owned(),
        }
    }

    fn blob_path(blob_id: &BlobId, context_id: Option<&ContextId>) -> String {
        match context_id {
            Some(ctx_id) => format!("admin-api/blobs/{blob_id}?context_id={ctx_id}"),
            None => format!("admin-api/blobs/{blob_id}"),
        }
    }

    pub async fn delete_blob(&self, blob_id: &BlobId) -> Result<BlobDeleteResponse> {
        let response = self
            .connection
            .delete(&format!("admin-api/blobs/{blob_id}"))
            .await?;
        Ok(response)
    }

    pub async fn list_blobs(&self) -> Result<BlobListResponse> {
        let response = self.connection.get("admin-api/blobs").await?;
        Ok(response)
    }

    pub async fn get_blob_info(&self, blob_id: &BlobId) -> Result<BlobInfoResponse> {
        let headers = self
            .connection
            .head(&format!("admin-api/blobs/{blob_id}"))
            .await?;

        let size = headers
            .get("content-length")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let mime_type = headers
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();

        let hash_hex = headers
            .get("x-blob-hash")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");

        let hash =
            hex::decode(hash_hex).map_err(|_| eyre::eyre!("Invalid hash in response headers"))?;

        let hash_array: [u8; 32] = hash
            .try_into()
            .map_err(|_| eyre::eyre!("Hash must be 32 bytes"))?;

        let blob_info = BlobInfoResponse {
            data: BlobMetadata {
                blob_id: *blob_id,
                size,
                mime_type,
                hash: hash_array,
            },
        };

        Ok(blob_info)
    }

    pub async fn upload_blob(
        &self,
        data: Vec<u8>,
        context_id: Option<&ContextId>,
    ) -> Result<BlobInfo> {
        let path = Self::blobs_path(context_id);

        let response = self.connection.put_binary(&path, data).await?;

        #[derive(serde::Deserialize)]
        struct BlobUploadResponse {
            data: BlobInfo,
        }

        let upload_response: BlobUploadResponse = response.json().await?;
        Ok(upload_response.data)
    }

    pub async fn download_blob(
        &self,
        blob_id: &BlobId,
        context_id: Option<&ContextId>,
    ) -> Result<Vec<u8>> {
        let path = Self::blob_path(blob_id, context_id);
        let data = self.connection.get_binary(&path).await?;
        Ok(data)
    }
}
