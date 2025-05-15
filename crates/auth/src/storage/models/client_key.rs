use serde::{Deserialize, Serialize};

/// Client key storage model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientKey {
    /// The client ID
    pub client_id: String,

    /// The root key ID
    pub root_key_id: String,

    /// The name of the client
    pub name: String,

    /// The permissions granted to the client
    pub permissions: Vec<String>,

    /// When the client key was created
    pub created_at: u64,

    /// When the client key expires
    pub expires_at: Option<u64>,

    /// When the client key was revoked (if it was)
    pub revoked_at: Option<u64>,
}

impl ClientKey {
    /// Create a new client key
    ///
    /// # Arguments
    ///
    /// * `client_id` - The client ID
    /// * `root_key_id` - The root key ID
    /// * `name` - The name of the client
    /// * `permissions` - The permissions granted to the client
    /// * `expires_at` - When the client key expires (if ever)
    ///
    /// # Returns
    ///
    /// * `Self` - The new client key
    pub fn new(
        client_id: String,
        root_key_id: String,
        name: String,
        permissions: Vec<String>,
        expires_at: Option<u64>,
    ) -> Self {
        Self {
            client_id,
            root_key_id,
            name,
            permissions,
            created_at: chrono::Utc::now().timestamp() as u64,
            expires_at,
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
            "default-client".to_string(),
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
    /// * `client_id` - The registered OAuth client ID
    /// * `root_key_id` - The root key ID (user identity)
    /// * `name` - Human-readable name of the client application
    /// * `permissions` - The permissions (scopes) granted to this client
    /// * `expires_at` - Optional expiration time
    ///
    /// # Returns
    ///
    /// * `Self` - A new client key for the OAuth client
    pub fn create_for_oauth_client(
        client_id: String,
        root_key_id: String,
        name: String,
        permissions: Vec<String>,
        expires_at: Option<u64>,
    ) -> Self {
        Self::new(client_id, root_key_id, name, permissions, expires_at)
    }

    /// Revoke the client key
    pub fn revoke(&mut self) {
        self.revoked_at = Some(chrono::Utc::now().timestamp() as u64);
    }

    /// Update the permissions
    ///
    /// # Arguments
    ///
    /// * `permissions` - The new permissions
    pub fn update_permissions(&mut self, permissions: Vec<String>) {
        self.permissions = permissions;
    }

    /// Check if the client key is revoked
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Check if the client key is expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            expires_at < chrono::Utc::now().timestamp() as u64
        } else {
            false
        }
    }

    /// Check if the client key is valid (not revoked and not expired)
    pub fn is_valid(&self) -> bool {
        !self.is_revoked() && !self.is_expired()
    }
}
