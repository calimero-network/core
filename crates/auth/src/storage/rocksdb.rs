use std::path::Path;

use async_trait::async_trait;
use rocksdb::{IteratorMode, DB};

use super::{
    deserialize, models::prefixes, serialize, ClientKey, Permission, RootKey, Storage,
    StorageError,
};

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

    // Model-specific methods

    /// Get a root key by ID
    ///
    /// # Arguments
    ///
    /// * `key_id` - The key ID
    ///
    /// # Returns
    ///
    /// * `Result<Option<RootKey>, StorageError>` - The root key if found
    pub async fn get_root_key(&self, key_id: &str) -> Result<Option<RootKey>, StorageError> {
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
        match self.get(&key).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    /// Set a root key
    ///
    /// # Arguments
    ///
    /// * `key_id` - The key ID
    /// * `root_key` - The root key
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    pub async fn set_root_key(&self, key_id: &str, root_key: &RootKey) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
        let value = serialize(root_key)?;
        self.set(&key, &value).await
    }

    /// Delete a root key
    ///
    /// # Arguments
    ///
    /// * `key_id` - The key ID
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    pub async fn delete_root_key(&self, key_id: &str) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
        self.delete(&key).await
    }

    /// List all root keys
    ///
    /// # Returns
    ///
    /// * `Result<Vec<(String, RootKey)>, StorageError>` - The root keys
    pub async fn list_root_keys(&self) -> Result<Vec<(String, RootKey)>, StorageError> {
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

    /// Get a client key by ID
    ///
    /// # Arguments
    ///
    /// * `client_id` - The client ID
    ///
    /// # Returns
    ///
    /// * `Result<Option<ClientKey>, StorageError>` - The client key if found
    pub async fn get_client_key(&self, client_id: &str) -> Result<Option<ClientKey>, StorageError> {
        let key = format!("{}{}", prefixes::CLIENT_KEY, client_id);
        match self.get(&key).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    /// Set a client key
    ///
    /// # Arguments
    ///
    /// * `client_id` - The client ID
    /// * `client_key` - The client key
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    pub async fn set_client_key(
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

    /// Delete a client key
    ///
    /// # Arguments
    ///
    /// * `client_id` - The client ID
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    pub async fn delete_client_key(&self, client_id: &str) -> Result<(), StorageError> {
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

    /// List client keys for a root key
    ///
    /// # Arguments
    ///
    /// * `root_key_id` - The root key ID
    ///
    /// # Returns
    ///
    /// * `Result<Vec<ClientKey>, StorageError>` - The client keys
    pub async fn list_client_keys_for_root(
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

    /// Get a permission by ID
    ///
    /// # Arguments
    ///
    /// * `permission_id` - The permission ID
    ///
    /// # Returns
    ///
    /// * `Result<Option<Permission>, StorageError>` - The permission if found
    pub async fn get_permission(
        &self,
        permission_id: &str,
    ) -> Result<Option<Permission>, StorageError> {
        let key = format!("{}{}", prefixes::PERMISSION, permission_id);
        match self.get(&key).await? {
            Some(data) => Ok(Some(deserialize(&data)?)),
            None => Ok(None),
        }
    }

    /// Set a permission
    ///
    /// # Arguments
    ///
    /// * `permission_id` - The permission ID
    /// * `permission` - The permission
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    pub async fn set_permission(
        &self,
        permission_id: &str,
        permission: &Permission,
    ) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::PERMISSION, permission_id);
        let value = serialize(permission)?;
        self.set(&key, &value).await
    }

    /// Delete a permission
    ///
    /// # Arguments
    ///
    /// * `permission_id` - The permission ID
    ///
    /// # Returns
    ///
    /// * `Result<(), StorageError>` - Success or error
    pub async fn delete_permission(&self, permission_id: &str) -> Result<(), StorageError> {
        let key = format!("{}{}", prefixes::PERMISSION, permission_id);
        self.delete(&key).await
    }

    /// List all permissions
    ///
    /// # Returns
    ///
    /// * `Result<Vec<Permission>, StorageError>` - The permissions
    pub async fn list_permissions(&self) -> Result<Vec<Permission>, StorageError> {
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

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

    #[tokio::test]
    async fn test_root_key_operations() {
        let (db, _dir) = setup_db().await;
        let key_id = "test-key";
        let root_key = RootKey::new("pk12345".to_string(), "near".to_string());

        // Initially, the key shouldn't exist
        let result = db.get_root_key(key_id).await.unwrap();
        assert!(result.is_none());

        // Set the key
        db.set_root_key(key_id, &root_key).await.unwrap();

        // Now the key should exist
        let result = db.get_root_key(key_id).await.unwrap();
        assert!(result.is_some());
        let stored_key = result.unwrap();
        assert_eq!(stored_key.public_key, root_key.public_key);
        assert_eq!(stored_key.auth_method, root_key.auth_method);

        // List all root keys
        let result = db.list_root_keys().await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, key_id);
        assert_eq!(result[0].1.public_key, root_key.public_key);

        // Delete the key
        db.delete_root_key(key_id).await.unwrap();

        // Key should no longer exist
        let result = db.get_root_key(key_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_client_key_operations() {
        let (db, _dir) = setup_db().await;
        let root_key_id = "root-key";
        let client_id = "client-key";
        let client_key = ClientKey::new(
            client_id.to_string(),
            root_key_id.to_string(),
            "Test Client".to_string(),
            vec!["permission1".to_string()],
            None,
        );

        // Set the client key
        db.set_client_key(client_id, &client_key).await.unwrap();

        // Get the client key
        let result = db.get_client_key(client_id).await.unwrap();
        assert!(result.is_some());
        let stored_key = result.unwrap();
        assert_eq!(stored_key.client_id, client_key.client_id);
        assert_eq!(stored_key.root_key_id, client_key.root_key_id);

        // List client keys for root
        let result = db.list_client_keys_for_root(root_key_id).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].client_id, client_key.client_id);

        // Delete the client key
        db.delete_client_key(client_id).await.unwrap();

        // Key should no longer exist
        let result = db.get_client_key(client_id).await.unwrap();
        assert!(result.is_none());

        // Root key to client index should be empty
        let result = db.list_client_keys_for_root(root_key_id).await.unwrap();
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_permission_operations() {
        let (db, _dir) = setup_db().await;
        let permission_id = "test-permission";
        let permission = Permission::new(
            permission_id.to_string(),
            "Test Permission".to_string(),
            "A test permission".to_string(),
            "test".to_string(),
        );

        // Initially, the permission shouldn't exist
        let result = db.get_permission(permission_id).await.unwrap();
        assert!(result.is_none());

        // Set the permission
        db.set_permission(permission_id, &permission).await.unwrap();

        // Now the permission should exist
        let result = db.get_permission(permission_id).await.unwrap();
        assert!(result.is_some());
        let stored_permission = result.unwrap();
        assert_eq!(stored_permission.permission_id, permission.permission_id);
        assert_eq!(stored_permission.name, permission.name);

        // List all permissions
        let result = db.list_permissions().await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].permission_id, permission.permission_id);

        // Delete the permission
        db.delete_permission(permission_id).await.unwrap();

        // Permission should no longer exist
        let result = db.get_permission(permission_id).await.unwrap();
        assert!(result.is_none());
    }
}
