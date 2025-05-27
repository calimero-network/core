use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Client key storage model
///
/// A Client Key is an application-specific key that is derived from a Root Key.
/// While Root Keys represent user identities, Client Keys represent specific applications
/// or services that are authorized to act on behalf of that user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientKey {
    /// The root key ID this client belongs to (user identity)
    pub root_key_id: String,

    /// The application name
    pub name: String,

    /// The application's permissions (subset of root key permissions)
    pub permissions: Vec<String>,

    /// When the key was created (Unix timestamp)
    pub created_at: u64,

    /// When the key expires (Unix timestamp)
    pub expires_at: Option<u64>,

    /// When the key was last used (Unix timestamp)
    pub last_used_at: Option<u64>,

    /// When the key was revoked (Unix timestamp)
    pub revoked_at: Option<u64>,
}

impl ClientKey {
    /// Create a new client key for an application
    ///
    /// # Arguments
    ///
    /// * `root_key_id` - The root key (user) this client belongs to
    /// * `name` - The application name
    /// * `permissions` - The permissions granted to this application (must be a subset of root key permissions)
    /// * `expires_at` - When the client key expires (if any)
    ///
    /// # Returns
    ///
    /// * `Self` - The new client key
    pub fn new(
        root_key_id: String,
        name: String,
        permissions: Vec<String>,
        expires_at: Option<u64>,
    ) -> Self {
        let now = Utc::now().timestamp() as u64;
        Self {
            root_key_id,
            name,
            permissions,
            created_at: now,
            expires_at,
            last_used_at: Some(now),
            revoked_at: None,
        }
    }

    /// Create a default client key for simple authentication scenarios
    ///
    /// This helper method creates a client key with a default client ID.
    /// Use this when you don't need to distinguish between different clients/devices
    /// and just want simple user authentication.
    ///
    /// # Arguments
    ///
    /// * `root_key_id` - The root key ID (user identity)
    /// * `permissions` - The permissions granted to the user
    /// * `expires_at` - Optional expiration time
    ///
    /// # Returns
    ///
    /// * `Self` - A new client key with default client settings
    pub fn create_default_for_user(
        root_key_id: String,
        permissions: Vec<String>,
        expires_at: Option<u64>,
    ) -> Self {
        Self::new(
            root_key_id,
            "Default Application".to_string(),
            permissions,
            expires_at,
        )
    }

    /// Create a client key for OAuth client application
    ///
    /// This helper method creates a client key for OAuth scenarios where
    /// multiple client applications might request access to a user's resources.
    ///
    /// # Arguments
    ///
    /// * `root_key_id` - The root key ID (user identity)
    /// * `name` - Human-readable name of the client application
    /// * `permissions` - The permissions (scopes) granted to this client
    /// * `expires_at` - Optional expiration time
    ///
    /// # Returns
    ///
    /// * `Self` - A new client key for the OAuth client
    pub fn create_for_oauth_client(
        root_key_id: String,
        name: String,
        permissions: Vec<String>,
        expires_at: Option<u64>,
    ) -> Self {
        Self::new(root_key_id, name, permissions, expires_at)
    }

    /// Check if the key has a specific permission
    pub fn has_permission(&self, permission: &str) -> bool {
        // Check if key is valid
        if !self.is_valid() {
            return false;
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

    /// Check if the key is revoked
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Check if the key is expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            expires_at < Utc::now().timestamp() as u64
        } else {
            false
        }
    }

    /// Check if the key is valid (not revoked and not expired)
    pub fn is_valid(&self) -> bool {
        let now = Utc::now().timestamp() as u64;
        self.revoked_at.is_none() && self.expires_at.map(|exp| exp > now).unwrap_or(true)
    }

    /// Update the last used timestamp
    pub fn update_last_used(&mut self) {
        self.last_used_at = Some(Utc::now().timestamp() as u64);
    }

    /// Revoke the key
    pub fn revoke(&mut self) {
        self.revoked_at = Some(Utc::now().timestamp() as u64);
    }

    /// Update the permissions
    ///
    /// # Arguments
    ///
    /// * `permissions` - The new permissions (must be a subset of root key permissions)
    pub fn update_permissions(&mut self, permissions: Vec<String>) {
        self.permissions = permissions;
    }

    /// Extend the key's expiry time
    ///
    /// # Arguments
    ///
    /// * `new_expiry` - The new expiry timestamp
    pub fn extend_expiry(&mut self, new_expiry: u64) {
        if let Some(current_expiry) = self.expires_at {
            if new_expiry > current_expiry {
                self.expires_at = Some(new_expiry);
            }
        } else {
            self.expires_at = Some(new_expiry);
        }
    }
}
