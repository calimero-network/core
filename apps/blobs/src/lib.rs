//! # Blob API Implementation Example
//!
//! Minimal file sharing app demonstrating Calimero blob storage.
//!
//! See README.md for complete documentation and usage examples.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env, BlobId, PublicKey};
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap};

// === CONSTANTS ===

/// Bytes per kilobyte
const BYTES_PER_KB: f64 = 1024.0;

/// Bytes per megabyte
const BYTES_PER_MB: f64 = BYTES_PER_KB * 1024.0;

// === DATA STRUCTURES ===

/// Represents a file stored in the system
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct FileRecord {
    /// Unique file identifier, namespaced by the uploader's identity to stay
    /// collision-free across replicas (e.g. "<uploader-base58>_0").
    pub id: String,

    /// Human-readable file name (e.g., "document.pdf", "image.png")
    pub name: String,

    /// Blob ID. Serializes to/from a base58 string in JSON (e.g.
    /// "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty") via the SDK's
    /// `BlobId` newtype, so no per-app encoding helper is needed.
    pub blob_id: BlobId,

    /// File size in bytes
    pub size: u64,

    /// MIME type following RFC 6838 standard
    /// Examples: "application/pdf", "image/png", "text/plain", "video/mp4"
    pub mime_type: String,

    /// Uploader's identity as base58-encoded public key
    /// Derived from `env::executor_id()` and converted to base58 string
    pub uploaded_by: String,

    /// Upload timestamp in milliseconds since Unix epoch (January 1, 1970 00:00:00 UTC)
    /// Obtained from `env::time_now()`
    pub uploaded_at: u64,
}

// Atomic whole-record LWW by `uploaded_at` (see `impl_atomic_lww!`); not a
// struct of CRDT fields, so `#[derive(Mergeable)]` doesn't apply.
calimero_storage::impl_atomic_lww!(FileRecord, uploaded_at);

/// Application state for the file sharing system.
#[app::state(emits = FileShareEvent)]
pub struct FileShareState {
    /// Context owner's identity as base58-encoded public key.
    /// Set during initialization from `env::executor_id()`.
    pub owner: LwwRegister<String>,

    /// Map of file ID to file metadata records.
    /// Key: file ID (e.g. "<uploader-base58>_0"), Value: FileRecord.
    pub files: UnorderedMap<String, FileRecord>,

    /// Monotonic counter used to generate unique file IDs.
    pub file_counter: Counter,
}

/// Events emitted by the application
#[app::event]
pub enum FileShareEvent {
    /// Emitted when a file is successfully uploaded
    FileUploaded {
        /// Unique file identifier (e.g. "<uploader-base58>_0")
        id: String,
        /// File name
        name: String,
        /// File size in bytes
        size: u64,
        /// Uploader's base58-encoded public key
        uploader: String,
    },
    /// Emitted when a file is deleted
    FileDeleted {
        /// ID of the deleted file
        id: String,
        /// Name of the deleted file
        name: String,
    },
}

// === APPLICATION LOGIC ===

#[app::logic]
impl FileShareState {
    /// Initialize a new file sharing context
    #[app::init]
    pub fn init() -> FileShareState {
        // `executor_id()` is the caller's public key, so render it with the
        // SDK's `PublicKey` newtype rather than the (now-removed) blob helper.
        let owner = PublicKey::from(env::executor_id()).to_string();

        app::log!("Initializing file sharing app for owner: {}", owner);

        FileShareState {
            owner: LwwRegister::new(owner),
            files: UnorderedMap::new(),
            file_counter: Counter::new(),
        }
    }

    /// Upload a file by storing its blob ID and metadata
    ///
    /// The client first uploads the file binary using `blobClient.uploadBlob()` which
    /// returns a blob_id. This method then stores the metadata and announces the blob
    /// to the network so other nodes can discover and download it.
    ///
    /// # Arguments
    /// * `name` - Human-readable file name
    /// * `blob_id` - Blob ID (base58 string over the wire; obtained from the blob client)
    /// * `size` - File size in bytes
    /// * `mime_type` - MIME type (e.g., "application/pdf", "image/png")
    ///
    /// # Returns
    /// * `Ok(String)` - The generated file ID, namespaced by uploader identity
    ///   (e.g. "<uploader-base58>_0")
    /// * `Err(app::Error)` - Error if storage operation fails
    pub fn upload_file(
        &mut self,
        name: String,
        blob_id: BlobId,
        size: u64,
        mime_type: String,
    ) -> app::Result<String> {
        // File IDs must be unique across replicas. The counter alone is not
        // enough: it's a CRDT whose value converges, so two nodes uploading
        // concurrently both read the same value and would mint the same ID,
        // and one upload would overwrite the other on merge. Namespacing the
        // ID with the uploader's identity makes it collision-free — each node
        // has a distinct executor key, so `<uploader>_<counter>` is globally
        // unique even when counters coincide.
        let uploader = PublicKey::from(env::executor_id()).to_string();
        let next_id = self.file_counter.value()?;
        let file_id = format!("{uploader}_{next_id}");
        self.file_counter.increment()?;

        let timestamp = env::time_now();

        // BLOB API: Announce blob to network for peer discovery
        // This makes the blob discoverable by other nodes in this context
        let current_context = env::context_id();
        if env::blob_announce_to_context(blob_id.as_ref(), &current_context) {
            app::log!("Announced blob {} to network", blob_id);
        } else {
            app::log!("Warning: Failed to announce blob {}", blob_id);
        }

        // Create the file record
        let file_record = FileRecord {
            id: file_id.clone(),
            name: name.clone(),
            blob_id,
            size,
            mime_type,
            uploaded_by: uploader.clone(),
            uploaded_at: timestamp,
        };

        // Store the file record
        self.files.insert(file_id.clone(), file_record)?;

        // Emit event
        app::emit!(FileShareEvent::FileUploaded {
            id: file_id.clone(),
            name: name.clone(),
            size,
            uploader,
        });

        app::log!("File uploaded successfully: {} (ID: {})", name, file_id);

        Ok(file_id)
    }

    /// Delete a file by its ID
    ///
    /// Note: This only removes the file metadata from contract storage.
    /// The actual blob data remains in the blob store, as the SDK does not
    /// currently expose blob deletion methods.
    ///
    /// # Arguments
    /// * `file_id` - The ID of the file to delete (e.g. "<uploader-base58>_0")
    ///
    /// # Returns
    /// * `Ok(())` - File metadata successfully deleted
    /// * `Err(app::Error)` - Error if file not found or deletion fails
    pub fn delete_file(&mut self, file_id: String) -> app::Result<()> {
        // Retrieve the file before deleting to get its name for the event
        let file_record = self
            .files
            .get(&file_id)?
            .ok_or_else(|| app::err!("File not found: {file_id}"))?;

        let file_name = file_record.name.clone();

        // Remove the file metadata from storage
        // NOTE: The underlying blob is not deleted from blob storage
        self.files.remove(&file_id)?;

        // Emit event
        app::emit!(FileShareEvent::FileDeleted {
            id: file_id.clone(),
            name: file_name.clone(),
        });

        app::log!("File deleted: {} (ID: {})", file_name, file_id);

        Ok(())
    }

    /// List all files in the system
    ///
    /// # Returns
    /// * `Ok(Vec<FileRecord>)` - Vector of all file records with complete metadata (not just names)
    /// * `Err(app::Error)` - Error if storage operation fails (rarely occurs)
    pub fn list_files(&self) -> app::Result<Vec<FileRecord>> {
        let mut files = Vec::new();

        for (_, file_record) in self.files.entries()? {
            files.push(file_record.clone());
        }

        app::log!("Listed {} files", files.len());

        Ok(files)
    }

    /// Get a specific file by ID
    ///
    /// # Arguments
    /// * `file_id` - The ID of the file to retrieve (e.g. "<uploader-base58>_0")
    ///
    /// # Returns
    /// * `Ok(FileRecord)` - Complete file record with all metadata
    /// * `Err(app::Error)` - Error if file not found or retrieval fails
    pub fn get_file(&self, file_id: String) -> app::Result<FileRecord> {
        let Some(file_record) = self.files.get(&file_id)? else {
            app::bail!("File not found: {file_id}");
        };

        Ok(file_record.clone())
    }

    /// Get blob ID for download (base58-encoded)
    ///
    /// Use this to retrieve the blob ID for downloading the actual file content
    /// via `blobClient.downloadBlob(blob_id, context_id)`.
    ///
    /// # Arguments
    /// * `file_id` - The ID of the file (e.g. "<uploader-base58>_0")
    ///
    /// # Returns
    /// * `Ok(BlobId)` - The blob ID (base58 string over the wire, e.g.
    ///   "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty")
    /// * `Err(app::Error)` - Error if file not found
    pub fn get_blob_id_b58(&self, file_id: String) -> app::Result<BlobId> {
        let file_record = self.get_file(file_id)?;
        Ok(file_record.blob_id)
    }

    /// Search files by name (case-insensitive substring match)
    ///
    /// # Arguments
    /// * `query` - Search term to match against file names
    ///
    /// # Returns
    /// * `Ok(Vec<FileRecord>)` - Vector of matching file records (not just names), may be empty if no matches
    /// * `Err(app::Error)` - Error if storage operation fails (rarely occurs)
    pub fn search_files(&self, query: String) -> app::Result<Vec<FileRecord>> {
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        for (_, file_record) in self.files.entries()? {
            if file_record.name.to_lowercase().contains(&query_lower) {
                results.push(file_record.clone());
            }
        }

        app::log!("Search for '{}' found {} results", query, results.len());

        Ok(results)
    }

    /// Get total size of all files in bytes
    ///
    /// Calculates the sum of all file sizes (blob data only).
    /// Note: This does not include contract storage overhead (FileRecord structs, map overhead, etc.).
    ///
    /// # Returns
    /// * `Ok(u64)` - Total size of all files in bytes (sum of file sizes)
    /// * `Err(app::Error)` - Error if storage operation fails (rarely occurs)
    pub fn get_total_files_size(&self) -> app::Result<u64> {
        let mut total_size = 0u64;

        for (_, file_record) in self.files.entries()? {
            // Saturate rather than overflow: adversarial or corrupt sizes
            // would otherwise panic in debug and wrap in release.
            total_size = total_size.saturating_add(file_record.size);
        }

        Ok(total_size)
    }

    /// Get file sharing statistics as a formatted string
    ///
    /// # Returns
    /// * `Ok(String)` - Formatted statistics including file count, total file size (not contract storage), and owner
    /// * `Err(app::Error)` - Error if storage operations fail
    ///
    /// # Example Output
    /// ```text
    /// File Sharing Statistics:
    /// - Total files: 3
    /// - Total storage: 2.44 MB (2564096 bytes)
    /// - Owner: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
    /// ```
    ///
    /// Note: "Total storage" refers to the sum of all file sizes, not the actual
    /// contract storage usage (which would include metadata overhead).
    pub fn get_stats(&self) -> app::Result<String> {
        let file_count = self.files.len()?;

        let total_size = self.get_total_files_size()?;

        let total_mb = (total_size as f64) / BYTES_PER_MB;

        Ok(format!(
            "File Sharing Statistics:\n\
             - Total files: {}\n\
             - Total storage: {:.2} MB ({} bytes)\n\
             - Owner: {}",
            file_count,
            total_mb,
            total_size,
            self.owner.get()
        ))
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    // An arbitrary blob id for metadata-only tests (no bytes needed).
    fn blob_id() -> BlobId {
        BlobId::from([7u8; 32])
    }

    #[test]
    fn upload_list_and_delete() {
        let mut app = TestHost::new(FileShareState::init);

        let file_id = app
            .call(|s| s.upload_file("notes.txt".into(), blob_id(), 12, "text/plain".into()))
            .unwrap();

        assert_eq!(app.view(|s| s.list_files()).unwrap().len(), 1);
        assert_eq!(app.view(|s| s.get_total_files_size()).unwrap(), 12);
        assert_eq!(
            app.view(|s| s.get_blob_id_b58(file_id.clone())).unwrap(),
            blob_id()
        );

        app.call(|s| s.delete_file(file_id.clone())).unwrap();
        assert_eq!(app.view(|s| s.list_files()).unwrap().len(), 0);
        assert_eq!(app.view(|s| s.get_total_files_size()).unwrap(), 0);
    }

    #[test]
    fn search_matches_by_name() {
        let mut app = TestHost::new(FileShareState::init);

        app.call(|s| s.upload_file("report.pdf".into(), blob_id(), 5, "application/pdf".into()))
            .unwrap();
        app.call(|s| s.upload_file("photo.png".into(), blob_id(), 7, "image/png".into()))
            .unwrap();

        assert_eq!(
            app.view(|s| s.search_files("report".into())).unwrap().len(),
            1
        );
        assert_eq!(
            app.view(|s| s.search_files("nope".into())).unwrap().len(),
            0
        );
    }

    #[test]
    fn distinct_uploaders_get_distinct_file_ids() {
        let mut app = TestHost::new(FileShareState::init);

        let id_a = app
            .call_as([1u8; 32], |s| {
                s.upload_file("a.txt".into(), blob_id(), 1, "text/plain".into())
            })
            .unwrap();
        let id_b = app
            .call_as([2u8; 32], |s| {
                s.upload_file("b.txt".into(), blob_id(), 1, "text/plain".into())
            })
            .unwrap();

        // IDs are namespaced by the uploader's identity, so two uploaders never
        // mint a colliding ID even when the converging counter coincides.
        assert_ne!(id_a, id_b);
        assert!(id_a.starts_with(&PublicKey::from([1u8; 32]).to_string()));
        assert!(id_b.starts_with(&PublicKey::from([2u8; 32]).to_string()));
        assert_eq!(app.view(|s| s.list_files()).unwrap().len(), 2);
    }

    #[test]
    fn total_size_saturates_instead_of_overflowing() {
        let mut app = TestHost::new(FileShareState::init);

        app.call(|s| {
            s.upload_file(
                "big".into(),
                blob_id(),
                u64::MAX,
                "application/octet-stream".into(),
            )
        })
        .unwrap();
        app.call(|s| {
            s.upload_file(
                "more".into(),
                blob_id(),
                10,
                "application/octet-stream".into(),
            )
        })
        .unwrap();

        // u64::MAX + 10 saturates to u64::MAX rather than wrapping or panicking.
        assert_eq!(app.view(|s| s.get_total_files_size()).unwrap(), u64::MAX);
    }

    #[test]
    fn upload_survives_announce_failure() {
        let mut app = TestHost::new(FileShareState::init);

        // The harness can now drive the announce-failure branch real WASM hits.
        app.set_blob_announce_should_fail(true);

        app.call(|s| s.upload_file("f.txt".into(), blob_id(), 3, "text/plain".into()))
            .unwrap();

        // A failed announce is logged, not fatal: the file is still recorded.
        assert_eq!(app.view(|s| s.list_files()).unwrap().len(), 1);
        assert!(app.logs().iter().any(|l| l.contains("Failed to announce")));
    }
}
