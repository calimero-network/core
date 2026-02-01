use axum::body::Body;
use axum::http::Request;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::auth::permissions::{Permission, PermissionValidator};

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

    /// Node URL this key belongs to (for multi-node deployments)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_url: Option<String>,

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
        node_url: Option<String>,
    ) -> Self {
        Self {
            key_type: KeyType::Root,
            public_key: Some(public_key),
            auth_method: Some(auth_method),
            root_key_id: None,
            name: None,
            permissions,
            node_url,
            metadata: KeyMetadata::new(),
        }
    }

    /// Create a new client key
    pub fn new_client_key(
        root_key_id: String,
        name: String,
        permissions: Vec<String>,
        node_url: Option<String>,
    ) -> Self {
        Self {
            key_type: KeyType::Client,
            public_key: None,
            auth_method: None,
            root_key_id: Some(root_key_id),
            name: Some(name),
            permissions,
            node_url,
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

        // Convert required permission string to Permission enum
        if let Ok(required_perm) = required.parse::<Permission>() {
            // Root keys have special handling
            if self.is_root_key() {
                // Root keys automatically get admin access
                if required == "admin" || required.starts_with("admin:") {
                    return true;
                }
            }

            // Check if we have master permission
            if self.permissions.iter().any(|p| p == "admin") {
                return true;
            }

            // Convert our permissions to Permission enums and check each
            for perm_str in &self.permissions {
                if let Ok(held_perm) = perm_str.parse::<Permission>() {
                    if held_perm.satisfies(&required_perm) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Add a permission to the key
    pub fn add_permission(&mut self, permission: &str) -> Result<(), String> {
        // Validate permission format
        if permission.parse::<Permission>().is_err() {
            return Err("Invalid permission format".to_string());
        }

        // Check for duplicates
        if self.permissions.contains(&permission.to_string()) {
            return Ok(());
        }

        self.permissions.push(permission.to_string());
        Ok(())
    }

    /// Set permissions for this key
    pub fn set_permissions(&mut self, permissions: Vec<String>) {
        self.permissions = permissions;
    }

    /// Validate permissions for a request
    pub fn validate_request_permissions(&self, request: &Request<Body>) -> bool {
        let validator = PermissionValidator::new();

        // Get required permissions for this request
        let required_permissions = validator.determine_required_permissions(request);

        // Validate against our permissions
        validator.validate_permissions(&self.permissions, &required_permissions)
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

    /// Get the node URL this key belongs to
    pub fn get_node_url(&self) -> Option<&str> {
        self.node_url.as_deref()
    }

    /// Check if this key is valid for the given node URL
    pub fn is_valid_for_node(&self, node_url: Option<&str>) -> bool {
        match (&self.node_url, node_url) {
            (None, _) => true, // Legacy keys without node_url are valid everywhere
            (Some(key_node_url), Some(request_node_url)) => {
                request_node_url.starts_with(key_node_url)
            }
            (Some(_), None) => false, // Node-specific key used without node context
        }
    }
}

/// Common metadata for keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMetadata {
    /// When the key was created
    pub created_at: u64,
    /// When the key was revoked
    pub revoked_at: Option<u64>,
    /// When the key was last used (for idle timeout tracking)
    /// Defaults to created_at if not set (for backward compatibility with existing keys)
    #[serde(default)]
    pub last_activity: Option<u64>,
}

impl Default for KeyMetadata {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyMetadata {
    /// Create new key metadata
    pub fn new() -> Self {
        let now = Utc::now().timestamp() as u64;
        Self {
            created_at: now,
            revoked_at: None,
            last_activity: Some(now),
        }
    }

    /// Revoke the key
    pub fn revoke(&mut self) {
        self.revoked_at = Some(Utc::now().timestamp() as u64);
    }

    /// Update the last activity timestamp
    pub fn touch(&mut self) {
        self.last_activity = Some(Utc::now().timestamp() as u64);
    }

    /// Get the last activity timestamp, falling back to created_at for backward compatibility
    pub fn get_last_activity(&self) -> u64 {
        self.last_activity.unwrap_or(self.created_at)
    }

    /// Check if the key has been idle for longer than the specified timeout
    ///
    /// # Arguments
    ///
    /// * `idle_timeout_secs` - The idle timeout in seconds (0 means disabled)
    ///
    /// # Returns
    ///
    /// * `bool` - true if the key is idle (exceeded timeout), false otherwise
    pub fn is_idle(&self, idle_timeout_secs: u64) -> bool {
        if idle_timeout_secs == 0 {
            return false; // Idle timeout disabled
        }
        let now = Utc::now().timestamp() as u64;
        let last_activity = self.get_last_activity();
        now.saturating_sub(last_activity) > idle_timeout_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_metadata_new() {
        let metadata = KeyMetadata::new();
        assert!(metadata.revoked_at.is_none());
        assert!(metadata.last_activity.is_some());
        // last_activity should be approximately equal to created_at
        assert_eq!(metadata.last_activity.unwrap(), metadata.created_at);
    }

    #[test]
    fn test_key_metadata_touch() {
        let mut metadata = KeyMetadata::new();
        let original_activity = metadata.get_last_activity();
        // Touch should update the last_activity
        metadata.touch();
        // Since we can't easily test time changes, just verify it's set
        assert!(metadata.last_activity.is_some());
        assert!(metadata.get_last_activity() >= original_activity);
    }

    #[test]
    fn test_key_metadata_get_last_activity_with_value() {
        let mut metadata = KeyMetadata::new();
        metadata.last_activity = Some(12345);
        assert_eq!(metadata.get_last_activity(), 12345);
    }

    #[test]
    fn test_key_metadata_get_last_activity_fallback() {
        let mut metadata = KeyMetadata::new();
        metadata.last_activity = None;
        // Should fall back to created_at
        assert_eq!(metadata.get_last_activity(), metadata.created_at);
    }

    #[test]
    fn test_key_metadata_is_idle_disabled() {
        let mut metadata = KeyMetadata::new();
        // Set last_activity to a very old timestamp
        metadata.last_activity = Some(1);
        // With idle_timeout of 0, should never be idle
        assert!(!metadata.is_idle(0));
    }

    #[test]
    fn test_key_metadata_is_idle_not_expired() {
        let metadata = KeyMetadata::new();
        // Just created, with a 30 minute timeout, should not be idle
        assert!(!metadata.is_idle(30 * 60));
    }

    #[test]
    fn test_key_metadata_is_idle_expired() {
        let mut metadata = KeyMetadata::new();
        // Set last_activity to 2 hours ago
        let now = Utc::now().timestamp() as u64;
        metadata.last_activity = Some(now.saturating_sub(2 * 60 * 60));
        // With a 30 minute timeout, should be idle
        assert!(metadata.is_idle(30 * 60));
    }

    #[test]
    fn test_key_metadata_backward_compatibility() {
        // Simulate a key from before idle timeout was added (no last_activity)
        let metadata = KeyMetadata {
            created_at: 1000,
            revoked_at: None,
            last_activity: None,
        };
        // get_last_activity should return created_at
        assert_eq!(metadata.get_last_activity(), 1000);
    }

    #[test]
    fn test_key_is_valid_and_not_idle() {
        let key = Key::new_root_key_with_permissions(
            "test_pub_key".to_string(),
            "near".to_string(),
            vec!["admin".to_string()],
            None,
        );
        assert!(key.is_valid());
        // Newly created key should not be idle
        assert!(!key.metadata.is_idle(30 * 60));
    }
}
