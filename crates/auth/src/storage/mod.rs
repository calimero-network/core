use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::StorageConfig;

pub mod key_manager;
pub mod models;
pub mod providers;
pub mod registry;

// Re-export storage implementations and key manager for backward compatibility
pub use key_manager::KeyManager;
pub use models::{prefixes, Permission, Key};
pub use providers::memory::MemoryStorage;

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

    /// Get multiple values from storage
    ///
    /// # Arguments
    ///
    /// * `keys` - The keys to get
    ///
    /// # Returns
    ///
    /// * `Result<HashMap<String, Vec<u8>>, StorageError>` - The values for keys that exist
    async fn get_batch(&self, keys: &[String]) -> Result<HashMap<String, Vec<u8>>, StorageError> {
        // Default implementation uses single get operations
        let mut result = HashMap::new();
        for key in keys {
            if let Some(value) = self.get(key).await? {
                result.insert(key.clone(), value);
            }
        }
        Ok(result)
    }

    /// Set multiple values in storage
    ///
    /// # Arguments
    ///
    /// * `values` - The key-value pairs to set
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    async fn set_batch(&self, values: &HashMap<String, Vec<u8>>) -> Result<(), StorageError> {
        // Default implementation uses single set operations
        for (key, value) in values {
            self.set(key, value).await?;
        }
        Ok(())
    }

    /// Delete multiple values from storage
    ///
    /// # Arguments
    ///
    /// * `keys` - The keys to delete
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    async fn delete_batch(&self, keys: &[String]) -> Result<(), StorageError> {
        // Default implementation uses single delete operations
        for key in keys {
            // Ignore "not found" errors
            let _ = self.delete(key).await;
        }
        Ok(())
    }

    /// Create a secondary index
    ///
    /// # Arguments
    ///
    /// * `index_name` - The name of the index
    /// * `key` - The primary key
    /// * `index_key` - The index key
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    async fn create_index(
        &self,
        index_name: &str,
        key: &str,
        index_key: &str,
    ) -> Result<(), StorageError> {
        // Store a reference from the index key to the primary key
        let index_storage_key = format!("index:{}:{}:{}", index_name, index_key, key);
        self.set(&index_storage_key, &[]).await
    }

    /// Find keys by a secondary index
    ///
    /// # Arguments
    ///
    /// * `index_name` - The name of the index
    /// * `index_key` - The index key
    ///
    /// # Returns
    ///
    /// * `Result<Vec<String>, StorageError>` - The primary keys found
    async fn find_by_index(
        &self,
        index_name: &str,
        index_key: &str,
    ) -> Result<Vec<String>, StorageError> {
        let index_prefix = format!("index:{}:{}", index_name, index_key);
        let keys = self.list_keys(&index_prefix).await?;

        // Extract primary keys from index keys
        let primary_keys = keys
            .iter()
            .filter_map(|index_key| {
                let parts: Vec<&str> = index_key.split(':').collect();
                if parts.len() >= 4 {
                    Some(parts[3].to_string())
                } else {
                    None
                }
            })
            .collect();

        Ok(primary_keys)
    }

    /// Delete an index
    ///
    /// # Arguments
    ///
    /// * `index_name` - The name of the index
    /// * `key` - The primary key
    /// * `index_key` - The index key
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    async fn delete_index(
        &self,
        index_name: &str,
        key: &str,
        index_key: &str,
    ) -> Result<(), StorageError> {
        let index_storage_key = format!("index:{}:{}:{}", index_name, index_key, key);
        self.delete(&index_storage_key).await
    }

    /// Get storage health status
    ///
    /// # Returns
    ///
    /// * `Result<serde_json::Value, StorageError>` - Health information
    async fn health_check(&self) -> Result<serde_json::Value, StorageError> {
        // Implement a basic health check by default
        let health_key = "_health_check_key";
        let health_value = b"ok";

        // Try to write and read a value
        self.set(health_key, health_value).await?;
        let read_result = self.get(health_key).await?;
        let read_ok = read_result.is_some() && read_result.unwrap() == health_value;

        // Clean up
        let _ = self.delete(health_key).await;

        Ok(serde_json::json!({
            "status": if read_ok { "healthy" } else { "unhealthy" },
            "read_write_test": read_ok,
        }))
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
    // Get all registered providers
    let providers = registry::get_all_providers();

    // Find a provider that supports this configuration
    for provider in providers {
        if provider.supports_config(config) {
            let storage = provider.create_storage(config)?;
            return Ok(storage);
        }
    }

    // If no registered provider is found, return an error
    Err(StorageError::StorageError(format!(
        "No registered storage provider found for configuration: {:?}",
        config
    )))
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
