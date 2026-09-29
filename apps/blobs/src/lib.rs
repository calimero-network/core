//! # Blob API Implementation Example
//!
//! Minimal file sharing app demonstrating Calimero blob storage.
//!
//! Each file record is its uploader's own entry in an `AuthoredMap`: only the
//! uploader can change or delete it, on every node, and the uploader shown is
//! read from the entry's owner stamp, never from the record. A file id names
//! its uploader (`"<account>_<nonce>"`), and every read by id reads only that
//! account's entry, so no member can file a record that answers for someone
//! else's id.
//!
//! See README.md for complete documentation and usage examples.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::abi::AbiType;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env, AccountId, BlobId};
use calimero_storage::collections::{AuthoredMap, Frozen};

// === CONSTANTS ===

/// Bytes per kilobyte
const BYTES_PER_KB: f64 = 1024.0;

/// Bytes per megabyte
const BYTES_PER_MB: f64 = BYTES_PER_KB * 1024.0;

// === DATA STRUCTURES ===

/// Represents a file stored in the system
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, AbiType)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct FileRecord {
    /// Unique file identifier, `"<uploader account hex>_<nonce hex>"`: it names
    /// its uploader, and the nonce keeps two of their devices apart.
    pub id: String,

    /// Human-readable file name (e.g., "document.pdf", "image.png")
    pub name: String,

    /// Blob ID. Serializes to/from a 64-hex string in JSON via the SDK's
    /// `BlobId` newtype, so no per-app encoding helper is needed.
    pub blob_id: BlobId,

    /// File size in bytes
    pub size: u64,

    /// MIME type following RFC 6838 standard
    /// Examples: "application/pdf", "image/png", "text/plain", "video/mp4"
    pub mime_type: String,

    /// The uploader's account, hex-encoded. Every read fills it in from the
    /// entry's owner stamp, so what a writer stored here is never shown.
    pub uploaded_by: String,

    /// Upload timestamp in milliseconds since Unix epoch (January 1, 1970 00:00:00 UTC)
    /// Obtained from `env::time_now()`
    pub uploaded_at: u64,
}

// Atomic whole-record LWW by `uploaded_at` (see `impl_atomic_lww_leaf!`); not a
// struct of CRDT fields, so `#[derive(Mergeable)]` doesn't apply.
calimero_storage::impl_atomic_lww_leaf!(FileRecord, uploaded_at);

/// Application state for the file sharing system.
#[app::state(emits = FileShareEvent)]
pub struct FileShareState {
    /// The account that created the context, hex-encoded. `Frozen`: written
    /// once in `init`, and no node accepts a change afterwards.
    pub owner: Frozen<String>,

    /// File ID -> metadata record, each its uploader's own entry.
    /// Key: `"<uploader account hex>_<nonce hex>"`.
    pub files: AuthoredMap<String, FileRecord>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The account a file id names: the 64 hex characters before its first `_`.
fn named_account(file_id: &str) -> Option<AccountId> {
    let (named, _) = file_id.split_once('_')?;
    named.parse().ok()
}

/// Events emitted by the application
#[app::event]
pub enum FileShareEvent {
    /// Emitted when a file is successfully uploaded
    FileUploaded {
        /// Unique file identifier (e.g. "<uploader-hex>_0")
        id: String,
        /// File name
        name: String,
        /// File size in bytes
        size: u64,
        /// Uploader's account, hex-encoded
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
        // The account, not the device: the owner is a person.
        let owner = AccountId::from(env::account_id()).to_string();

        app::log!("Initializing file sharing app for owner: {}", owner);

        FileShareState {
            owner: Frozen::new(owner),
            files: AuthoredMap::new(),
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
    /// * `blob_id` - Blob ID (64-hex string over the wire; obtained from the blob client)
    /// * `size` - File size in bytes
    /// * `mime_type` - MIME type (e.g., "application/pdf", "image/png")
    ///
    /// # Returns
    /// * `Ok(String)` - The generated file ID, `"<uploader account hex>_<nonce>"`
    /// * `Err(app::Error)` - Error if storage operation fails
    pub fn upload_file(
        &mut self,
        name: String,
        blob_id: BlobId,
        size: u64,
        mime_type: String,
    ) -> app::Result<String> {
        // File IDs must be unique across replicas. A shared counter is not
        // enough: two nodes uploading concurrently both read the same value
        // and would mint the same ID. The uploader's account names whose entry
        // the id is (nobody else's entry at it is ever read), and a random
        // nonce keeps two devices of one account apart.
        let uploader = AccountId::from(env::account_id()).to_string();
        let mut nonce = [0u8; 8];
        env::random_bytes(&mut nonce);
        let file_id = format!("{uploader}_{}", hex(&nonce));

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

    /// Delete a file by its ID. Only its uploader may: the record is their own
    /// entry, and every node refuses anyone else's removal.
    ///
    /// Note: This only removes the file metadata from contract storage.
    /// The actual blob data remains in the blob store, as the SDK does not
    /// currently expose blob deletion methods.
    ///
    /// # Arguments
    /// * `file_id` - The ID of the file to delete
    ///
    /// # Errors
    /// * `Err(app::Error)` - Error if file not found, not the caller's, or
    ///   deletion fails
    pub fn delete_file(&mut self, file_id: String) -> app::Result<()> {
        // Retrieve the file before deleting to get its name for the event
        let file_record = self.get_file(file_id.clone())?;
        if named_account(&file_id) != Some(AccountId::from(env::account_id())) {
            app::bail!("Only its uploader may delete file {file_id}");
        }

        let file_name = file_record.name.clone();

        // Remove the file metadata from storage
        // NOTE: The underlying blob is not deleted from blob storage
        let _ = self.files.remove(&file_id)?;

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
        let files = self.genuine_files()?;

        app::log!("Listed {} files", files.len());

        Ok(files)
    }

    /// Get a specific file by ID
    ///
    /// # Arguments
    /// * `file_id` - The ID of the file to retrieve
    ///
    /// # Returns
    /// * `Ok(FileRecord)` - Complete file record with all metadata
    /// * `Err(app::Error)` - Error if file not found or retrieval fails
    ///
    /// Reads the entry of the account the id names, and no other: keys are per
    /// owner, so a key-only `get` would only ever find the caller's own.
    pub fn get_file(&self, file_id: String) -> app::Result<FileRecord> {
        let Some(uploader) = named_account(&file_id) else {
            app::bail!("File not found: {file_id}");
        };
        let Some(mut file_record) = self.files.get_by(&uploader, &file_id)? else {
            app::bail!("File not found: {file_id}");
        };
        file_record.uploaded_by = uploader.to_string();

        Ok(file_record)
    }

    /// Get blob ID for download (hex-encoded)
    ///
    /// Use this to retrieve the blob ID for downloading the actual file content
    /// via `blobClient.downloadBlob(blob_id, context_id)`.
    ///
    /// # Arguments
    /// * `file_id` - The ID of the file
    ///
    /// # Returns
    /// * `Ok(BlobId)` - The blob ID, a 64-hex string over the wire
    /// * `Err(app::Error)` - Error if file not found
    pub fn get_blob_id_hex(&self, file_id: String) -> app::Result<BlobId> {
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

        for file_record in self.genuine_files()? {
            if file_record.name.to_lowercase().contains(&query_lower) {
                results.push(file_record);
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

        for file_record in self.genuine_files()? {
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
    /// - Owner: a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1
    /// ```
    ///
    /// Note: "Total storage" refers to the sum of all file sizes, not the actual
    /// contract storage usage (which would include metadata overhead).
    pub fn get_stats(&self) -> app::Result<String> {
        let file_count = self.genuine_files()?.len();

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
            self.owner.get()?
        ))
    }
}

impl FileShareState {
    /// Every record whose owner is the account its id names, with
    /// `uploaded_by` read from the owner stamp. A record any other account
    /// holds at an id is a patched peer's, and dropped.
    fn genuine_files(&self) -> app::Result<Vec<FileRecord>> {
        let mut files = Vec::new();
        for (owner, file_id, mut file_record) in self.files.entries_with_owners()? {
            if named_account(&file_id) == Some(owner) {
                file_record.uploaded_by = owner.to_string();
                files.push(file_record);
            }
        }
        Ok(files)
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
            app.view(|s| s.get_blob_id_hex(file_id.clone())).unwrap(),
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

    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    #[test]
    fn distinct_uploaders_get_distinct_file_ids() {
        let mut app = TestHost::new(FileShareState::init);

        let id_a = app
            .call_as_account(ALICE, ALICE, |s| {
                s.upload_file("a.txt".into(), blob_id(), 1, "text/plain".into())
            })
            .unwrap();
        let id_b = app
            .call_as_account(BOB, BOB, |s| {
                s.upload_file("b.txt".into(), blob_id(), 1, "text/plain".into())
            })
            .unwrap();
        // Two devices of one account, uploading as they would concurrently.
        let id_a2 = app
            .call_as_account([0xD2; 32], ALICE, |s| {
                s.upload_file("c.txt".into(), blob_id(), 1, "text/plain".into())
            })
            .unwrap();

        // IDs name the uploader's account, and the nonce keeps one account's
        // devices apart.
        assert_ne!(id_a, id_b);
        assert_ne!(id_a, id_a2);
        assert!(id_a.starts_with(&format!("{}_", AccountId::from(ALICE))));
        assert!(id_b.starts_with(&format!("{}_", AccountId::from(BOB))));
        assert_eq!(app.view(|s| s.list_files()).unwrap().len(), 3);
    }

    #[test]
    fn only_the_uploader_deletes_and_the_stamp_names_them() {
        let mut app = TestHost::new(FileShareState::init);
        let id = app
            .call_as_account(ALICE, ALICE, |s| {
                s.upload_file("a.txt".into(), blob_id(), 1, "text/plain".into())
            })
            .unwrap();

        assert!(app
            .call_as_account(BOB, BOB, |s| s.delete_file(id.clone()))
            .is_err());
        app.set_account(BOB);
        let file = app.view(|s| s.get_file(id.clone())).unwrap();
        assert_eq!(file.uploaded_by, AccountId::from(ALICE).to_string());

        app.call_as_account(ALICE, ALICE, |s| s.delete_file(id.clone()))
            .unwrap();
        assert!(app.view(|s| s.list_files()).unwrap().is_empty());
    }

    /// What a patched peer does: skip `upload_file` and file its own record at
    /// Alice's id, claiming Alice uploaded it. No read sees it.
    #[test]
    fn a_record_at_someone_else_s_id_is_never_read() {
        let mut app = TestHost::new(FileShareState::init);
        let id = app
            .call_as_account(ALICE, ALICE, |s| {
                s.upload_file("real.txt".into(), blob_id(), 1, "text/plain".into())
            })
            .unwrap();
        app.call_as_account(BOB, BOB, |s| {
            s.files.insert(
                id.clone(),
                FileRecord {
                    id: id.clone(),
                    name: "forged.exe".into(),
                    blob_id: BlobId::from([9u8; 32]),
                    size: 1,
                    mime_type: "application/octet-stream".into(),
                    uploaded_by: AccountId::from(ALICE).to_string(),
                    uploaded_at: u64::MAX,
                },
            )
        })
        .unwrap();

        assert_eq!(
            app.view(|s| s.get_file(id.clone())).unwrap().name,
            "real.txt"
        );
        let names: Vec<_> = app
            .view(|s| s.list_files())
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["real.txt"]);
        assert!(app
            .view(|s| s.search_files("forged".into()))
            .unwrap()
            .is_empty());
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
