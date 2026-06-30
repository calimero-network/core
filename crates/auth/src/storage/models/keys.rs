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

    /// Check if the key has expired.
    ///
    /// A key with no `expires_at` set never expires (returns `false`),
    /// preserving the behavior of keys created before expiry was introduced.
    pub fn is_expired(&self) -> bool {
        self.metadata.is_expired()
    }

    /// Check if the key is valid (neither revoked nor expired)
    pub fn is_valid(&self) -> bool {
        !self.is_revoked() && !self.is_expired()
    }

    /// Revoke the key
    pub fn revoke(&mut self) {
        self.metadata.revoke();
    }

    /// Set an absolute expiry (Unix timestamp, seconds) on this key.
    ///
    /// `None` clears the expiry, making the key non-expiring.
    pub fn set_expires_at(&mut self, expires_at: Option<u64>) {
        self.metadata.expires_at = expires_at;
    }

    /// Builder-style helper that applies a time-to-live (in seconds) to the key,
    /// computing an absolute `expires_at` from the current time.
    ///
    /// A `None` TTL leaves the key non-expiring (current default behavior).
    #[must_use]
    pub fn with_ttl_secs(mut self, ttl_secs: Option<u64>) -> Self {
        if let Some(ttl_secs) = ttl_secs {
            let now = Utc::now().timestamp().max(0) as u64;
            self.metadata.expires_at = Some(now.saturating_add(ttl_secs));
        }
        self
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

    /// Check if this key is valid for the given node URL.
    ///
    /// Node binding is an **exact host match**: the host parsed from the key's
    /// `node_url` must equal the host parsed from the request's `node_url`.
    /// A prefix/`starts_with` comparison was previously used, which allowed
    /// `node.example.com.attacker.com` to match a key bound to
    /// `node.example.com`. This fails closed: if either side cannot be parsed
    /// into a host, the key is rejected for that request.
    pub fn is_valid_for_node(&self, node_url: Option<&str>) -> bool {
        match (&self.node_url, node_url) {
            (None, _) => true, // Legacy keys without node_url are valid everywhere
            (Some(key_node_url), Some(request_node_url)) => {
                match (extract_host(key_node_url), extract_host(request_node_url)) {
                    (Some(key_host), Some(request_host)) => key_host == request_host,
                    // Fail closed when either host cannot be determined.
                    _ => false,
                }
            }
            (Some(_), None) => false, // Node-specific key used without node context
        }
    }
}

/// Extract the host component from a node URL string.
///
/// Accepts either a full URL (e.g. `https://node.example.com:8080`) or a bare
/// `host[:port]` authority (e.g. `node.example.com:8080`). Returns the
/// lowercased host with any port stripped, or `None` if no host can be
/// determined. Used for exact node-binding comparisons.
fn extract_host(node_url: &str) -> Option<String> {
    if let Ok(url) = url::Url::parse(node_url) {
        if let Some(host) = url.host_str() {
            return Some(host.to_ascii_lowercase());
        }
    }

    // Fall back to treating the value as a bare `host[:port]` authority.
    let trimmed = node_url.trim();
    if trimmed.is_empty() {
        return None;
    }

    let host = trimmed.split('/').next().unwrap_or(trimmed);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// Common metadata for keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMetadata {
    /// When the key was created
    pub created_at: u64,
    /// When the key was revoked
    pub revoked_at: Option<u64>,
    /// When the key expires (Unix timestamp, seconds).
    ///
    /// `None` means the key never expires. This field defaults to `None` on
    /// deserialization so keys persisted before expiry existed still load and
    /// remain non-expiring.
    #[serde(default)]
    pub expires_at: Option<u64>,
}

impl Default for KeyMetadata {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyMetadata {
    /// Create new key metadata
    pub fn new() -> Self {
        Self {
            created_at: Utc::now().timestamp() as u64,
            revoked_at: None,
            expires_at: None,
        }
    }

    /// Check whether the key has expired relative to the current time.
    ///
    /// Returns `false` when no expiry is set (non-expiring key).
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => {
                let now = Utc::now().timestamp().max(0) as u64;
                now >= expires_at
            }
            None => false,
        }
    }

    /// Revoke the key
    pub fn revoke(&mut self) {
        self.revoked_at = Some(Utc::now().timestamp() as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_key() -> Key {
        Key::new_root_key_with_permissions(
            "pubkey".to_string(),
            "user_password".to_string(),
            vec!["admin".to_string()],
            None,
        )
    }

    fn now_secs() -> u64 {
        Utc::now().timestamp().max(0) as u64
    }

    // --- #9: key expiry -------------------------------------------------

    #[test]
    fn test_key_with_no_expiry_is_valid() {
        let key = root_key();
        assert!(key.metadata.expires_at.is_none());
        assert!(!key.is_expired());
        assert!(key.is_valid());
    }

    #[test]
    fn test_key_not_yet_expired_is_valid() {
        let mut key = root_key();
        key.set_expires_at(Some(now_secs() + 3600));
        assert!(!key.is_expired());
        assert!(key.is_valid());
    }

    #[test]
    fn test_expired_key_is_invalid() {
        let mut key = root_key();
        key.set_expires_at(Some(now_secs().saturating_sub(10)));
        assert!(key.is_expired());
        assert!(!key.is_valid());
    }

    #[test]
    fn test_with_ttl_secs_sets_future_expiry() {
        let key = root_key().with_ttl_secs(Some(3600));
        assert!(key.metadata.expires_at.is_some());
        assert!(!key.is_expired());
        assert!(key.is_valid());
    }

    #[test]
    fn test_with_ttl_secs_none_is_non_expiring() {
        let key = root_key().with_ttl_secs(None);
        assert!(key.metadata.expires_at.is_none());
        assert!(!key.is_expired());
    }

    #[test]
    fn test_expires_at_defaults_to_none_on_deserialize() {
        // Legacy persisted metadata without an `expires_at` field must still
        // load and remain non-expiring.
        let legacy = r#"{"created_at": 1000, "revoked_at": null}"#;
        let meta: KeyMetadata = serde_json::from_str(legacy).unwrap();
        assert!(meta.expires_at.is_none());
        assert!(!meta.is_expired());
    }

    #[test]
    fn test_legacy_key_json_without_expiry_loads() {
        let legacy = r#"{
            "key_type": "Root",
            "public_key": "pubkey",
            "auth_method": "user_password",
            "root_key_id": null,
            "name": null,
            "permissions": ["admin"],
            "created_at": 1000,
            "revoked_at": null
        }"#;
        let key: Key = serde_json::from_str(legacy).unwrap();
        assert!(key.metadata.expires_at.is_none());
        assert!(key.is_valid());
    }

    // --- #11: node-host binding exact match, fail closed ----------------

    #[test]
    fn test_node_binding_rejects_suffix_attack() {
        let mut key = root_key();
        key.node_url = Some("https://node.example.com".to_string());
        assert!(!key.is_valid_for_node(Some("https://node.example.com.attacker.com")));
    }

    #[test]
    fn test_node_binding_exact_match_accepted() {
        let mut key = root_key();
        key.node_url = Some("https://node.example.com".to_string());
        assert!(key.is_valid_for_node(Some("https://node.example.com")));
    }

    #[test]
    fn test_node_binding_exact_match_ignores_port() {
        let mut key = root_key();
        key.node_url = Some("https://node.example.com".to_string());
        assert!(key.is_valid_for_node(Some("https://node.example.com:8443")));
    }

    #[test]
    fn test_node_binding_absent_request_node_rejected() {
        let mut key = root_key();
        key.node_url = Some("https://node.example.com".to_string());
        assert!(!key.is_valid_for_node(None));
    }

    #[test]
    fn test_node_binding_legacy_key_valid_everywhere() {
        let key = root_key();
        assert!(key.node_url.is_none());
        assert!(key.is_valid_for_node(Some("https://anything.example.com")));
        assert!(key.is_valid_for_node(None));
    }

    #[test]
    fn test_node_binding_bare_host_authority() {
        let mut key = root_key();
        key.node_url = Some("node.example.com".to_string());
        assert!(key.is_valid_for_node(Some("node.example.com:8443")));
        assert!(!key.is_valid_for_node(Some("node.example.com.attacker.com")));
    }
}
