//! Blob-related response types for the Calimero server API

use calimero_primitives::blobs::{BlobId, BlobInfo, BlobMetadata};
use serde::{Deserialize, Serialize};

/// Response for blob deletion operations
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub struct BlobDeleteResponse {
    /// The ID of the deleted blob
    pub blob_id: BlobId,
    /// Whether the blob was successfully deleted
    pub deleted: bool,
}

/// Response for blob listing operations
#[derive(Debug, Serialize, Deserialize)]
pub struct BlobListResponse {
    /// The blob list data
    pub data: BlobListResponseData,
}

/// Data contained in blob list responses
#[derive(Debug, Serialize, Deserialize)]
pub struct BlobListResponseData {
    /// List of blob information
    pub blobs: Vec<BlobInfo>,
}

/// Response for blob information retrieval
#[derive(Debug, Serialize, Deserialize)]
pub struct BlobInfoResponse {
    /// The blob metadata
    pub data: BlobMetadata,
}
