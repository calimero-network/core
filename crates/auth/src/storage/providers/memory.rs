use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::config::StorageConfig;
use crate::register_storage_provider;
use crate::storage::registry::StorageProvider;
use crate::storage::{Storage, StorageError};

/// In-memory storage implementation
///
/// This implementation stores all data in memory and is primarily intended for testing
/// and development purposes. Data is lost when the process exits.
pub struct MemoryStorage {
    data: RwLock<HashMap<String, Vec<u8>>>,
}

impl MemoryStorage {
    /// Create a new memory storage
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl Storage for MemoryStorage {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.data.read().get(key).cloned())
    }

    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        self.data.write().insert(key.to_string(), value.to_vec());
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.data.write().remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        Ok(self.data.read().contains_key(key))
    }

    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        Ok(self
            .data
            .read()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }

    async fn get_batch(&self, keys: &[String]) -> Result<HashMap<String, Vec<u8>>, StorageError> {
        let data = self.data.read();
        let mut result = HashMap::new();
        for key in keys {
            if let Some(value) = data.get(key) {
                result.insert(key.clone(), value.clone());
            }
        }
        Ok(result)
    }

    async fn set_batch(&self, values: &HashMap<String, Vec<u8>>) -> Result<(), StorageError> {
        let mut data = self.data.write();
        for (key, value) in values {
            data.insert(key.clone(), value.clone());
        }
        Ok(())
    }

    async fn delete_batch(&self, keys: &[String]) -> Result<(), StorageError> {
        let mut data = self.data.write();
        for key in keys {
            data.remove(key);
        }
        Ok(())
    }
}

/// Provider implementation for in-memory storage
pub struct MemoryStorageProvider;

impl StorageProvider for MemoryStorageProvider {
    fn name(&self) -> &str {
        "memory"
    }

    fn supports_config(&self, config: &StorageConfig) -> bool {
        matches!(config, StorageConfig::Memory)
    }

    fn create_storage(&self, config: &StorageConfig) -> Result<Arc<dyn Storage>, StorageError> {
        if matches!(config, StorageConfig::Memory) {
            let storage = MemoryStorage::new();
            Ok(Arc::new(storage))
        } else {
            Err(StorageError::StorageError(
                "Invalid configuration for Memory storage".to_string(),
            ))
        }
    }
}

// Register the Memory storage provider
register_storage_provider!(MemoryStorageProvider);

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_storage() {
        let storage = MemoryStorage::new();

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
        let storage = MemoryStorage::new();

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
}
