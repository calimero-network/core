use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use calimero_client::{ClientStorage, storage::JwtToken as ClientJwtToken};
use async_trait::async_trait;
use eyre::Result;

// Keep the old JwtToken for backward compatibility during migration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

/// File-based implementation of ClientStorage for meroctl
/// 
/// This implementation uses meroctl's Config functionality to provide
/// file-based token storage that integrates with the existing configuration system.
#[derive(Debug, Clone)]
pub struct FileTokenStorage;

impl FileTokenStorage {
    /// Create a new file token storage instance
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ClientStorage for FileTokenStorage {
    async fn load_tokens(&self, node_name: &str) -> Result<Option<ClientJwtToken>> {
        // Load config and get tokens for the specified node
        let config = crate::config::Config::load().await?;
        
        if let Some(connection) = config.nodes.get(node_name) {
            match connection {
                crate::config::NodeConnection::Local { jwt_tokens, .. } |
                crate::config::NodeConnection::Remote { jwt_tokens, .. } => {
                    if let Some(tokens) = jwt_tokens {
                        // Convert from meroctl JwtToken to calimero-client JwtToken
                        let client_tokens = if let Some(refresh) = &tokens.refresh_token {
                            ClientJwtToken::with_refresh(tokens.access_token.clone(), refresh.clone())
                        } else {
                            ClientJwtToken::new(tokens.access_token.clone())
                        };
                        Ok(Some(client_tokens))
                    } else {
                        Ok(None)
                    }
                }
            }
        } else {
            Ok(None)
        }
    }
    
    async fn save_tokens(&self, node_name: &str, tokens: &ClientJwtToken) -> Result<()> {
        // Load existing config
        let mut config = crate::config::Config::load().await?;
        
        // Convert from calimero-client JwtToken to meroctl JwtToken
        let meroctl_tokens = JwtToken {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
        };
        
        // Update the node connection with new tokens
        if let Some(connection) = config.nodes.get_mut(node_name) {
            match connection {
                crate::config::NodeConnection::Local { jwt_tokens, .. } |
                crate::config::NodeConnection::Remote { jwt_tokens, .. } => {
                    *jwt_tokens = Some(meroctl_tokens);
                }
            }
        }
        
        // Save the updated config
        config.save().await
    }
    
    async fn update_tokens(&self, node_name: &str, new_tokens: &ClientJwtToken) -> Result<()> {
        // For now, just call save_tokens since it does the same thing
        self.save_tokens(node_name, new_tokens).await
    }
    
    async fn remove_tokens(&self, node_name: &str) -> Result<()> {
        // Load existing config
        let mut config = crate::config::Config::load().await?;
        
        // Remove tokens from the node connection
        if let Some(connection) = config.nodes.get_mut(node_name) {
            match connection {
                crate::config::NodeConnection::Local { jwt_tokens, .. } |
                crate::config::NodeConnection::Remote { jwt_tokens, .. } => {
                    *jwt_tokens = None;
                }
            }
        }
        
        // Save the updated config
        config.save().await
    }
    
    async fn list_nodes(&self) -> Result<Vec<String>> {
        // Load config and return list of node names
        let config = crate::config::Config::load().await?;
        Ok(config.nodes.keys().cloned().collect())
    }
}

/// Simple in-memory cache for external connection tokens
/// These tokens are only kept for the duration of the session
#[derive(Debug, Default)]
pub struct SessionTokenCache {
    tokens: Mutex<HashMap<String, ClientJwtToken>>,
}

impl SessionTokenCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store tokens for an external connection (session only)
    pub async fn store_tokens(&self, url: &str, tokens: &ClientJwtToken) {
        let key = format!("external_{}", url);
        if let Ok(mut cache) = self.tokens.lock() {
            drop(cache.insert(key, tokens.clone()));
        }
    }

    /// Get tokens for an external connection
    pub async fn get_tokens(&self, url: &str) -> Option<ClientJwtToken> {
        let key = format!("external_{}", url);
        self.tokens.lock().ok()?.get(&key).cloned()
    }

    /// Update tokens for an external connection
    pub async fn update_tokens(&self, url: &str, tokens: &ClientJwtToken) {
        self.store_tokens(url, tokens).await;
    }

    /// Clear all cached tokens
    pub async fn clear_all(&self) {
        if let Ok(mut cache) = self.tokens.lock() {
            cache.clear();
        }
    }
}

/// Global session cache instance
static SESSION_CACHE: OnceLock<Arc<SessionTokenCache>> = OnceLock::new();

/// Get the global session cache instance
pub fn get_session_cache() -> &'static Arc<SessionTokenCache> {
    SESSION_CACHE.get_or_init(|| Arc::new(SessionTokenCache::new()))
}
