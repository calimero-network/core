use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::StorageConfig;

pub mod models;
pub mod rocksdb;

pub use models::{prefixes, ClientKey, Permission, RootKey};

/// Storage error
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Item not found")]
    NotFound,

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),
}

/// Storage trait
#[async_trait]
pub trait Storage: Send + Sync + 'static {
    /// Get a value from storage
    ///
    /// # Arguments
    ///
    /// * `key` - The key to get
    ///
    /// # Returns
    ///
    /// * `Result<Option<Vec<u8>>, StorageError>` - The value if found
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;

    /// Set a value in storage
    ///
    /// # Arguments
    ///
    /// * `key` - The key to set
    /// * `value` - The value to set
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError>;

    /// Delete a value from storage
    ///
    /// # Arguments
    ///
    /// * `key` - The key to delete
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    async fn delete(&self, key: &str) -> Result<(), StorageError>;

    /// Check if a key exists in storage
    ///
    /// # Arguments
    ///
    /// * `key` - The key to check
    ///
    /// # Returns
    ///
    /// * `Result<bool, StorageError>` - Whether the key exists
    async fn exists(&self, key: &str) -> Result<bool, StorageError>;

    /// List keys with a prefix
    ///
    /// # Arguments
    ///
    /// * `prefix` - The prefix to list keys for
    ///
    /// # Returns
    ///
    /// * `Result<Vec<String>, StorageError>` - The keys
    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, StorageError>;
}

/// Simple in-memory storage implementation for development
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
        let data = self.data.read();
        Ok(data.get(key).cloned())
    }

    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let mut data = self.data.write();
        data.insert(key.to_string(), value.to_vec());
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let mut data = self.data.write();
        if data.remove(key).is_none() {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        let data = self.data.read();
        Ok(data.contains_key(key))
    }

    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let data = self.data.read();
        let keys = data
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        Ok(keys)
    }
}

/// Create a storage instance based on the configuration
///
/// # Arguments
///
/// * `config` - The storage configuration
///
/// # Returns
///
/// * `Result<Arc<dyn Storage>, StorageError>` - The storage instance
pub async fn create_storage(config: &StorageConfig) -> Result<Arc<dyn Storage>, StorageError> {
    match config {
        StorageConfig::RocksDB { path } => {
            let storage = rocksdb::RocksDBStorage::new(path)
                .map_err(|e| StorageError::StorageError(e.to_string()))?;
            Ok(Arc::new(storage))
        }
        #[cfg(feature = "redis-storage")]
        StorageConfig::Redis { url, pool_size } => {
            let storage = redis_storage::RedisStorage::new(url, *pool_size)
                .await
                .map_err(|e| StorageError::StorageError(e.to_string()))?;
            Ok(Arc::new(storage))
        }
        #[cfg(feature = "postgres")]
        StorageConfig::Postgres { url, pool_size } => {
            let storage = postgres_storage::PostgresStorage::new(url, *pool_size)
                .await
                .map_err(|e| StorageError::StorageError(e.to_string()))?;
            Ok(Arc::new(storage))
        }
        #[cfg(feature = "sqlite")]
        StorageConfig::SQLite { path } => {
            let storage = sqlite_storage::SQLiteStorage::new(path)
                .await
                .map_err(|e| StorageError::StorageError(e.to_string()))?;
            Ok(Arc::new(storage))
        }
        StorageConfig::Memory => {
            let storage = MemoryStorage::new();
            Ok(Arc::new(storage))
        }
        #[allow(unreachable_patterns)]
        _ => Err(StorageError::StorageError(
            "Unsupported storage type".to_string(),
        )),
    }
}

/// Helper function to serialize an object to JSON
///
/// # Arguments
///
/// * `value` - The value to serialize
///
/// # Returns
///
/// * `Result<Vec<u8>, StorageError>` - The serialized value
pub fn serialize<T: Serialize>(value: &T) -> Result<Vec<u8>, StorageError> {
    serde_json::to_vec(value).map_err(|e| StorageError::SerializationError(e.to_string()))
}

/// Helper function to deserialize an object from JSON
///
/// # Arguments
///
/// * `data` - The data to deserialize
///
/// # Returns
///
/// * `Result<T, StorageError>` - The deserialized value
pub fn deserialize<T: for<'de> Deserialize<'de>>(data: &[u8]) -> Result<T, StorageError> {
    serde_json::from_slice(data).map_err(|e| StorageError::SerializationError(e.to_string()))
}
