use serde::{Deserialize, Serialize};

/// Root key storage model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootKey {
    /// The public key
    pub public_key: String,

    /// The authentication method
    pub auth_method: String,

    /// Permissions assigned to this key
    pub permissions: Vec<String>,

    /// When the key was created
    pub created_at: u64,

    /// When the key expires
    pub expires_at: Option<u64>,
    
    /// When the key was last used
    pub last_used_at: Option<u64>,

    /// When the key was revoked (if it was)
    pub revoked_at: Option<u64>,

}

impl RootKey {
    /// Create a new root key
    ///
    /// # Arguments
    ///
    /// * `public_key` - The public key
    /// * `auth_method` - The authentication method
    ///
    /// # Returns
    ///
    /// * `Self` - The new root key
    pub fn new(public_key: String, auth_method: String) -> Self {
        Self {
            public_key,
            auth_method,
            permissions: Vec::new(),
            created_at: chrono::Utc::now().timestamp() as u64,
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        }
    }

    /// Create a new root key with permissions
    ///
    /// # Arguments
    ///
    /// * `public_key` - The public key
    /// * `auth_method` - The authentication method
    /// * `permissions` - Initial permissions
    ///
    /// # Returns
    ///
    /// * `Self` - The new root key
    pub fn new_with_permissions(
        public_key: String,
        auth_method: String,
        permissions: Vec<String>,
    ) -> Self {
        Self {
            public_key,
            auth_method,
            permissions,
            created_at: chrono::Utc::now().timestamp() as u64,
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        }
    }

    /// Revoke the key
    pub fn revoke(&mut self) {
        self.revoked_at = Some(chrono::Utc::now().timestamp() as u64);
    }

    /// Update the last used timestamp
    pub fn update_last_used(&mut self) {
        self.last_used_at = Some(chrono::Utc::now().timestamp() as u64);
    }

    /// Check if the key is revoked
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Check if the key is valid (not revoked)
    pub fn is_valid(&self) -> bool {
        !self.is_revoked()
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
        // Root keys have admin access by default
        if permission.starts_with("admin:") {
            return true;
        }

        // Root keys can manage their own client keys
        if permission.starts_with("clients:") {
            return true;
        }

        // Check explicit permissions
        if self.permissions.contains(&"*".to_string()) {
            return true;
        }

        self.permissions.contains(&permission.to_string())
    }
}
