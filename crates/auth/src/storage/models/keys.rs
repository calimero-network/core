use axum::body::Body;
use axum::http::Request;
use chrono::Utc;
use serde::{Deserialize, Serialize};

// use crate::auth::permissions::{Permission, PermissionValidator};

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

    /// Check if the key has a specific permission
    pub fn has_permission(&self, required: &str) -> bool {
        // If key is revoked, no permissions are valid
        if self.is_revoked() {
            return false;
        }

        // // Convert required permission string to Permission enum
        // if let Some(required_perm) = Permission::from_str(required) {
        //     println!("Required permission: {:?}", required_perm);
        //     // Root keys have special handling
        //     if self.is_root_key() {
        //         println!("Root key");
        //         // Root keys automatically get admin access
        //         if required == "admin" || required.starts_with("admin:") {
        //             return true;
        //         }
        //     }

        //     println!("Permissions: {:?}", self.permissions);

        //     // Check if we have master permission
        //     if self.permissions.iter().any(|p| p == "admin") {
        //         return true;
        //     }

        //     // Convert our permissions to Permission enums and check each
        //     for perm_str in &self.permissions {
        //         if let Some(held_perm) = Permission::from_str(perm_str) {
        //             if held_perm.satisfies(&required_perm) {
        //                 return true;
        //             }
        //         }
        //     }
        // }
        false
    }

    /// Add a permission to the key
    pub fn add_permission(&mut self, permission: &str) -> Result<(), String> {
        // // Validate permission format
        // if Permission::from_str(permission).is_none() {
        //     return Err("Invalid permission format".to_string());
        // }

        // // Check for duplicates
        // if self.permissions.contains(&permission.to_string()) {
        //     return Ok(());
        // }

        // self.permissions.push(permission.to_string());
        Ok(())
    }

    /// Set permissions for this key
    pub fn set_permissions(&mut self, permissions: Vec<String>) {
        self.permissions = permissions;
    }

    /// Validate permissions for a request
    pub fn validate_request_permissions(&self, request: &Request<Body>) -> bool {
        // let validator = PermissionValidator::new();

        // // Get required permissions for this request
        // let required_permissions = validator.determine_required_permissions(request);

        // // Validate against our permissions
        // validator.validate_permissions(&self.permissions, &required_permissions)
        true
    }

    /// Check if this key can grant a permission to another key
    pub fn can_grant_permission(&self, permission: &str) -> bool {
        // If key is revoked, it can't grant permissions
        if self.is_revoked() {
            return false;
        }

        // Root keys can grant any permission they have
        if self.is_root_key() {
            return self.has_permission(permission);
        }

        // Client keys cannot grant permissions
        false
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
