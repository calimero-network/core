use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use super::{Storage, StorageError};

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
    }
}
