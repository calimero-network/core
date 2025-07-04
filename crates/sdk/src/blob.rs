//! Simple blob API for handling large binary data.
//!
//! This provides a straightforward API for blob operations that uses
//! the existing add_blob and get_blob_bytes methods.

// Re-export BlobId for applications
pub use calimero_primitives::blobs::BlobId;

use crate::env;

/// Store data as a blob and return its ID
/// Uses the existing add_blob infrastructure via simplified host function
pub fn store_blob(data: &[u8]) -> Result<BlobId, String> {
    let blob_id_bytes =
        env::store_blob_bytes(data).map_err(|e| format!("Failed to store blob: {:?}", e))?;
    Ok(BlobId::from(blob_id_bytes))
}

/// Load a blob by its ID  
/// Uses the existing get_blob_bytes infrastructure via simplified host function
pub fn load_blob(blob_id: &BlobId) -> Result<Option<Vec<u8>>, String> {
    match env::load_blob_bytes(blob_id.as_ref()) {
        Ok(data) => Ok(Some(data)),
        Err(_) => Ok(None), // Blob not found
    }
}

/// Check if a blob exists
/// This is a convenience function that just checks if load_blob returns data
pub fn blob_exists(blob_id: &BlobId) -> bool {
    load_blob(blob_id)
        .map(|data| data.is_some())
        .unwrap_or(false)
}
