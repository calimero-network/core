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
        // Ensure the directory exists
        std::fs::create_dir_all(&path)
            .map_err(|e| StorageError::StorageError(format!("Failed to create DB directory: {e}")))?;

        let mut options = rocksdb::Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        
        // Durability and performance options
        options.set_use_fsync(true);  // Forces fsync on writes
        options.set_atomic_flush(true);  // Ensures atomic flushes across column families
        options.set_manual_wal_flush(false);  // Let RocksDB handle WAL flushing
        options.set_keep_log_file_num(10);  // Keep more WAL files
        options.set_write_buffer_size(64 * 1024 * 1024);  // 64MB write buffer
        options.set_max_write_buffer_number(3);
        
        // Additional performance tuning
        options.set_bytes_per_sync(1048576); // 1MB
        options.set_wal_bytes_per_sync(524288); // 512KB
        options.set_compaction_readahead_size(2 * 1024 * 1024); // 2MB

        let db = DB::open(&options, path)
            .map_err(|e| StorageError::StorageError(format!("Failed to open RocksDB: {e}")))?;

        Ok(Self { db })
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
        self.db
            .get(key.as_bytes())
            .map(|v| v.is_some())
            .map_err(|e| StorageError::StorageError(format!("Failed to check key existence: {e}")))
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
            let storage = RocksDBStorage::new(path)
                .map_err(|e| StorageError::StorageError(e.to_string()))?;
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

    #[tokio::test]
    async fn test_rocksdb_specific_errors() {
        // Test invalid path
        let result = RocksDBStorage::new("/nonexistent/path/that/should/fail");
        assert!(result.is_err());

        // Test opening an existing database
        let temp_dir = tempdir().unwrap();
        let _storage1 = RocksDBStorage::new(temp_dir.path()).unwrap();
        let _storage2 = RocksDBStorage::new(temp_dir.path()).unwrap();
    }
}
