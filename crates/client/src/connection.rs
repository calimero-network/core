//! Connection management for Calimero client
//!
//! This module provides the core connection functionality for making
//! authenticated API requests to Calimero services.

use eyre::{bail, eyre, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::storage::JwtToken;
use crate::traits::{ClientAuthenticator, ClientStorage};

/// Authentication mode for a connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// Authentication is required for this connection
    Required,
    /// No authentication is required for this connection
    None,
}

// Define RequestType enum locally since it's not available in client
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
        node_name: Option<String>,
        authenticator: A,
        client_storage: S,
    ) -> Self {
        Self {
            api_url,
            client: Client::new(),
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

        // Load tokens from storage before making the request
        let auth_header = if let Some(ref node_name) = self.node_name {
            if let Ok(Some(tokens)) = self.client_storage.load_tokens(node_name).await {
                Some(format!("Bearer {}", tokens.access_token))
            } else {
                None
            }
        } else {
            None
        };

        let response = self
            .execute_request_with_auth_retry(|| {
                let mut builder = self.client.head(url.clone());

                if let Some(ref auth_header) = auth_header {
                    builder = builder.header("Authorization", auth_header);
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

        // Load tokens from storage before making the request
        let auth_header = if let Some(ref node_name) = self.node_name {
            if let Ok(Some(tokens)) = self.client_storage.load_tokens(node_name).await {
                Some(format!("Bearer {}", tokens.access_token))
            } else {
                None
            }
        } else {
            None
        };

        let response = self
            .execute_request_with_auth_retry(|| {
                let mut builder = match req_type {
                    RequestType::Get => self.client.get(url.clone()),
                    RequestType::Post => self.client.post(url.clone()).json(&body),
                    RequestType::Delete => self.client.delete(url.clone()),
                };

                if let Some(ref auth_header) = auth_header {
                    builder = builder.header("Authorization", auth_header);
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
                        // Update stored tokens
                        if let Some(ref node_name) = self.node_name {
                            self.client_storage
                                .update_tokens(node_name, &new_token)
                                .await?;
                        }
                        continue;
                    }
                    Err(RefreshError::RefreshFailed) => {
                        // Token refresh failed, try full re-authentication
                        match self.authenticator.authenticate(&self.api_url).await {
                            Ok(new_tokens) => {
                                // Update stored tokens
                                if let Some(ref node_name) = self.node_name {
                                    self.client_storage
                                        .update_tokens(node_name, &new_tokens)
                                        .await?;
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
        if let Some(ref node_name) = self.node_name {
            if let Ok(Some(tokens)) = self.client_storage.load_tokens(node_name).await {
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
            return Err(eyre!(
                "Token refresh failed with status: {}",
                response.status()
            ));
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
            self.client_storage
                .update_tokens(node_name, new_tokens)
                .await
        } else {
            // For external connections without a node name, we can't update storage
            // This is expected behavior
            Ok(())
        }
    }

    /// Detect the authentication mode for this connection
    pub async fn detect_auth_mode(&self) -> Result<AuthMode> {
        // Check the admin API health endpoint to determine if authentication is required
        let health_url = self.api_url.join("admin-api/health")?;

        match self.client.get(health_url).send().await {
            Ok(response) => {
                if response.status() == 401 {
                    // 401 Unauthorized means authentication is required
                    Ok(AuthMode::Required)
                } else if response.status().is_success() {
                    // 200 OK means no authentication required
                    Ok(AuthMode::None)
                } else {
                    // Other status codes, assume authentication is required for safety
                    Ok(AuthMode::Required)
                }
            }
            Err(_) => {
                // If we can't reach the endpoint, assume no authentication required
                // This is common for local nodes that don't have the admin API enabled
                Ok(AuthMode::None)
            }
        }
    }
}
