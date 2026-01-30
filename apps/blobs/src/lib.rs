//! # Blob API Implementation Example
//!
//! Minimal file sharing app demonstrating Calimero blob storage.
//!
//! See README.md for complete documentation and usage examples.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env};
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap};

// === CONSTANTS ===

/// Size of blob ID in bytes (32 bytes = 256 bits)
const BLOB_ID_SIZE: usize = 32;

/// Maximum size of base58-encoded blob ID string
/// Base58 encoding of 32 bytes requires max 44 characters
const BASE58_ENCODED_MAX_SIZE: usize = 44;

/// Bytes per kilobyte
const BYTES_PER_KB: f64 = 1024.0;

/// Bytes per megabyte
const BYTES_PER_MB: f64 = BYTES_PER_KB * 1024.0;

// === BLOB ID ENCODING HELPERS ===

/// Convert blob ID bytes to base58 string
fn encode_blob_id_base58(blob_id_bytes: &[u8; BLOB_ID_SIZE]) -> String {
    let mut buf = [0u8; BASE58_ENCODED_MAX_SIZE];
    let len = bs58::encode(blob_id_bytes).onto(&mut buf[..]).unwrap();
    std::str::from_utf8(&buf[..len]).unwrap().to_owned()
}

/// Parse base58 string to blob ID bytes
fn parse_blob_id_base58(blob_id_str: &str) -> Result<[u8; BLOB_ID_SIZE], String> {
    match bs58::decode(blob_id_str).into_vec() {
        Ok(bytes) => {
            if bytes.len() != BLOB_ID_SIZE {
                return Err(format!(
                    "Invalid blob ID length: expected {} bytes, got {}",
                    BLOB_ID_SIZE,
                    bytes.len()
                ));
            }
            let mut blob_id = [0u8; BLOB_ID_SIZE];
            blob_id.copy_from_slice(&bytes);
            Ok(blob_id)
        }
        Err(e) => Err(format!("Failed to decode blob ID '{blob_id_str}': {e}")),
    }
}

/// Serialize blob ID as base58 string for JSON responses
fn serialize_blob_id_bytes<S>(
    blob_id_bytes: &[u8; BLOB_ID_SIZE],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: calimero_sdk::serde::Serializer,
{
    let safe_string = encode_blob_id_base58(blob_id_bytes);
    serializer.serialize_str(&safe_string)
}

// === DATA STRUCTURES ===

/// Represents a file stored in the system
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct FileRecord {
    /// Unique file identifier (e.g., "file_0", "file_1")
    pub id: String,

    /// Human-readable file name (e.g., "document.pdf", "image.png")
    pub name: String,

    /// Blob ID as 32-byte array
    /// Serialized as base58 string in JSON (e.g., "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty")
    /// Note: Must use literal `32` instead of `BLOB_ID_SIZE` const for ABI generator compatibility
    #[serde(serialize_with = "serialize_blob_id_bytes")]
    pub blob_id: [u8; 32],

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

// Implement Mergeable for FileRecord
//
// **Why is this needed?**
// The auto-generated merge for FileShareState includes:
//   self.files.merge(&other.files)?;
//
// UnorderedMap::merge() requires V: Mergeable, so FileRecord must implement it.
//
// **When is this actually called?**
// - NOT on different files (DAG handles via element IDs)
// - NOT on sequential updates (HLC provides ordering)
// - ONLY on concurrent updates to the SAME file key (rare!)
//
// **What does it do?**
// Simple LWW based on uploaded_at timestamp. For file uploads, this is correct
// since uploads are atomic (you upload the whole file, not partial updates).
//
// **Could we avoid this?**
// Yes! If FileShareState used proper CRDT types for root fields:
//   owner: LwwRegister<String> instead of String
//   file_counter: Counter instead of u64
// Then this Mergeable impl wouldn't be needed at all - DAG would handle everything!
//
// See: crates/storage/DAG_VS_MERGE.md for full explanation
impl calimero_storage::collections::Mergeable for FileRecord {
    fn merge(
        &mut self,
        other: &Self,
    ) -> Result<(), calimero_storage::collections::crdt_meta::MergeError> {
        // Simple LWW: take the version with later uploaded_at timestamp
        // This is correct for file uploads (atomic operations)
        if other.uploaded_at > self.uploaded_at {
            *self = other.clone();
        }
        Ok(())
    }
}

/// Application state for the file sharing system
#[app::state(emits = FileShareEvent)]
#[derive(BorshDeserialize, BorshSerialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct FileShareState {
    /// Context owner's identity as base58-encoded public key
    /// Set during initialization from `env::executor_id()`
    pub owner: LwwRegister<String>,

    /// Map of file ID to file metadata records
    /// Key: file ID (e.g., "file_0"), Value: FileRecord
    pub files: UnorderedMap<String, FileRecord>,

    /// Counter for generating unique file IDs
    /// Incremented on each file upload (CRDT G-Counter for distributed safety)
    pub file_counter: Counter,
}

/// Events emitted by the application
#[app::event]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum FileShareEvent {
    /// Emitted when a file is successfully uploaded
    FileUploaded {
        /// Unique file identifier (e.g., "file_0")
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
        let owner_id = env::executor_id();
        let owner = encode_blob_id_base58(&owner_id);

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
    /// * `blob_id_str` - Base58-encoded blob ID (obtained from blob client)
    /// * `size` - File size in bytes
    /// * `mime_type` - MIME type (e.g., "application/pdf", "image/png")
    ///
    /// # Returns
    /// * `Ok(String)` - The generated file ID (e.g., "file_0", "file_1")
    /// * `Err(String)` - Error message if blob ID is invalid or storage operation fails
    pub fn upload_file(
        &mut self,
        name: String,
        blob_id_str: String,
        size: u64,
        mime_type: String,
    ) -> Result<String, String> {
        let blob_id = parse_blob_id_base58(&blob_id_str)?;

        // Get current counter value for file ID, then increment
        let counter_value = self
            .file_counter
            .value()
            .map_err(|e| format!("Failed to get counter: {e:?}"))?;
        let file_id = format!("file_{}", counter_value);
        self.file_counter
            .increment()
            .map_err(|e| format!("Failed to increment counter: {e:?}"))?;

        let uploader_id = env::executor_id();
        let uploader = encode_blob_id_base58(&uploader_id);
        let timestamp = env::time_now();

        // BLOB API: Announce blob to network for peer discovery
        // This makes the blob discoverable by other nodes in this context
        let current_context = env::context_id();
        if env::blob_announce_to_context(&blob_id, &current_context) {
            app::log!("Announced blob {} to network", blob_id_str);
        } else {
            app::log!("Warning: Failed to announce blob {}", blob_id_str);
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
        self.files
            .insert(file_id.clone(), file_record)
            .map_err(|e| format!("Failed to store file record: {e:?}"))?;

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
    /// * `file_id` - The ID of the file to delete (e.g., "file_0")
    ///
    /// # Returns
    /// * `Ok(())` - File metadata successfully deleted
    /// * `Err(String)` - Error message if file not found or deletion fails
    pub fn delete_file(&mut self, file_id: String) -> Result<(), String> {
        // Retrieve the file before deleting to get its name for the event
        let file_record = self
            .files
            .get(&file_id)
            .map_err(|e| format!("Failed to access file: {e:?}"))?
            .ok_or_else(|| format!("File not found: {file_id}"))?;

        let file_name = file_record.name.clone();

        // Remove the file metadata from storage
        // NOTE: The underlying blob is not deleted from blob storage
        self.files
            .remove(&file_id)
            .map_err(|e| format!("Failed to delete file: {e:?}"))?;

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
    /// * `Err(String)` - Error message if storage operation fails (rarely occurs)
    pub fn list_files(&self) -> Result<Vec<FileRecord>, String> {
        let mut files = Vec::new();

        if let Ok(entries) = self.files.entries() {
            for (_, file_record) in entries {
                files.push(file_record.clone());
            }
        }

        app::log!("Listed {} files", files.len());

        Ok(files)
    }

    /// Get a specific file by ID
    ///
    /// # Arguments
    /// * `file_id` - The ID of the file to retrieve (e.g., "file_0")
    ///
    /// # Returns
    /// * `Ok(FileRecord)` - Complete file record with all metadata
    /// * `Err(String)` - Error message if file not found or retrieval fails
    pub fn get_file(&self, file_id: String) -> Result<FileRecord, String> {
        match self.files.get(&file_id) {
            Ok(Some(file_record)) => Ok(file_record.clone()),
            Ok(None) => Err(format!("File not found: {file_id}")),
            Err(e) => Err(format!("Failed to retrieve file: {e:?}")),
        }
    }

    /// Get blob ID for download (base58-encoded)
    ///
    /// Use this to retrieve the blob ID for downloading the actual file content
    /// via `blobClient.downloadBlob(blob_id, context_id)`.
    ///
    /// # Arguments
    /// * `file_id` - The ID of the file (e.g., "file_0")
    ///
    /// # Returns
    /// * `Ok(String)` - Base58-encoded blob ID (e.g., "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty")
    /// * `Err(String)` - Error message if file not found
    pub fn get_blob_id_b58(&self, file_id: String) -> Result<String, String> {
        let file_record = self.get_file(file_id)?;
        Ok(encode_blob_id_base58(&file_record.blob_id))
    }

    /// Search files by name (case-insensitive substring match)
    ///
    /// # Arguments
    /// * `query` - Search term to match against file names
    ///
    /// # Returns
    /// * `Ok(Vec<FileRecord>)` - Vector of matching file records (not just names), may be empty if no matches
    /// * `Err(String)` - Error message if storage operation fails (rarely occurs)
    pub fn search_files(&self, query: String) -> Result<Vec<FileRecord>, String> {
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        if let Ok(entries) = self.files.entries() {
            for (_, file_record) in entries {
                if file_record.name.to_lowercase().contains(&query_lower) {
                    results.push(file_record.clone());
                }
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
    /// * `Err(String)` - Error message if storage operation fails (rarely occurs)
    pub fn get_total_files_size(&self) -> Result<u64, String> {
        let mut total_size = 0u64;

        if let Ok(entries) = self.files.entries() {
            for (_, file_record) in entries {
                total_size += file_record.size;
            }
        }

        Ok(total_size)
    }

    /// Get file sharing statistics as a formatted string
    ///
    /// # Returns
    /// * `Ok(String)` - Formatted statistics including file count, total file size (not contract storage), and owner
    /// * `Err(String)` - Error message if storage operations fail
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
    pub fn get_stats(&self) -> Result<String, String> {
        let file_count = self
            .files
            .len()
            .map_err(|e| format!("Failed to get file count: {e:?}"))?;

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
