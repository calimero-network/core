use async_trait::async_trait;
use client::storage::JwtToken as ClientJwtToken;
use client::ClientStorage;
use eyre::Result;
use serde::{Deserialize, Serialize};

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
                crate::config::NodeConnection::Local { jwt_tokens, .. }
                | crate::config::NodeConnection::Remote { jwt_tokens, .. } => {
                    if let Some(tokens) = jwt_tokens {
                        // Convert from client JwtToken to meroctl JwtToken
                        let client_tokens = if let Some(refresh) = &tokens.refresh_token {
                            ClientJwtToken::with_refresh(
                                tokens.access_token.clone(),
                                refresh.clone(),
                            )
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

        // Convert from client JwtToken to meroctl JwtToken
        let meroctl_tokens = JwtToken {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
        };

        // Update the node connection with new tokens
        if let Some(connection) = config.nodes.get_mut(node_name) {
            match connection {
                crate::config::NodeConnection::Local { jwt_tokens, .. }
                | crate::config::NodeConnection::Remote { jwt_tokens, .. } => {
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
                crate::config::NodeConnection::Local { jwt_tokens, .. }
                | crate::config::NodeConnection::Remote { jwt_tokens, .. } => {
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
