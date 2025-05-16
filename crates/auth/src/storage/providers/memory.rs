use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

// Import the registry
use crate::config::StorageConfig;
use crate::register_storage_provider;
use crate::storage::models::prefixes;
use crate::storage::registry::StorageProvider;
use crate::storage::{
    deserialize, serialize, ClientKey, KeyStorage, Permission, RootKey, Storage, StorageError,
};

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

#[async_trait]
impl KeyStorage for MemoryStorage {
    async fn get_root_key(&self, key_id: &str) -> Result<Option<RootKey>, StorageError> {
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
        match self.get(&key).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    async fn set_root_key(&self, key_id: &str, root_key: &RootKey) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
        let value = serialize(root_key)?;

        // Store the main key-value
        self.set(&key, &value).await?;

        // Create secondary index for public key lookups
        let public_key_index = format!("{}{}", prefixes::PUBLIC_KEY_INDEX, root_key.public_key);
        self.set(&public_key_index, key_id.as_bytes()).await?;

        Ok(())
    }

    async fn delete_root_key(&self, key_id: &str) -> Result<(), StorageError> {
        if let Some(root_key) = self.get_root_key(key_id).await? {
            // Delete the main key
            let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
            self.delete(&key).await?;

            // Delete the public key index
            let public_key_index = format!("{}{}", prefixes::PUBLIC_KEY_INDEX, root_key.public_key);
            self.delete(&public_key_index).await?;

            // Also delete the root-to-client index
            let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, key_id);
            self.delete(&root_clients_key).await?;

            Ok(())
        } else {
            Err(StorageError::NotFound)
        }
    }

    async fn list_root_keys(&self) -> Result<Vec<(String, RootKey)>, StorageError> {
        let keys = self.list_keys(prefixes::ROOT_KEY).await?;
        let mut result = Vec::with_capacity(keys.len());

        for key in keys {
            if let Some(data) = self.get(&key).await? {
                let key_id = key.trim_start_matches(prefixes::ROOT_KEY).to_string();
                let root_key: RootKey = deserialize(&data)?;
                result.push((key_id, root_key));
            }
        }

        Ok(result)
    }

    async fn find_root_key_by_public_key(
        &self,
        public_key: &str,
    ) -> Result<Option<(String, RootKey)>, StorageError> {
        let public_key_index = format!("{}{}", prefixes::PUBLIC_KEY_INDEX, public_key);

        if let Some(key_id_bytes) = self.get(&public_key_index).await? {
            let key_id = String::from_utf8(key_id_bytes).map_err(|e| {
                StorageError::SerializationError(format!("Invalid UTF-8 in key ID: {}", e))
            })?;

            if let Some(root_key) = self.get_root_key(&key_id).await? {
                return Ok(Some((key_id, root_key)));
            }
        }

        Ok(None)
    }

    async fn get_client_key(&self, client_id: &str) -> Result<Option<ClientKey>, StorageError> {
        let key = format!("{}{}", prefixes::CLIENT_KEY, client_id);
        match self.get(&key).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    async fn set_client_key(
        &self,
        client_id: &str,
        client_key: &ClientKey,
    ) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::CLIENT_KEY, client_id);
        let value = serialize(client_key)?;

        // Store the client key
        self.set(&key, &value).await?;

        // Also store a secondary index from root key to client key
        let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, client_key.root_key_id);

        // Check if the index already exists
        let index_value = match self.get(&root_clients_key).await? {
            Some(data) => {
                let mut client_ids: Vec<String> = deserialize(&data)?;
                // Add the client ID if it doesn't already exist
                if !client_ids.contains(&client_id.to_string()) {
                    client_ids.push(client_id.to_string());
                }
                serialize(&client_ids)?
            }
            None => {
                let client_ids = vec![client_id.to_string()];
                serialize(&client_ids)?
            }
        };

        // Store the index
        self.set(&root_clients_key, &index_value).await
    }

    async fn delete_client_key(&self, client_id: &str) -> Result<(), StorageError> {
        // First get the client key to find its root key ID
        if let Some(client_key) = self.get_client_key(client_id).await? {
            // Delete the client key
            let key = format!("{}{}", prefixes::CLIENT_KEY, client_id);
            self.delete(&key).await?;

            // Update the root key to client index
            let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, client_key.root_key_id);

            if let Some(data) = self.get(&root_clients_key).await? {
                let mut client_ids: Vec<String> = deserialize(&data)?;
                client_ids.retain(|id| id != client_id);

                if client_ids.is_empty() {
                    // If no more clients, delete the index
                    self.delete(&root_clients_key).await?;
                } else {
                    // Otherwise update it
                    let value = serialize(&client_ids)?;
                    self.set(&root_clients_key, &value).await?;
                }
            }

            Ok(())
        } else {
            Err(StorageError::NotFound)
        }
    }

    async fn list_client_keys_for_root(
        &self,
        root_key_id: &str,
    ) -> Result<Vec<ClientKey>, StorageError> {
        let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, root_key_id);

        match self.get(&root_clients_key).await? {
            Some(data) => {
                let client_ids: Vec<String> = deserialize(&data)?;
                let mut result = Vec::with_capacity(client_ids.len());

                for client_id in client_ids {
                    if let Some(client_key) = self.get_client_key(&client_id).await? {
                        result.push(client_key);
                    }
                }

                Ok(result)
            }
            None => Ok(Vec::new()),
        }
    }

    async fn get_permission(
        &self,
        permission_id: &str,
    ) -> Result<Option<Permission>, StorageError> {
        let key = format!("{}{}", prefixes::PERMISSION, permission_id);
        match self.get(&key).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    async fn set_permission(
        &self,
        permission_id: &str,
        permission: &Permission,
    ) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::PERMISSION, permission_id);
        let value = serialize(permission)?;
        self.set(&key, &value).await
    }

    async fn delete_permission(&self, permission_id: &str) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::PERMISSION, permission_id);
        self.delete(&key).await
    }

    async fn list_permissions(&self) -> Result<Vec<Permission>, StorageError> {
        let keys = self.list_keys(prefixes::PERMISSION).await?;
        let mut result = Vec::with_capacity(keys.len());

        for key in keys {
            if let Some(data) = self.get(&key).await? {
                let permission: Permission = deserialize(&data)?;
                result.push(permission);
            }
        }

        Ok(result)
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

    fn create_storage(&self, config: &StorageConfig) -> Result<Arc<dyn KeyStorage>, StorageError> {
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
    }

    #[tokio::test]
    async fn test_key_storage_operations() {
        let storage = MemoryStorage::new();

        // Test root key operations
        let root_key_id = "test-root";
        let root_key = RootKey::new("pk12345".to_string(), "near".to_string());

        // Set root key
        storage.set_root_key(root_key_id, &root_key).await.unwrap();

        // Get root key
        let retrieved = storage.get_root_key(root_key_id).await.unwrap().unwrap();
        assert_eq!(retrieved.public_key, root_key.public_key);

        // Find by public key
        let (found_id, found_key) = storage
            .find_root_key_by_public_key(&root_key.public_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found_id, root_key_id);
        assert_eq!(found_key.public_key, root_key.public_key);

        // List root keys
        let keys = storage.list_root_keys().await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, root_key_id);

        // Test permission operations
        let perm_id = "test-perm";
        let permission = Permission::new(
            perm_id.to_string(),
            "Test Permission".to_string(),
            "A test permission".to_string(),
            "test".to_string(),
        );

        storage.set_permission(perm_id, &permission).await.unwrap();

        let retrieved_perm = storage.get_permission(perm_id).await.unwrap().unwrap();
        assert_eq!(retrieved_perm.permission_id, permission.permission_id);

        let perms = storage.list_permissions().await.unwrap();
        assert_eq!(perms.len(), 1);

        // Test client key operations
        let client_id = "test-client";
        let client_key = ClientKey::new(
            client_id.to_string(),
            root_key_id.to_string(),
            "Test Client".to_string(),
            vec![perm_id.to_string()],
            None,
        );

        storage
            .set_client_key(client_id, &client_key)
            .await
            .unwrap();

        let retrieved_client = storage.get_client_key(client_id).await.unwrap().unwrap();
        assert_eq!(retrieved_client.client_id, client_key.client_id);

        let clients = storage
            .list_client_keys_for_root(root_key_id)
            .await
            .unwrap();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].client_id, client_id);

        // Test delete operations
        storage.delete_client_key(client_id).await.unwrap();
        assert!(storage.get_client_key(client_id).await.unwrap().is_none());

        storage.delete_permission(perm_id).await.unwrap();
        assert!(storage.get_permission(perm_id).await.unwrap().is_none());

        storage.delete_root_key(root_key_id).await.unwrap();
        assert!(storage.get_root_key(root_key_id).await.unwrap().is_none());
        assert!(storage
            .find_root_key_by_public_key(&root_key.public_key)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_multiple_clients_per_root() {
        let storage = MemoryStorage::new();

        // Create a root key
        let root_key_id = "multi-client-root";
        let root_key = RootKey::new("pk-multi".to_string(), "near".to_string());
        storage.set_root_key(root_key_id, &root_key).await.unwrap();

        // Create multiple client keys
        for i in 1..=5 {
            let client_id = format!("client{}", i);
            let client_key = ClientKey::new(
                client_id.clone(),
                root_key_id.to_string(),
                format!("Client {}", i),
                vec![],
                None,
            );

            storage
                .set_client_key(&client_id, &client_key)
                .await
                .unwrap();
        }

        // Verify we can list all clients
        let clients = storage
            .list_client_keys_for_root(root_key_id)
            .await
            .unwrap();
        assert_eq!(clients.len(), 5);

        // Delete a couple of clients
        storage.delete_client_key("client1").await.unwrap();
        storage.delete_client_key("client3").await.unwrap();

        // Verify the remaining clients
        let clients = storage
            .list_client_keys_for_root(root_key_id)
            .await
            .unwrap();
        assert_eq!(clients.len(), 3);

        // Collect client IDs for verification
        let client_ids: Vec<String> = clients.iter().map(|c| c.client_id.clone()).collect();
        assert!(client_ids.contains(&"client2".to_string()));
        assert!(client_ids.contains(&"client4".to_string()));
        assert!(client_ids.contains(&"client5".to_string()));

        // Delete the root key and ensure the list is empty
        storage.delete_root_key(root_key_id).await.unwrap();
        let clients_after_delete = storage
            .list_client_keys_for_root(root_key_id)
            .await
            .unwrap();
        assert_eq!(clients_after_delete.len(), 0);
    }

    #[tokio::test]
    async fn test_error_handling() {
        let storage = MemoryStorage::new();

        // Test not found error
        let result = storage.delete_root_key("nonexistent").await;
        assert!(matches!(result, Err(StorageError::NotFound)));

        // Test secondary indices
        let root_key = RootKey::new("pk-err-test".to_string(), "near".to_string());
        storage.set_root_key("err-test", &root_key).await.unwrap();

        let result = storage
            .find_root_key_by_public_key("nonexistent-pk")
            .await
            .unwrap();
        assert!(result.is_none());
    }
}
