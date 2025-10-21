//! # Blob API Implementation Example
//!
//! Minimal file sharing app demonstrating Calimero blob storage.
//!
//! See README.md for complete documentation and usage examples.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::{app, env};
use calimero_storage::collections::UnorderedMap;

// === BLOB ID ENCODING HELPERS ===

/// Convert blob ID bytes to base58 string
fn encode_blob_id_base58(blob_id_bytes: &[u8; 32]) -> String {
    let mut buf = [0u8; 44];
    let len = bs58::encode(blob_id_bytes).onto(&mut buf[..]).unwrap();
    std::str::from_utf8(&buf[..len]).unwrap().to_owned()
}

/// Parse base58 string to blob ID bytes
fn parse_blob_id_base58(blob_id_str: &str) -> Result<[u8; 32], String> {
    match bs58::decode(blob_id_str).into_vec() {
        Ok(bytes) => {
            if bytes.len() != 32 {
                return Err(format!(
                    "Invalid blob ID length: expected 32 bytes, got {}",
                    bytes.len()
                ));
            }
            let mut blob_id = [0u8; 32];
            blob_id.copy_from_slice(&bytes);
            Ok(blob_id)
        }
        Err(e) => Err(format!("Failed to decode blob ID '{}': {}", blob_id_str, e)),
    }
}

/// Serialize blob ID as base58 string for JSON responses
fn serialize_blob_id_bytes<S>(blob_id_bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
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
    pub id: String,
    pub name: String,
    #[serde(serialize_with = "serialize_blob_id_bytes")]
    pub blob_id: [u8; 32],
    pub size: u64,
    pub mime_type: String,
    pub uploaded_by: String,
    pub uploaded_at: u64,
}

/// Application state for the file sharing system
#[app::state(emits = FileShareEvent)]
#[derive(BorshDeserialize, BorshSerialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct FileShareState {
    pub owner: String,
    pub files: UnorderedMap<String, FileRecord>,
    pub file_counter: u64,
}

/// Events emitted by the application
#[app::event]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum FileShareEvent {
    FileUploaded {
        id: String,
        name: String,
        size: u64,
        uploader: String,
    },
    FileDeleted {
        id: String,
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
            owner,
            files: UnorderedMap::new(),
            file_counter: 0,
        }
    }

    /// Upload a file by storing its blob ID and metadata
    ///
    /// The client first uploads the file binary using `blobClient.uploadBlob()` which
    /// returns a blob_id. This method then stores the metadata and announces the blob
    /// to the network so other nodes can discover and download it.
    pub fn upload_file(
        &mut self,
        name: String,
        blob_id_str: String,
        size: u64,
        mime_type: String,
    ) -> Result<String, String> {
        let blob_id = parse_blob_id_base58(&blob_id_str)?;

        let file_id = format!("file_{}", self.file_counter);
        self.file_counter += 1;

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
            .map_err(|e| format!("Failed to store file record: {:?}", e))?;

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
    pub fn delete_file(&mut self, file_id: String) -> Result<(), String> {
        // Retrieve the file before deleting to get its name for the event
        let file_record = self
            .files
            .get(&file_id)
            .map_err(|e| format!("Failed to access file: {:?}", e))?
            .ok_or_else(|| format!("File not found: {}", file_id))?;

        let file_name = file_record.name.clone();

        // Remove the file from storage
        self.files
            .remove(&file_id)
            .map_err(|e| format!("Failed to delete file: {:?}", e))?;

        // Emit event
        app::emit!(FileShareEvent::FileDeleted {
            id: file_id.clone(),
            name: file_name.clone(),
        });

        app::log!("File deleted: {} (ID: {})", file_name, file_id);

        Ok(())
    }

    /// List all files
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
    pub fn get_file(&self, file_id: String) -> Result<FileRecord, String> {
        match self.files.get(&file_id) {
            Ok(Some(file_record)) => Ok(file_record.clone()),
            Ok(None) => Err(format!("File not found: {}", file_id)),
            Err(e) => Err(format!("Failed to retrieve file: {:?}", e)),
        }
    }

    /// Get blob ID for download (base58-encoded)
    pub fn get_blob_id(&self, file_id: String) -> Result<String, String> {
        let file_record = self.get_file(file_id)?;
        Ok(encode_blob_id_base58(&file_record.blob_id))
    }

    /// Search files by name
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

    /// Get total storage usage in bytes
    pub fn get_total_storage(&self) -> Result<u64, String> {
        let mut total_size = 0u64;

        if let Ok(entries) = self.files.entries() {
            for (_, file_record) in entries {
                total_size += file_record.size;
            }
        }

        Ok(total_size)
    }

    /// Get storage statistics
    pub fn get_stats(&self) -> Result<String, String> {
        let file_count = self
            .files
            .len()
            .map_err(|e| format!("Failed to get file count: {:?}", e))?;

        let total_size = self.get_total_storage()?;

        let total_mb = (total_size as f64) / (1024.0 * 1024.0);

        Ok(format!(
            "File Sharing Statistics:\n\
             - Total files: {}\n\
             - Total storage: {:.2} MB ({} bytes)\n\
             - Owner: {}",
            file_count, total_mb, total_size, self.owner
        ))
    }
}
