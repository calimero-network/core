use std::sync::Arc;

use crate::storage::models::{prefixes, Key, KeyType, Permission};
use crate::storage::{deserialize, serialize, Storage, StorageError};

/// KeyManager handles all domain-specific key management operations
/// using an underlying storage implementation
#[derive(Clone)]
pub struct KeyManager {
    storage: Arc<dyn Storage>,
}

impl KeyManager {
    /// Create a new KeyManager with the given storage backend
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }

    /// Get a key by ID
    pub async fn get_key(&self, key_id: &str) -> Result<Option<Key>, StorageError> {
        // Try root key prefix first
        let root_key = format!("{}{}", prefixes::ROOT_KEY, key_id);
        if let Some(data) = self.storage.get(&root_key).await? {
            let key: Key = deserialize(&data)?;
            if key.is_valid() {
                return Ok(Some(key));
            }
            return Ok(None);
        }

        // Try client key prefix
        let client_key = format!("{}{}", prefixes::CLIENT_KEY, key_id);
        if let Some(data) = self.storage.get(&client_key).await? {
            let key: Key = deserialize(&data)?;
            if key.is_valid() {
                return Ok(Some(key));
            }
            return Ok(None);
        }

        Ok(None)
    }

    /// Set a key with appropriate indexing based on key type
    pub async fn set_key(&self, key_id: &str, key: &Key) -> Result<(), StorageError> {
        let value = serialize(key)?;

        match key.key_type {
            KeyType::Root => {
                // Store the main key-value
                let key_path = format!("{}{}", prefixes::ROOT_KEY, key_id);
                self.storage.set(&key_path, &value).await?;

                // Create secondary index for public key lookups
                if let Some(public_key) = &key.public_key {
                    let public_key_index = format!("{}{}", prefixes::PUBLIC_KEY_INDEX, public_key);
                    self.storage.set(&public_key_index, key_id.as_bytes()).await?;
                }
            }
            KeyType::Client => {
                // Store the main key-value
                let key_path = format!("{}{}", prefixes::CLIENT_KEY, key_id);
                self.storage.set(&key_path, &value).await?;

                // Update the root-to-client index
                if let Some(root_key_id) = &key.root_key_id {
                    let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, root_key_id);
                    let mut client_ids = match self.storage.get(&root_clients_key).await? {
                        Some(data) => deserialize(&data)?,
                        None => Vec::new(),
                    };

                    if !client_ids.contains(&key_id.to_string()) {
                        client_ids.push(key_id.to_string());
                        self.storage
                            .set(&root_clients_key, &serialize(&client_ids)?)
                            .await?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Delete a key and its indices
    pub async fn delete_key(&self, key_id: &str) -> Result<(), StorageError> {
        if let Some(key) = self.get_key(key_id).await? {
            match key.key_type {
                KeyType::Root => {
                    // Delete the main key
                    let key_path = format!("{}{}", prefixes::ROOT_KEY, key_id);
                    self.storage.delete(&key_path).await?;

                    // Delete the public key index
                    if let Some(public_key) = key.public_key {
                        let public_key_index = format!("{}{}", prefixes::PUBLIC_KEY_INDEX, public_key);
                        self.storage.delete(&public_key_index).await?;
                    }

                    // Delete the root-to-client index
                    let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, key_id);
                    self.storage.delete(&root_clients_key).await?;
                }
                KeyType::Client => {
                    // Delete the main key
                    let key_path = format!("{}{}", prefixes::CLIENT_KEY, key_id);
                    self.storage.delete(&key_path).await?;

                    // Update the root-to-client index
                    if let Some(root_key_id) = key.root_key_id {
                        let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, root_key_id);
                        if let Some(data) = self.storage.get(&root_clients_key).await? {
                            let mut client_ids: Vec<String> = deserialize(&data)?;
                            client_ids.retain(|id| id != key_id);

                            if client_ids.is_empty() {
                                self.storage.delete(&root_clients_key).await?;
                            } else {
                                self.storage
                                    .set(&root_clients_key, &serialize(&client_ids)?)
                                    .await?;
                            }
                        }
                    }
                }
            }
            Ok(())
        } else {
            Err(StorageError::NotFound)
        }
    }

    /// List all keys of a specific type
    pub async fn list_keys(&self, key_type: KeyType) -> Result<Vec<(String, Key)>, StorageError> {
        let prefix = match key_type {
            KeyType::Root => prefixes::ROOT_KEY,
            KeyType::Client => prefixes::CLIENT_KEY,
        };

        let keys = self.storage.list_keys(prefix).await?;
        let mut result = Vec::with_capacity(keys.len());

        for key in keys {
            if let Some(data) = self.storage.get(&key).await? {
                let key_data: Key = deserialize(&data)?;
                if key_data.is_valid() {
                    let key_id = key.trim_start_matches(prefix).to_string();
                    result.push((key_id, key_data));
                }
            }
        }

        Ok(result)
    }

    /// Find a root key by its public key
    pub async fn find_root_key_by_public_key(
        &self,
        public_key: &str,
    ) -> Result<Option<(String, Key)>, StorageError> {
        let public_key_index = format!("{}{}", prefixes::PUBLIC_KEY_INDEX, public_key);

        if let Some(key_id_bytes) = self.storage.get(&public_key_index).await? {
            let key_id = String::from_utf8(key_id_bytes).map_err(|e| {
                StorageError::SerializationError(format!("Invalid UTF-8 in key ID: {}", e))
            })?;

            if let Some(key) = self.get_key(&key_id).await? {
                if key.is_valid() && key.is_root_key() {
                    return Ok(Some((key_id, key)));
                }
            }
        }

        Ok(None)
    }

    /// List client keys for a root key
    pub async fn list_client_keys_for_root(
        &self,
        root_key_id: &str,
    ) -> Result<Vec<Key>, StorageError> {
        let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, root_key_id);

        match self.storage.get(&root_clients_key).await? {
            Some(data) => {
                let client_ids: Vec<String> = deserialize(&data)?;
                let mut result = Vec::with_capacity(client_ids.len());

                for client_id in client_ids {
                    if let Some(key) = self.get_key(&client_id).await? {
                        if key.is_valid() && key.is_client_key() {
                            result.push(key);
                        }
                    }
                }

                Ok(result)
            }
            None => Ok(Vec::new()),
        }
    }

    /// Get a permission by ID
    pub async fn get_permission(
        &self,
        permission_id: &str,
    ) -> Result<Option<Permission>, StorageError> {
        let key = format!("{}{}", prefixes::PERMISSION, permission_id);
        match self.storage.get(&key).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    /// Set a permission
    pub async fn set_permission(
        &self,
        permission_id: &str,
        permission: &Permission,
    ) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::PERMISSION, permission_id);
        let value = serialize(permission)?;
        self.storage.set(&key, &value).await
    }

    /// Delete a permission
    pub async fn delete_permission(&self, permission_id: &str) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::PERMISSION, permission_id);
        self.storage.delete(&key).await
    }

    /// List all permissions
    pub async fn list_permissions(&self) -> Result<Vec<Permission>, StorageError> {
        let keys = self.storage.list_keys(prefixes::PERMISSION).await?;
        let mut result = Vec::with_capacity(keys.len());

        for key in keys {
            if let Some(data) = self.storage.get(&key).await? {
                let permission: Permission = deserialize(&data)?;
                result.push(permission);
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::providers::memory::MemoryStorage;

    #[tokio::test]
    async fn test_key_operations() {
        let storage = Arc::new(MemoryStorage::new());
        let key_manager = KeyManager::new(storage);

        // Test root key operations
        let root_key = Key::new_root_key(
            "test_pub_key".to_string(),
            "near".to_string(),
        );
        
        // Test set and get
        key_manager.set_key("test_key", &root_key).await.unwrap();
        let retrieved = key_manager.get_key("test_key").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert!(retrieved.is_root_key());
        assert_eq!(retrieved.get_public_key(), Some("test_pub_key"));
        assert_eq!(retrieved.get_auth_method(), Some("near"));

        // Test find by public key
        let found = key_manager
            .find_root_key_by_public_key("test_pub_key")
            .await
            .unwrap();
        assert!(found.is_some());
        let (found_id, found_key) = found.unwrap();
        assert_eq!(found_id, "test_key");
        assert_eq!(found_key.get_public_key(), Some("test_pub_key"));

        // Test client key operations
        let client_key = Key::new_client_key(
            "test_key".to_string(),
            "Test Client".to_string(),
            vec!["test_perm".to_string()],
        );

        // Test set and get
        key_manager.set_key("test_client", &client_key).await.unwrap();
        let retrieved = key_manager.get_key("test_client").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert!(retrieved.is_client_key());
        assert_eq!(retrieved.get_root_key_id(), Some("test_key"));
        assert_eq!(retrieved.get_name(), Some("Test Client"));

        // Test list client keys for root
        let clients = key_manager.list_client_keys_for_root("test_key").await.unwrap();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].get_root_key_id(), Some("test_key"));

        // Test delete
        key_manager.delete_key("test_client").await.unwrap();
        assert!(key_manager.get_key("test_client").await.unwrap().is_none());
        assert_eq!(
            key_manager
                .list_client_keys_for_root("test_key")
                .await
                .unwrap()
                .len(),
            0
        );

        key_manager.delete_key("test_key").await.unwrap();
        assert!(key_manager.get_key("test_key").await.unwrap().is_none());
        assert!(key_manager
            .find_root_key_by_public_key("test_pub_key")
            .await
            .unwrap()
            .is_none());
    }
}
