use serde::{Deserialize, Serialize};

/// Root key storage model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootKey {
    /// The public key
    pub public_key: String,

    /// The authentication method
    pub auth_method: String,

    /// When the key was created
    pub created_at: u64,

    /// When the key was revoked (if it was)
    pub revoked_at: Option<u64>,

    /// When the key was last used
    pub last_used_at: Option<u64>,
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
            created_at: chrono::Utc::now().timestamp() as u64,
            revoked_at: None,
            last_used_at: None,
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
}
