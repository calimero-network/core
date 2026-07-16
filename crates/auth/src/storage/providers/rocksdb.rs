use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use rocksdb::{IteratorMode, DB};

use crate::config::StorageConfig;
use crate::register_storage_provider;
use crate::storage::registry::StorageProvider;
use crate::storage::{Storage, StorageError};

/// RocksDB storage implementation
pub struct RocksDBStorage {
    db: DB,
}

impl RocksDBStorage {
    /// Create a new RocksDB storage instance
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the RocksDB database
    ///
    /// # Returns
    ///
    /// * `Result<Self, StorageError>` - The new instance
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        // Ensure the directory exists. This directory holds the JWT signing
        // secrets at rest, so on unix it must be created with `0700` (owner-only)
        // permissions rather than the umask default (typically `0755`), which would
        // let any local user traverse it. See finding #6 (auth-secret-at-rest).
        Self::create_db_dir(&path)?;

        let mut options = rocksdb::Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        // Durability and performance options
        options.set_use_fsync(true); // Forces fsync on writes
        options.set_atomic_flush(true); // Ensures atomic flushes across column families
        options.set_manual_wal_flush(false); // Let RocksDB handle WAL flushing
        options.set_keep_log_file_num(10); // Keep more WAL files
        options.set_write_buffer_size(64 * 1024 * 1024); // 64MB write buffer
        options.set_max_write_buffer_number(3);

        // Additional performance tuning
        options.set_bytes_per_sync(1048576); // 1MB
        options.set_wal_bytes_per_sync(524288); // 512KB
        options.set_compaction_readahead_size(2 * 1024 * 1024); // 2MB

        let db = DB::open(&options, &path)
            .map_err(|e| StorageError::StorageError(format!("Failed to open RocksDB: {e}")))?;

        // Tighten permissions on the secret-bearing files RocksDB just created
        // (SST/WAL/MANIFEST/CURRENT/LOG) to `0600`. The `0700` directory above is
        // the enforced boundary; this is defense-in-depth for stray file copies.
        #[cfg(unix)]
        Self::restrict_db_files(&path);

        Ok(Self { db })
    }

    /// Create the RocksDB directory with owner-only (`0700`) permissions on unix.
    ///
    /// On non-unix targets this falls back to a plain `create_dir_all`.
    fn create_db_dir<P: AsRef<Path>>(path: P) -> Result<(), StorageError> {
        let map_err = |e| StorageError::StorageError(format!("Failed to create DB directory: {e}"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&path)
                .map_err(map_err)?;

            // `recursive(true)` only applies the mode to components it *creates*;
            // if the leaf directory already existed it keeps its old mode, so
            // re-assert `0700` explicitly.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .map_err(map_err)?;
        }

        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(&path).map_err(map_err)?;
        }

        Ok(())
    }

    /// Best-effort tightening of every file directly inside the DB directory to
    /// `0600` (owner read/write only). Failures are logged, not fatal: the
    /// `0700` directory already blocks other users from reaching these files.
    #[cfg(unix)]
    fn restrict_db_files<P: AsRef<Path>>(path: P) {
        use std::os::unix::fs::PermissionsExt;

        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!("Failed to enumerate DB directory for chmod: {e}");
                return;
            }
        };

        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_file() {
                if let Err(e) =
                    std::fs::set_permissions(&entry_path, std::fs::Permissions::from_mode(0o600))
                {
                    tracing::warn!("Failed to restrict permissions on {entry_path:?}: {e}");
                }
            }
        }
    }
}

impl Drop for RocksDBStorage {
    fn drop(&mut self) {
        // Ensure all writes are flushed before closing
        let _ = self.db.flush();
    }
}

#[async_trait]
impl Storage for RocksDBStorage {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.db
            .get(key.as_bytes())
            .map_err(|e| StorageError::StorageError(format!("Failed to get key: {e}")))
    }

    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        self.db
            .put(key.as_bytes(), value)
            .map_err(|e| StorageError::StorageError(format!("Failed to set key: {e}")))
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.db
            .delete(key.as_bytes())
            .map_err(|e| StorageError::StorageError(format!("Failed to delete key: {e}")))
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        let key_bytes = key.as_bytes();
        let exists = self.db.key_may_exist(key_bytes)
            && self
                .db
                .get(key_bytes)
                .map_err(|e| {
                    StorageError::StorageError(format!("Failed to check key existence: {e}"))
                })?
                .is_some();

        Ok(exists)
    }

    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let prefix_bytes = prefix.as_bytes();
        let iter = self.db.iterator(IteratorMode::From(
            prefix_bytes,
            rocksdb::Direction::Forward,
        ));

        let mut keys = Vec::new();
        for item in iter {
            let (key, _) = item.map_err(|e| {
                StorageError::StorageError(format!("Failed to iterate over keys: {e}"))
            })?;

            // Convert the key bytes to a string
            let key_str = String::from_utf8_lossy(&key).to_string();

            // Only include keys that start with the prefix
            if key_str.starts_with(prefix) {
                keys.push(key_str);
            } else {
                // Once we've moved past the prefix, we can stop
                break;
            }
        }

        Ok(keys)
    }
}

/// Provider implementation for RocksDB storage
pub struct RocksDBProvider;

impl StorageProvider for RocksDBProvider {
    fn name(&self) -> &str {
        "rocksdb"
    }

    fn supports_config(&self, config: &StorageConfig) -> bool {
        matches!(config, StorageConfig::RocksDB { .. })
    }

    fn create_storage(&self, config: &StorageConfig) -> Result<Arc<dyn Storage>, StorageError> {
        if let StorageConfig::RocksDB { path } = config {
            let storage =
                RocksDBStorage::new(path).map_err(|e| StorageError::StorageError(e.to_string()))?;
            Ok(Arc::new(storage))
        } else {
            Err(StorageError::StorageError(
                "Invalid configuration for RocksDB".to_string(),
            ))
        }
    }
}

// Register the RocksDB provider
register_storage_provider!(RocksDBProvider);

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn test_rocksdb_storage() {
        let temp_dir = tempdir().unwrap();
        let storage = RocksDBStorage::new(temp_dir.path()).unwrap();

        // Test set and get
        storage.set("test_key", b"test_value").await.unwrap();
        let value = storage.get("test_key").await.unwrap();
        assert_eq!(value, Some(b"test_value".to_vec()));

        // Test exists
        assert!(storage.exists("test_key").await.unwrap());
        assert!(!storage.exists("nonexistent_key").await.unwrap());

        // Test delete
        storage.delete("test_key").await.unwrap();
        let value = storage.get("test_key").await.unwrap();
        assert_eq!(value, None);

        // Test list_keys
        storage.set("prefix1:key1", b"value1").await.unwrap();
        storage.set("prefix1:key2", b"value2").await.unwrap();
        storage.set("prefix2:key3", b"value3").await.unwrap();

        let keys = storage.list_keys("prefix1:").await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"prefix1:key1".to_string()));
        assert!(keys.contains(&"prefix1:key2".to_string()));

        // Test batch operations
        let mut batch_values = HashMap::new();
        batch_values.insert("batch1".to_string(), b"value1".to_vec());
        batch_values.insert("batch2".to_string(), b"value2".to_vec());

        // Test set_batch
        storage.set_batch(&batch_values).await.unwrap();

        // Test get_batch
        let keys: Vec<String> = batch_values.keys().cloned().collect();
        let retrieved = storage.get_batch(&keys).await.unwrap();
        assert_eq!(retrieved, batch_values);

        // Test delete_batch
        storage.delete_batch(&keys).await.unwrap();
        let empty = storage.get_batch(&keys).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_error_handling() {
        let temp_dir = tempdir().unwrap();
        let storage = RocksDBStorage::new(temp_dir.path()).unwrap();

        // Test not found cases
        let result = storage.get("nonexistent").await.unwrap();
        assert!(result.is_none());

        // Test empty batch operations
        let empty_batch: HashMap<String, Vec<u8>> = HashMap::new();
        storage.set_batch(&empty_batch).await.unwrap();

        let empty_keys: Vec<String> = Vec::new();
        let result = storage.get_batch(&empty_keys).await.unwrap();
        assert!(result.is_empty());

        storage.delete_batch(&empty_keys).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_rocksdb_dir_created_0700() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("secret-db");

        let _storage = RocksDBStorage::new(&db_path).unwrap();

        let mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "DB directory must be owner-only (0700)");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_rocksdb_files_restricted_0600() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("secret-db");

        let storage = RocksDBStorage::new(&db_path).unwrap();
        storage.set("k", b"v").await.unwrap();
        // Force RocksDB to flush an SST so on-disk files exist.
        storage.db.flush().unwrap();

        // Re-apply the tightening (as `new` does at open time) so we assert it on
        // the full current file set, then verify none are group/other accessible.
        RocksDBStorage::restrict_db_files(&db_path);

        for entry in std::fs::read_dir(&db_path).unwrap().flatten() {
            let p = entry.path();
            if p.is_file() {
                let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "DB file {p:?} must be owner-only (0600)");
            }
        }
    }

    #[tokio::test]
    async fn test_rocksdb_specific_errors() {
        // Test invalid path
        let result = RocksDBStorage::new("/nonexistent/path/that/should/fail");
        assert!(result.is_err());

        // Test that we can reopen a database after closing the first instance
        let temp_dir = tempdir().unwrap();
        {
            let _storage1 = RocksDBStorage::new(temp_dir.path()).unwrap();
            // _storage1 is dropped here, releasing the lock
        }
        // Now we can open it again
        let _storage2 = RocksDBStorage::new(temp_dir.path()).unwrap();
    }
}
