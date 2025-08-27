//! Connection management for Calimero client
//! 
//! This module provides the core connection functionality for making
//! authenticated API requests to Calimero services.

use std::sync::{Arc, Mutex};

use eyre::{bail, eyre, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::storage::JwtToken;
use crate::traits::{ClientAuthenticator, ClientStorage};

// Define RequestType enum locally since it's not available in calimero-client
#[derive(Debug, Clone, Copy)]
enum RequestType {
    Get,
    Post,
    Delete,
}

#[derive(Debug)]
enum RefreshError {
    NoRefreshToken,
    RefreshFailed,
}

/// Generic connection information that can work with any authenticator and storage implementation
#[derive(Clone, Debug)]
pub struct ConnectionInfo<A, S> 
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub api_url: Url,
    pub client: Client,
    pub jwt_tokens: Arc<Mutex<Option<JwtToken>>>,
    pub node_name: Option<String>,
    pub authenticator: A,
    pub client_storage: S,
}

impl<A, S> ConnectionInfo<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub fn new(
        api_url: Url,
        jwt_tokens: Option<JwtToken>,
        node_name: Option<String>,
        authenticator: A,
        client_storage: S,
    ) -> Self {
        Self {
            api_url,
            client: Client::new(),
            jwt_tokens: Arc::new(Mutex::new(jwt_tokens)),
            node_name,
            authenticator,
            client_storage,
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(RequestType::Get, path, None::<()>).await
    }

    pub async fn post<I, O>(&self, path: &str, body: I) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::Post, path, Some(body)).await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(RequestType::Delete, path, None::<()>).await
    }

    pub async fn head(&self, path: &str) -> Result<reqwest::header::HeaderMap> {
        let mut url = self.api_url.clone();
        url.set_path(path);

        let response = self
            .execute_request_with_auth_retry(|| {
                let mut builder = self.client.head(url.clone());

                if let Some(ref tokens) = *self.jwt_tokens.lock().unwrap() {
                    builder =
                        builder.header("Authorization", format!("Bearer {}", tokens.access_token));
                }

                builder.send()
            })
            .await?;

        Ok(response.headers().clone())
    }

    async fn request<I, O>(&self, req_type: RequestType, path: &str, body: Option<I>) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        let mut url = self.api_url.clone();
        url.set_path(path);

        let response = self
            .execute_request_with_auth_retry(|| {
                let mut builder = match req_type {
                    RequestType::Get => self.client.get(url.clone()),
                    RequestType::Post => self.client.post(url.clone()).json(&body),
                    RequestType::Delete => self.client.delete(url.clone()),
                };

                if let Some(ref tokens) = *self.jwt_tokens.lock().unwrap() {
                    builder =
                        builder.header("Authorization", format!("Bearer {}", tokens.access_token));
                }

                builder.send()
            })
            .await?;

        response.json::<O>().await.map_err(Into::into)
    }

    async fn execute_request_with_auth_retry<F, Fut>(
        &self,
        request_builder: F,
    ) -> Result<reqwest::Response>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
    {
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 2;

        loop {
            let response = request_builder().await?;

            if response.status() == 401 && retry_count < MAX_RETRIES {
                retry_count += 1;

                // Try to refresh tokens
                match self.refresh_token().await {
                    Ok(new_token) => {
                        // Update the in-memory tokens immediately
                        *self.jwt_tokens.lock().unwrap() = Some(new_token.clone());

                        // Update stored tokens based on connection type
                        if let Some(ref _node_name) = self.node_name {
                            // This is a registered node - update config file
                            self.update_tokens(&new_token).await?;
                        } else {
                            // This is an external connection - update session cache
                            // Note: This would need to be implemented by the concrete storage type
                            // For now, we'll just continue
                        }
                        continue;
                    }
                    Err(RefreshError::RefreshFailed) => {
                        // Token refresh failed, try full re-authentication
                        match self.authenticator.authenticate(&self.api_url).await {
                            Ok(new_tokens) => {
                                // Update the in-memory tokens immediately
                                *self.jwt_tokens.lock().unwrap() = Some(new_tokens.clone());

                                // Update stored tokens based on connection type
                                if let Some(ref _node_name) = self.node_name {
                                    // This is a registered node - update config file
                                    self.update_tokens(&new_tokens).await?;
                                } else {
                                    // This is an external connection - update session cache
                                    // Note: This would need to be implemented by the concrete storage type
                                    // For now, we'll just continue
                                }
                                continue;
                            }
                            Err(auth_err) => {
                                bail!("Authentication failed: {}", auth_err);
                            }
                        }
                    }
                    Err(RefreshError::NoRefreshToken) => {
                        // No refresh token available, don't try re-authentication
                        bail!("No refresh token available for authentication");
                    }
                }
            }

            if response.status() == 403 {
                bail!("Access denied. Your authentication may not have sufficient permissions.");
            }

            if !response.status().is_success() {
                bail!("Request failed with status: {}", response.status());
            }

            return Ok(response);
        }
    }

    async fn refresh_token(&self) -> Result<JwtToken, RefreshError> {
        if let Some(ref tokens) = *self.jwt_tokens.lock().unwrap() {
            let refresh_token = tokens
                .refresh_token
                .clone()
                .ok_or(RefreshError::NoRefreshToken)?;

            match self
                .try_refresh_token(&tokens.access_token, &refresh_token)
                .await
            {
                Ok(new_token) => {
                    return Ok(new_token);
                }
                Err(_) => {
                    return Err(RefreshError::RefreshFailed);
                }
            }
        }

        Err(RefreshError::NoRefreshToken)
    }

    async fn try_refresh_token(&self, access_token: &str, refresh_token: &str) -> Result<JwtToken> {
        let refresh_url = self.api_url.join("/auth/refresh")?;

        #[derive(serde::Serialize)]
        struct RefreshRequest {
            access_token: String,
            refresh_token: String,
        }

        #[derive(serde::Deserialize, Debug)]
        struct RefreshResponse {
            access_token: String,
            refresh_token: String,
        }

        #[derive(serde::Deserialize, Debug)]
        struct WrappedResponse {
            data: RefreshResponse,
        }

        let request_body = RefreshRequest {
            access_token: access_token.to_string(),
            refresh_token: refresh_token.to_string(),
        };

        let response = self
            .client
            .post(refresh_url)
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(eyre!("Token refresh failed with status: {}", response.status()));
        }

        let wrapped_response: WrappedResponse = response.json().await?;

        Ok(JwtToken::with_refresh(
            wrapped_response.data.access_token,
            wrapped_response.data.refresh_token,
        ))
    }

    /// Update the stored JWT tokens using the storage interface
    pub async fn update_tokens(&self, new_tokens: &JwtToken) -> Result<()> {
        if let Some(node_name) = &self.node_name {
            self.client_storage.update_tokens(node_name, new_tokens).await
        } else {
            // For external connections without a node name, we can't update storage
            // This is expected behavior
            Ok(())
        }
    }

    /// Detect the authentication mode for this connection
    pub async fn detect_auth_mode(&self) -> Result<String> {
        // For now, assume all APIs require authentication
        // In a real implementation, this would check the API health endpoint
        Ok("oauth".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{ClientAuthenticator, ClientStorage};
    use crate::storage::JwtToken;
    
    // Mock implementations for testing
    #[derive(Debug, Clone)]
    struct MockAuthenticator;
    
    #[derive(Debug, Clone)]
    struct MockStorage;
    
    #[async_trait::async_trait]
    impl ClientAuthenticator for MockAuthenticator {
        async fn authenticate(&self, _api_url: &Url) -> Result<JwtToken> {
            Ok(JwtToken::new("test_token".to_string()))
        }
        
        async fn refresh_tokens(&self, _refresh_token: &str) -> Result<JwtToken> {
            Ok(JwtToken::new("refreshed_token".to_string()))
        }
        
        async fn handle_auth_failure(&self, _api_url: &Url) -> Result<JwtToken> {
            Ok(JwtToken::new("recovered_token".to_string()))
        }
        
        async fn check_auth_required(&self, _api_url: &Url) -> Result<bool> {
            Ok(true)
        }
        
        fn get_auth_method(&self) -> &'static str {
            "Mock"
        }
        
        fn supports_refresh(&self) -> bool {
            true
        }
    }
    
    #[async_trait::async_trait]
    impl ClientStorage for MockStorage {
        async fn load_tokens(&self, _node_name: &str) -> Result<Option<JwtToken>> {
            Ok(None)
        }
        
        async fn save_tokens(&self, _node_name: &str, _tokens: &JwtToken) -> Result<()> {
            Ok(())
        }
        
        async fn update_tokens(&self, _node_name: &str, _new_tokens: &JwtToken) -> Result<()> {
            Ok(())
        }
        
        async fn remove_tokens(&self, _node_name: &str) -> Result<()> {
            Ok(())
        }
        
        async fn list_nodes(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }
    }
    
    #[test]
    fn test_connection_info_creation() {
        let url = "https://api.test.com".parse().unwrap();
        let authenticator = MockAuthenticator;
        let storage = MockStorage;
        
        let conn = ConnectionInfo::new(
            url.clone(),
            None,
            Some("test-node".to_string()),
            authenticator,
            storage,
        );
        
        assert_eq!(conn.api_url, url);
        assert_eq!(conn.node_name, Some("test-node".to_string()));
    }
}
