use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Type of key
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum KeyType {
    /// Root key represents a user identity
    Root,
    /// Client key represents an application authorized by a root key
    Client,
}

/// Unified key model that handles both root and client keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Key {
    /// Type of key (root or client)
    pub key_type: KeyType,

    /// The public key (for root keys)
    pub public_key: Option<String>,

    /// The authentication method (for root keys)
    pub auth_method: Option<String>,

    /// The root key ID this client belongs to (for client keys)
    pub root_key_id: Option<String>,

    /// The application name (for client keys)
    pub name: Option<String>,

    /// Permissions assigned to this key
    pub permissions: Vec<String>,

    /// Key metadata
    #[serde(flatten)]
    pub metadata: KeyMetadata,
}

impl Key {
    /// Create a new root key
    pub fn new_root_key(public_key: String, auth_method: String) -> Self {
        Self {
            key_type: KeyType::Root,
            public_key: Some(public_key),
            auth_method: Some(auth_method),
            root_key_id: None,
            name: None,
            permissions: Vec::new(),
            metadata: KeyMetadata::new(),
        }
    }

    /// Create a new root key with permissions
    pub fn new_root_key_with_permissions(
        public_key: String,
        auth_method: String,
        permissions: Vec<String>,
    ) -> Self {
        Self {
            key_type: KeyType::Root,
            public_key: Some(public_key),
            auth_method: Some(auth_method),
            root_key_id: None,
            name: None,
            permissions,
            metadata: KeyMetadata::new(),
        }
    }

    /// Create a new client key
    pub fn new_client_key(root_key_id: String, name: String, permissions: Vec<String>) -> Self {
        Self {
            key_type: KeyType::Client,
            public_key: None,
            auth_method: None,
            root_key_id: Some(root_key_id),
            name: Some(name),
            permissions,
            metadata: KeyMetadata::new(),
        }
    }

    /// Check if this is a root key
    pub fn is_root_key(&self) -> bool {
        self.key_type == KeyType::Root
    }

    /// Check if this is a client key
    pub fn is_client_key(&self) -> bool {
        self.key_type == KeyType::Client
    }

    /// Get the public key (for root keys)
    pub fn get_public_key(&self) -> Option<&str> {
        self.public_key.as_deref()
    }

    /// Get the auth method (for root keys)
    pub fn get_auth_method(&self) -> Option<&str> {
        self.auth_method.as_deref()
    }

    /// Get the root key ID (for client keys)
    pub fn get_root_key_id(&self) -> Option<&str> {
        self.root_key_id.as_deref()
    }

    /// Get the name (for client keys)
    pub fn get_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Check if the key is revoked
    pub fn is_revoked(&self) -> bool {
        self.metadata.revoked_at.is_some()
    }

    /// Check if the key is valid (not revoked)
    pub fn is_valid(&self) -> bool {
        !self.is_revoked()
    }

    /// Revoke the key
    pub fn revoke(&mut self) {
        self.metadata.revoke();
    }

    /// Set permissions for this key
    pub fn set_permissions(&mut self, permissions: Vec<String>) {
        self.permissions = permissions;
    }

    /// Add a permission
    pub fn add_permission(&mut self, permission: String) {
        if !self.permissions.contains(&permission) {
            self.permissions.push(permission);
        }
    }

    /// Check if the key has a specific permission
    pub fn has_permission(&self, permission: &str) -> bool {
        // Check if key is valid
        if !self.is_valid() {
            return false;
        }

        // Root keys have admin access by default
        if self.is_root_key() {
            if permission.starts_with("admin:") {
                return true;
            }

            // Root keys can manage their own client keys
            if permission.starts_with("clients:") {
                return true;
            }
        }

        // Check exact match
        if self.permissions.contains(&permission.to_string()) {
            return true;
        }

        // Check wildcard permissions
        if self.permissions.contains(&"*".to_string()) {
            return true;
        }

        // Parse the permission into parts
        let parts: Vec<&str> = permission.split(':').collect();
        if parts.is_empty() {
            return false;
        }

        // Check hierarchical permissions
        let mut current = String::new();
        for part in parts {
            if current.is_empty() {
                current = part.to_string();
            } else {
                current = format!("{}:{}", current, part);
            }

            // Check if we have permission at this level
            if self.permissions.contains(&format!("{}:*", current)) {
                return true;
            }
        }

        // Check resource-specific permissions (with IDs)
        if permission.contains('[') && permission.contains(']') {
            let base_permission = permission.split('[').next().unwrap_or("");
            if self
                .permissions
                .iter()
                .any(|p| p.starts_with(base_permission))
            {
                return true;
            }
        }

        false
    }
}

/// Common metadata for keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMetadata {
    /// When the key was created
    pub created_at: u64,
    /// When the key was revoked
    pub revoked_at: Option<u64>,
}

impl KeyMetadata {
    /// Create new key metadata
    pub fn new() -> Self {
        Self {
            created_at: Utc::now().timestamp() as u64,
            revoked_at: None,
        }
    }

    /// Revoke the key
    pub fn revoke(&mut self) {
        self.revoked_at = Some(Utc::now().timestamp() as u64);
    }
}
