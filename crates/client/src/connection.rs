//! Connection management for Calimero client
//!
//! This module provides the core connection functionality for making
//! authenticated API requests to Calimero services.

// External crates
use eyre::{bail, eyre, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

// Local crate
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

    /// Check if a path requires authentication
    fn path_requires_auth(&self, path: &str) -> bool {
        // Only admin-api/health is public, everything else requires authentication
        !path.starts_with("admin-api/health")
    }

    pub async fn post<I, O>(&self, path: &str, body: I) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::Post, path, Some(body)).await
    }

    pub async fn post_no_body<O: DeserializeOwned>(&self, path: &str) -> Result<O> {
        self.request(RequestType::Post, path, None::<()>).await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(RequestType::Delete, path, None::<()>).await
    }

    pub async fn put_binary(&self, path: &str, data: Vec<u8>) -> Result<reqwest::Response> {
        let mut url = self.api_url.clone();

        if let Some((path_part, query_part)) = path.split_once('?') {
            url.set_path(path_part);
            url.set_query(Some(query_part));
        } else {
            url.set_path(path);
        }

        let requires_auth = self.path_requires_auth(path);

        self.execute_request_with_auth_retry(requires_auth, |auth_header| {
            let mut builder = self.client.put(url.clone()).body(data.clone());
            if let Some(h) = auth_header {
                builder = builder.header("Authorization", h);
            }
            builder.send()
        })
        .await
    }

    pub async fn get_binary(&self, path: &str) -> Result<Vec<u8>> {
        let mut url = self.api_url.clone();

        if let Some((path_part, query_part)) = path.split_once('?') {
            url.set_path(path_part);
            url.set_query(Some(query_part));
        } else {
            url.set_path(path);
        }

        let requires_auth = self.path_requires_auth(path);

        let response = self
            .execute_request_with_auth_retry(requires_auth, |auth_header| {
                let mut builder = self.client.get(url.clone());
                if let Some(h) = auth_header {
                    builder = builder.header("Authorization", h);
                }
                builder.send()
            })
            .await?;

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(Into::into)
    }

    pub async fn head(&self, path: &str) -> Result<reqwest::header::HeaderMap> {
        let mut url = self.api_url.clone();
        url.set_path(path);

        let requires_auth = self.path_requires_auth(path);

        let response = self
            .execute_request_with_auth_retry(requires_auth, |auth_header| {
                let mut builder = self.client.head(url.clone());
                if let Some(h) = auth_header {
                    builder = builder.header("Authorization", h);
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

        let requires_auth = self.path_requires_auth(path);

        let response = self
            .execute_request_with_auth_retry(requires_auth, |auth_header| {
                let mut builder = match req_type {
                    RequestType::Get => self.client.get(url.clone()),
                    RequestType::Post => self.client.post(url.clone()).json(&body),
                    RequestType::Delete => self.client.delete(url.clone()),
                };
                if let Some(h) = auth_header {
                    builder = builder.header("Authorization", h);
                }
                builder.send()
            })
            .await?;

        response.json::<O>().await.map_err(Into::into)
    }

    /// Load a fresh auth header from storage, or authenticate proactively if no tokens exist.
    /// Returns `None` if auth is not required or node_name is unset.
    async fn load_auth_header(&self, requires_auth: bool) -> Result<Option<String>> {
        if !requires_auth || self.node_name.is_none() {
            return Ok(None);
        }
        let node_name = self.node_name.as_ref().unwrap();

        if let Ok(Some(tokens)) = self.client_storage.load_tokens(node_name).await {
            return Ok(Some(format!("Bearer {}", tokens.access_token)));
        }

        // No tokens — authenticate proactively
        match self.authenticator.authenticate(&self.api_url).await {
            Ok(new_tokens) => {
                self.client_storage
                    .update_tokens(node_name, &new_tokens)
                    .await?;
                Ok(Some(format!("Bearer {}", new_tokens.access_token)))
            }
            Err(auth_err) => {
                bail!("Authentication failed: {}", auth_err);
            }
        }
    }

    /// Execute a request with automatic token refresh / re-authentication on 401.
    ///
    /// The closure receives a fresh `Option<String>` auth header on **every** call,
    /// including retries after token refresh. This ensures stale tokens are never
    /// reused across retry attempts.
    async fn execute_request_with_auth_retry<F, Fut>(
        &self,
        requires_auth: bool,
        request_builder: F,
    ) -> Result<reqwest::Response>
    where
        F: Fn(Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
    {
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 2;

        loop {
            // Load a fresh auth header on EVERY iteration so retries use up-to-date tokens.
            let auth_header = self.load_auth_header(requires_auth).await?;

            let response = request_builder(auth_header).await?;

            if response.status() == 401 && retry_count < MAX_RETRIES {
                retry_count += 1;

                // Try to refresh first; fall back to full re-auth on any failure.
                match self.refresh_token().await {
                    Ok(new_token) => {
                        if let Some(ref node_name) = self.node_name {
                            self.client_storage
                                .update_tokens(node_name, &new_token)
                                .await?;
                        }
                    }
                    Err(_) => {
                        match self.authenticator.authenticate(&self.api_url).await {
                            Ok(new_tokens) => {
                                if let Some(ref node_name) = self.node_name {
                                    self.client_storage
                                        .update_tokens(node_name, &new_tokens)
                                        .await?;
                                }
                            }
                            Err(auth_err) => {
                                bail!("Authentication failed: {}", auth_err);
                            }
                        }
                    }
                }
                // Loop back — next iteration loads fresh tokens.
                continue;
            }

            if response.status() == 403 {
                bail!("Access denied — your token may not have sufficient permissions.");
            }

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                bail!("{}", extract_error_message(&body, status));
            }

            return Ok(response);
        }
    }

    async fn refresh_token(&self) -> Result<JwtToken> {
        if let Some(ref node_name) = self.node_name {
            if let Ok(Some(tokens)) = self.client_storage.load_tokens(node_name).await {
                let refresh_token = tokens
                    .refresh_token
                    .clone()
                    .ok_or_else(|| eyre!("No refresh token available"))?;

                return self
                    .try_refresh_token(&tokens.access_token, &refresh_token)
                    .await;
            }
        }

        Err(eyre!("No tokens available to refresh"))
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
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.to_owned(),
        };

        let response = self
            .client
            .post(refresh_url)
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(eyre!(
                "Token refresh failed: {}",
                extract_error_message(&body, status)
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
        // Probe a protected endpoint — if it returns 401, auth is required.
        // admin-api/health is intentionally public, so we probe a protected endpoint instead.
        let probe_url = self.api_url.join("admin-api/contexts")?;

        match self.client.get(probe_url).send().await {
            Ok(response) => {
                if response.status() == 401 {
                    // 401 Unauthorized means authentication is required
                    Ok(AuthMode::Required)
                } else if response.status().is_success() || response.status() == 404 {
                    // 200/404 without auth challenge means no authentication required
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

/// Extract a human-readable error message from an HTTP error response body.
///
/// Tries to parse JSON and look for common error envelope shapes used by the node.
/// Falls back to the raw body text, then to just the status code.
fn extract_error_message(body: &str, status: reqwest::StatusCode) -> String {
    let trimmed = body.trim();

    if trimmed.is_empty() {
        return format!("HTTP {}", status.as_u16());
    }

    // Try to parse as JSON and extract a meaningful message
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        // { "error": { "message": "..." } }
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return msg.to_owned();
        }
        // { "error": "..." }
        if let Some(msg) = json.get("error").and_then(|m| m.as_str()) {
            return msg.to_owned();
        }
        // { "message": "..." }
        if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
            return msg.to_owned();
        }
        // { "data": null, "error": { ... } } — already handled above;
        // fall through to raw body
    }

    // Not JSON or no message field — return raw body, truncated to 300 chars
    let text: String = trimmed.chars().take(300).collect();
    text
}
