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
    Patch,
    Put,
    DeleteWithBody,
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

    pub async fn delete_with_body<I, O>(&self, path: &str, body: I) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::DeleteWithBody, path, Some(body))
            .await
    }

    pub async fn patch<I, O>(&self, path: &str, body: I) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::Patch, path, Some(body)).await
    }

    pub async fn put_json<I, O>(&self, path: &str, body: I) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::Put, path, Some(body)).await
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
                    RequestType::Patch => self.client.patch(url.clone()).json(&body),
                    RequestType::Put => self.client.put(url.clone()).json(&body),
                    RequestType::DeleteWithBody => self.client.delete(url.clone()).json(&body),
                };
                if let Some(h) = auth_header {
                    builder = builder.header("Authorization", h);
                }
                builder.send()
            })
            .await?;

        response.json::<O>().await.map_err(Into::into)
    }

    /// Ensure a valid auth header is available and return it.
    ///
    /// Returns `None` immediately if `requires_auth` is false or `node_name` is unset.
    /// If stored tokens exist they are returned directly.  If no tokens are found,
    /// **this method triggers a full browser-based authentication flow** to obtain fresh
    /// tokens — callers should be aware of this side effect (it may open a browser window).
    async fn load_auth_header(&self, requires_auth: bool) -> Result<Option<String>> {
        if !requires_auth {
            return Ok(None);
        }
        let Some(node_name) = &self.node_name else {
            return Ok(None);
        };

        match self.client_storage.load_tokens(node_name).await {
            Ok(Some(tokens)) => return Ok(Some(format!("Bearer {}", tokens.access_token))),
            Ok(None) => {} // No stored tokens — fall through to proactive auth
            Err(e) => {
                // Surface storage failures so the user can diagnose disk/permission issues
                // rather than silently falling through to a re-authentication prompt.
                bail!("Failed to load stored tokens for '{}': {}", node_name, e);
            }
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

                // Try to refresh first; fall back to full re-auth on any failure
                // (including missing refresh token). Previously, missing refresh token
                // caused an immediate bail; now any refresh failure triggers full
                // re-authentication so the user gets a working session regardless.
                match self.refresh_token().await {
                    Ok(new_token) => {
                        if let Some(ref node_name) = self.node_name {
                            self.client_storage
                                .update_tokens(node_name, &new_token)
                                .await?;
                        }
                    }
                    Err(_) => match self.authenticator.authenticate(&self.api_url).await {
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
                    },
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
                let msg = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| format!("Request failed with status: {status}"));
                bail!("{}", msg);
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

    /// Detect the authentication mode for this connection.
    ///
    /// Probes `admin-api/contexts` (a protected endpoint) and inspects the response:
    /// - `401` → `AuthMode::Required`
    /// - `2xx` → `AuthMode::None` (server responded without demanding auth)
    /// - anything else (including `404`) → `AuthMode::Required` (safe default)
    /// - network error → `AuthMode::None` (node unreachable / no admin API)
    ///
    /// Note: unlike the previous implementation, **localhost / 127.0.0.1 no longer
    /// bypasses the probe**. Local nodes now go through the same detection so that
    /// locally-deployed nodes with auth enabled are handled correctly.
    pub async fn detect_auth_mode(&self) -> Result<AuthMode> {
        // Probe a protected endpoint — if it returns 401, auth is required.
        // admin-api/health is intentionally public, so we probe a protected endpoint instead.
        let probe_url = self.api_url.join("admin-api/contexts")?;

        match self.client.get(probe_url).send().await {
            Ok(response) => {
                if response.status() == 401 {
                    // 401 Unauthorized means authentication is required
                    Ok(AuthMode::Required)
                } else if response.status().is_success() {
                    // 2xx without auth challenge means no authentication required.
                    // Note: 404 is intentionally excluded — a protected endpoint can return 404
                    // (e.g., empty contexts list) while still requiring auth for mutations.
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
/// Only extracts from known-safe structured fields (`error.message`, `error`, `message`).
/// Falls back to "HTTP {status}" rather than including raw body text to avoid
/// leaking sensitive server-internal details.  All extracted strings are capped at
/// 300 characters for consistency.
fn extract_error_message(body: &str, status: reqwest::StatusCode) -> String {
    let trimmed = body.trim();

    if trimmed.is_empty() {
        return format!("HTTP {}", status.as_u16());
    }

    const MAX_LEN: usize = 300;

    let truncate = |s: &str| -> String {
        if s.chars().count() > MAX_LEN {
            format!("{}…", s.chars().take(MAX_LEN).collect::<String>())
        } else {
            s.to_owned()
        }
    };

    // Try to parse as JSON and extract a meaningful message from known-safe fields.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        // { "error": { "message": "..." } }
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return format!("HTTP {}: {}", status.as_u16(), truncate(msg));
        }
        // { "error": "..." }
        if let Some(msg) = json.get("error").and_then(|m| m.as_str()) {
            return format!("HTTP {}: {}", status.as_u16(), truncate(msg));
        }
        // { "message": "..." }
        if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
            return format!("HTTP {}: {}", status.as_u16(), truncate(msg));
        }
    }

    // Non-JSON or no known error field — return just the status code to avoid body leakage.
    format!("HTTP {}", status.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_body() {
        let s = reqwest::StatusCode::INTERNAL_SERVER_ERROR;
        assert_eq!(extract_error_message("", s), "HTTP 500");
        assert_eq!(extract_error_message("   ", s), "HTTP 500");
    }

    #[test]
    fn test_json_nested_error_message() {
        let s = reqwest::StatusCode::BAD_REQUEST;
        let body = r#"{"error":{"message":"invalid request"}}"#;
        assert_eq!(extract_error_message(body, s), "HTTP 400: invalid request");
    }

    #[test]
    fn test_json_flat_error_string() {
        let s = reqwest::StatusCode::UNAUTHORIZED;
        let body = r#"{"error":"unauthorized"}"#;
        assert_eq!(extract_error_message(body, s), "HTTP 401: unauthorized");
    }

    #[test]
    fn test_json_message_field() {
        let s = reqwest::StatusCode::NOT_FOUND;
        let body = r#"{"message":"not found"}"#;
        assert_eq!(extract_error_message(body, s), "HTTP 404: not found");
    }

    #[test]
    fn test_invalid_json_returns_status() {
        let s = reqwest::StatusCode::INTERNAL_SERVER_ERROR;
        assert_eq!(extract_error_message("not valid json {", s), "HTTP 500");
    }

    #[test]
    fn test_json_no_known_fields_returns_status() {
        let s = reqwest::StatusCode::BAD_REQUEST;
        let body = r#"{"data":null,"code":42}"#;
        assert_eq!(extract_error_message(body, s), "HTTP 400");
    }

    #[test]
    fn test_long_json_message_truncated() {
        let s = reqwest::StatusCode::BAD_REQUEST;
        let long_msg = "x".repeat(400);
        let body = format!(r#"{{"message":"{}"}}"#, long_msg);
        let result = extract_error_message(&body, s);
        // "HTTP 400: " (10 chars) + 300 x's + "…" (1 char) = 312 chars
        assert!(result.starts_with("HTTP 400: "));
        assert!(result.ends_with('…'));
        // The message portion should be truncated to 300 chars + ellipsis
        let msg_part = result.strip_prefix("HTTP 400: ").unwrap();
        assert_eq!(msg_part.chars().filter(|&c| c == 'x').count(), 300);
    }
}
