use std::sync::Arc;

use crate::storage::{
    models::{prefixes, ClientKey, Permission, RootKey},
    deserialize, serialize, Storage, StorageError,
};

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

    /// Get a root key by ID
    pub async fn get_root_key(&self, key_id: &str) -> Result<Option<RootKey>, StorageError> {
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
        match self.storage.get(&key).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    /// Set a root key with public key indexing
    pub async fn set_root_key(&self, key_id: &str, root_key: &RootKey) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
        let value = serialize(root_key)?;

        // Store the main key-value
        self.storage.set(&key, &value).await?;

        // Create secondary index for public key lookups
        let public_key_index = format!("{}{}", prefixes::PUBLIC_KEY_INDEX, root_key.public_key);
        self.storage.set(&public_key_index, key_id.as_bytes()).await?;

        Ok(())
    }

    /// Delete a root key and its indices
    pub async fn delete_root_key(&self, key_id: &str) -> Result<(), StorageError> {
        if let Some(root_key) = self.get_root_key(key_id).await? {
            // Delete the main key
            let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
            self.storage.delete(&key).await?;

            // Delete the public key index
            let public_key_index = format!("{}{}", prefixes::PUBLIC_KEY_INDEX, root_key.public_key);
            self.storage.delete(&public_key_index).await?;

            // Delete the root-to-client index
            let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, key_id);
            self.storage.delete(&root_clients_key).await?;

            Ok(())
        } else {
            Err(StorageError::NotFound)
        }
    }

    /// List all root keys
    pub async fn list_root_keys(&self) -> Result<Vec<(String, RootKey)>, StorageError> {
        let keys = self.storage.list_keys(prefixes::ROOT_KEY).await?;
        let mut result = Vec::with_capacity(keys.len());

        for key in keys {
            if let Some(data) = self.storage.get(&key).await? {
                let key_id = key.trim_start_matches(prefixes::ROOT_KEY).to_string();
                let root_key: RootKey = deserialize(&data)?;
                result.push((key_id, root_key));
            }
        }

        Ok(result)
    }

    /// Find a root key by its public key
    pub async fn find_root_key_by_public_key(
        &self,
        public_key: &str,
    ) -> Result<Option<(String, RootKey)>, StorageError> {
        let public_key_index = format!("{}{}", prefixes::PUBLIC_KEY_INDEX, public_key);

        if let Some(key_id_bytes) = self.storage.get(&public_key_index).await? {
            let key_id = String::from_utf8(key_id_bytes).map_err(|e| {
                StorageError::SerializationError(format!("Invalid UTF-8 in key ID: {}", e))
            })?;

            if let Some(root_key) = self.get_root_key(&key_id).await? {
                return Ok(Some((key_id, root_key)));
            }
        }

        Ok(None)
    }

    /// Get a client key by ID
    pub async fn get_client_key(&self, client_id: &str) -> Result<Option<ClientKey>, StorageError> {
        let key = format!("{}{}", prefixes::CLIENT_KEY, client_id);
        match self.storage.get(&key).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    /// Set a client key with root key indexing
    pub async fn set_client_key(
        &self,
        client_id: &str,
        client_key: &ClientKey,
    ) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::CLIENT_KEY, client_id);
        let value = serialize(client_key)?;

        // Store the main key-value
        self.storage.set(&key, &value).await?;

        // Update the root-to-client index
        let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, client_key.root_key_id);
        let mut client_ids = match self.storage.get(&root_clients_key).await? {
            Some(data) => deserialize(&data)?,
            None => Vec::new(),
        };

        if !client_ids.contains(&client_id.to_string()) {
            client_ids.push(client_id.to_string());
            self.storage.set(&root_clients_key, &serialize(&client_ids)?).await?;
        }

        Ok(())
    }

    /// Delete a client key and update indices
    pub async fn delete_client_key(&self, client_id: &str) -> Result<(), StorageError> {
        if let Some(client_key) = self.get_client_key(client_id).await? {
            // Delete the main key
            let key = format!("{}{}", prefixes::CLIENT_KEY, client_id);
            self.storage.delete(&key).await?;

            // Update the root-to-client index
            let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, client_key.root_key_id);
            if let Some(data) = self.storage.get(&root_clients_key).await? {
                let mut client_ids: Vec<String> = deserialize(&data)?;
                client_ids.retain(|id| id != client_id);
                
                if client_ids.is_empty() {
                    self.storage.delete(&root_clients_key).await?;
                } else {
                    self.storage.set(&root_clients_key, &serialize(&client_ids)?).await?;
                }
            }

            Ok(())
        } else {
            Err(StorageError::NotFound)
        }
    }

    /// List client keys for a root key
    pub async fn list_client_keys_for_root(
        &self,
        root_key_id: &str,
    ) -> Result<Vec<ClientKey>, StorageError> {
        let root_clients_key = format!("{}{}", prefixes::ROOT_CLIENTS, root_key_id);

        match self.storage.get(&root_clients_key).await? {
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
    async fn test_root_key_operations() {
        let storage = Arc::new(MemoryStorage::new());
        let key_manager = KeyManager::new(storage);

        // Create a test root key
        let root_key = RootKey {
            public_key: "test_pub_key".to_string(),
            auth_method: "near".to_string(),
            created_at: chrono::Utc::now().timestamp() as u64,
            revoked_at: None,
            last_used_at: None,
            permissions: vec!["test_perm".to_string()],
            metadata: Some(serde_json::json!({"test": "metadata"})),
        };

        // Test set and get
        key_manager.set_root_key("test_key", &root_key).await.unwrap();
        let retrieved = key_manager.get_root_key("test_key").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.public_key, root_key.public_key);
        assert_eq!(retrieved.auth_method, root_key.auth_method);
        assert_eq!(retrieved.permissions, root_key.permissions);
        
        // Test find by public key
        let found = key_manager.find_root_key_by_public_key("test_pub_key").await.unwrap();
        assert!(found.is_some());
        let (found_id, found_key) = found.unwrap();
        assert_eq!(found_id, "test_key");
        assert_eq!(found_key.public_key, root_key.public_key);

        // Test list root keys
        let keys = key_manager.list_root_keys().await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, "test_key");
        assert_eq!(keys[0].1.public_key, root_key.public_key);

        // Test delete
        key_manager.delete_root_key("test_key").await.unwrap();
        assert!(key_manager.get_root_key("test_key").await.unwrap().is_none());
        assert!(key_manager.find_root_key_by_public_key("test_pub_key").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_client_key_operations() {
        let storage = Arc::new(MemoryStorage::new());
        let key_manager = KeyManager::new(storage);

        // First create a root key
        let root_key = RootKey {
            public_key: "root_pub_key".to_string(),
            auth_method: "near".to_string(),
            created_at: chrono::Utc::now().timestamp() as u64,
            revoked_at: None,
            last_used_at: None,
            permissions: vec!["test_perm".to_string()],
            metadata: Some(serde_json::json!({"test": "metadata"})),
        };
        key_manager.set_root_key("root_key", &root_key).await.unwrap();

        // Create a test client key
        let client_key = ClientKey {
            client_id: "test_client".to_string(),
            root_key_id: "root_key".to_string(),
            name: "Test Client".to_string(),
            permissions: vec!["test_perm".to_string()],
            created_at: chrono::Utc::now().timestamp() as u64,
            revoked_at: None,
            last_used_at: None,
            expires_at: None,
        };

        // Test set and get
        key_manager.set_client_key("test_client", &client_key).await.unwrap();
        let retrieved = key_manager.get_client_key("test_client").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.client_id, client_key.client_id);
        assert_eq!(retrieved.root_key_id, client_key.root_key_id);
        assert_eq!(retrieved.permissions, client_key.permissions);

        // Test list client keys for root
        let clients = key_manager.list_client_keys_for_root("root_key").await.unwrap();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].client_id, client_key.client_id);

        // Test delete
        key_manager.delete_client_key("test_client").await.unwrap();
        assert!(key_manager.get_client_key("test_client").await.unwrap().is_none());
        assert_eq!(key_manager.list_client_keys_for_root("root_key").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_permission_operations() {
        let storage = Arc::new(MemoryStorage::new());
        let key_manager = KeyManager::new(storage);

        // Create a test permission
        let permission = Permission {
            permission_id: "test_perm".to_string(),
            name: "Test Permission".to_string(),
            description: "A test permission".to_string(),
            resource_type: "test".to_string(),
            resource_ids: Some(vec!["test".to_string()]),
            method: Some("GET".to_string()),
            user_id: Some("test_user".to_string()),
        };

        // Test set and get
        key_manager.set_permission("test_perm", &permission).await.unwrap();
        let retrieved = key_manager.get_permission("test_perm").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.permission_id, permission.permission_id);
        assert_eq!(retrieved.name, permission.name);
        assert_eq!(retrieved.description, permission.description);

        // Test list permissions
        let permissions = key_manager.list_permissions().await.unwrap();
        assert_eq!(permissions.len(), 1);
        assert_eq!(permissions[0].permission_id, permission.permission_id);

        // Test delete
        key_manager.delete_permission("test_perm").await.unwrap();
        assert!(key_manager.get_permission("test_perm").await.unwrap().is_none());
        assert_eq!(key_manager.list_permissions().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_multiple_clients_per_root() {
        let storage = Arc::new(MemoryStorage::new());
        let key_manager = KeyManager::new(storage);

        // Create a root key
        let root_key = RootKey {
            public_key: "root_pub_key".to_string(),
            auth_method: "near".to_string(),
            created_at: chrono::Utc::now().timestamp() as u64,
            revoked_at: None,
            last_used_at: None,
            permissions: vec!["test_perm".to_string()],
            metadata: Some(serde_json::json!({"test": "metadata"})),
        };
        key_manager.set_root_key("root_key", &root_key).await.unwrap();

        // Create multiple client keys
        for i in 1..=5 {
            let client_key = ClientKey {
                client_id: format!("client{}", i),
                root_key_id: "root_key".to_string(),
                name: format!("Client {}", i),
                permissions: vec!["test_perm".to_string()],
                created_at: chrono::Utc::now().timestamp() as u64,
                revoked_at: None,
                last_used_at: None,
                expires_at: None,
            };
            key_manager.set_client_key(&format!("client{}", i), &client_key).await.unwrap();
        }

        // Verify we can list all clients
        let clients = key_manager.list_client_keys_for_root("root_key").await.unwrap();
        assert_eq!(clients.len(), 5);

        // Delete a couple of clients
        key_manager.delete_client_key("client1").await.unwrap();
        key_manager.delete_client_key("client3").await.unwrap();

        // Verify the remaining clients
        let clients = key_manager.list_client_keys_for_root("root_key").await.unwrap();
        assert_eq!(clients.len(), 3);

        // Verify specific clients
        let client_ids: Vec<String> = clients.iter().map(|c| c.client_id.clone()).collect();
        assert!(client_ids.contains(&"client2".to_string()));
        assert!(client_ids.contains(&"client4".to_string()));
        assert!(client_ids.contains(&"client5".to_string()));

        // Delete the root key and verify no clients are returned
        key_manager.delete_root_key("root_key").await.unwrap();
        let clients = key_manager.list_client_keys_for_root("root_key").await.unwrap();
        assert_eq!(clients.len(), 0);
    }

    #[tokio::test]
    async fn test_error_handling() {
        let storage = Arc::new(MemoryStorage::new());
        let key_manager = KeyManager::new(storage);

        // Test not found errors
        let result = key_manager.delete_root_key("nonexistent").await;
        assert!(matches!(result, Err(StorageError::NotFound)));

        let result = key_manager.delete_client_key("nonexistent").await;
        assert!(matches!(result, Err(StorageError::NotFound)));

        let result = key_manager.delete_permission("nonexistent").await;
        assert!(matches!(result, Err(StorageError::NotFound)));

        // Test root key not found when creating client
        let client_key = ClientKey {
            client_id: "test_client".to_string(),
            root_key_id: "nonexistent_root".to_string(),
            name: "Test Client".to_string(),
            permissions: vec![],
            created_at: chrono::Utc::now().timestamp() as u64,
            revoked_at: None,
            last_used_at: None,
            expires_at: None,
        };
        key_manager.set_client_key("test_client", &client_key).await.unwrap();

        // The client should still be created even if the root key doesn't exist
        let retrieved = key_manager.get_client_key("test_client").await.unwrap();
        assert!(retrieved.is_some());
    }
} 