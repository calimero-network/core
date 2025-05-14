use std::path::Path;

use async_trait::async_trait;
use rocksdb::{IteratorMode, DB};

use super::{Storage, StorageError};

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
        let options = rocksdb::Options::default();
        let db = DB::open_default(path)
            .map_err(|e| StorageError::StorageError(format!("Failed to open RocksDB: {e}")))?;

        Ok(Self { db })
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
        let iter = self.db.iterator(IteratorMode::From(prefix_bytes, rocksdb::Direction::Forward));

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn setup_db() -> (RocksDBStorage, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = RocksDBStorage::new(dir.path()).unwrap();
        (db, dir)
    }

    #[tokio::test]
    async fn test_get_set() {
        let (db, _dir) = setup_db().await;
        let key = "test-key";
        let value = b"test-value";

        // Initially, the key shouldn't exist
        let result = db.get(key).await.unwrap();
        assert!(result.is_none());

        // Set the key
        db.set(key, value).await.unwrap();

        // Now the key should exist
        let result = db.get(key).await.unwrap();
        assert_eq!(result, Some(value.to_vec()));
    }

    #[tokio::test]
    async fn test_delete() {
        let (db, _dir) = setup_db().await;
        let key = "test-key";
        let value = b"test-value";

        // Set the key
        db.set(key, value).await.unwrap();

        // Delete the key
        db.delete(key).await.unwrap();

        // Key should no longer exist
        let result = db.get(key).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_exists() {
        let (db, _dir) = setup_db().await;
        let key = "test-key";
        let value = b"test-value";

        // Initially, the key shouldn't exist
        let result = db.exists(key).await.unwrap();
        assert!(!result);

        // Set the key
        db.set(key, value).await.unwrap();

        // Now the key should exist
        let result = db.exists(key).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_list_keys() {
        let (db, _dir) = setup_db().await;
        let prefix = "test-prefix:";
        let keys = [
            format!("{prefix}1"),
            format!("{prefix}2"),
            format!("{prefix}3"),
        ];
        let other_keys = ["other-key1", "other-key2"];

        // Set all the keys
        for key in &keys {
            db.set(key, b"value").await.unwrap();
        }
        for key in &other_keys {
            db.set(key, b"value").await.unwrap();
        }

        // List keys with the prefix
        let result = db.list_keys(prefix).await.unwrap();
        assert_eq!(result.len(), 3);
        for key in &keys {
            assert!(result.contains(key));
        }
        for key in &other_keys {
            assert!(!result.contains(&key.to_string()));
        }
    }
} 