use std::sync::{Arc, Mutex};

use eyre::{bail, eyre, Result, WrapErr};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::cli::auth::authenticate;
use crate::cli::storage::{get_session_cache, JwtToken};
use crate::cli::ApiError;
use crate::common::RequestType;

#[derive(Debug)]
enum RefreshError {
    NoRefreshToken,
    RefreshFailed,
}

#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    pub api_url: Url,
    pub client: Client,
    pub jwt_tokens: Arc<Mutex<Option<JwtToken>>>,
    pub node_name: Option<String>,
}

impl ConnectionInfo {
    pub fn new(api_url: Url, jwt_tokens: Option<JwtToken>, node_name: Option<String>) -> Self {
        Self {
            api_url,
            client: Client::new(),
            jwt_tokens: Arc::new(Mutex::new(jwt_tokens)),
            node_name,
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

    async fn request<I, O>(&self, req_type: RequestType, path: &str, body: Option<I>) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        let mut url = self.api_url.clone();
        url.set_path(path);

        loop {
            let mut builder = match req_type {
                RequestType::Get => self.client.get(url.clone()),
                RequestType::Post => self.client.post(url.clone()).json(&body),
                RequestType::Delete => self.client.delete(url.clone()),
            };

            if let Some(ref tokens) = *self.jwt_tokens.lock().unwrap() {
                builder =
                    builder.header("Authorization", format!("Bearer {}", tokens.access_token));
            }

            let response = builder.send().await?;

            if response.status() == 401 && self.jwt_tokens.lock().unwrap().is_some() {
                if let Some(auth_error) = response.headers().get("x-auth-error") {
                    println!("Auth error: {}", auth_error.to_str().unwrap_or(""));
                    if auth_error.to_str().unwrap_or("") == "token_expired" {
                        // Try token refresh first, then fall back to full authentication
                        match self.refresh_token().await {
                            Ok(new_tokens) => {
                                // Update the in-memory tokens immediately
                                *self.jwt_tokens.lock().unwrap() = Some(new_tokens.clone());

                                // Update stored tokens based on connection type
                                if let Some(ref node_name) = self.node_name {
                                    // This is a registered node - update config file
                                    Self::update_node_tokens(node_name, &new_tokens).await?;
                                } else {
                                    // This is an external connection - update session cache
                                    let session_cache = get_session_cache();
                                    session_cache.update_tokens(&self.api_url, &new_tokens);
                                }
                                continue;
                            }
                            Err(RefreshError::RefreshFailed) => {
                                // Token refresh failed, try full re-authentication
                                match authenticate(&self.api_url).await {
                                    Ok(new_tokens) => {
                                        // Update the in-memory tokens immediately
                                        *self.jwt_tokens.lock().unwrap() = Some(new_tokens.clone());

                                        // Update stored tokens based on connection type
                                        if let Some(ref node_name) = self.node_name {
                                            // This is a registered node - update config file
                                            Self::update_node_tokens(node_name, &new_tokens)
                                                .await?;
                                        } else {
                                            // This is an external connection - update session cache
                                            let session_cache = get_session_cache();
                                            session_cache.update_tokens(&self.api_url, &new_tokens);
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
                }

                bail!("Authentication required. Please re-add the node or use --api with the URL");
            }

            if response.status() == 403 {
                bail!("Access denied. Your authentication may not have sufficient permissions.");
            }

            if !response.status().is_success() {
                bail!(ApiError {
                    status_code: response.status().as_u16(),
                    message: response
                        .text()
                        .await
                        .map_err(|e| eyre!("Failed to get response text: {e}"))?,
                });
            }

            return response.json::<O>().await.map_err(Into::into);
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
        struct WrappedRefreshResponse {
            data: RefreshResponse,
            error: Option<String>,
        }

        let refresh_request = RefreshRequest {
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.to_owned(),
        };

        let response = self
            .client
            .post(refresh_url)
            .json(&refresh_request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_owned());
            bail!("Failed to refresh token: HTTP {} - {}", status, error_text);
        }

        let wrapped_response: WrappedRefreshResponse = response.json().await?;

        if let Some(error_msg) = wrapped_response.error {
            bail!("Token refresh failed: {}", error_msg);
        }

        Ok(JwtToken {
            access_token: wrapped_response.data.access_token,
            refresh_token: Some(wrapped_response.data.refresh_token),
        })
    }

    pub async fn detect_auth_mode(&self) -> Result<String> {
        let identity_url = self
            .api_url
            .join("/admin-api/health")
            .map_err(|e| eyre!("Failed to construct identity URL: {e}"))?;
        let response = self
            .client
            .get(identity_url)
            .send()
            .await
            .map_err(|e| eyre!("Failed to check authentication mode: {e}"))?;
        if response.status().is_success() {
            #[derive(serde::Deserialize)]
            struct IdentityResponse {
                authentication_mode: Option<String>,
            }

            let data: IdentityResponse = response
                .json()
                .await
                .map_err(|e| eyre!("Failed to parse identity response: {e}"))?;

            Ok(data
                .authentication_mode
                .unwrap_or_else(|| "none".to_owned()))
        } else if response.status() == 401 {
            Ok("required".to_owned())
        } else {
            Ok("none".to_owned())
        }
    }

    /// Update the stored JWT tokens for a specific node in the configuration
    async fn update_node_tokens(node_name: &str, new_tokens: &JwtToken) -> Result<()> {
        let mut config = crate::config::Config::load().await.wrap_err_with(|| {
            format!(
                "Failed to load config while updating tokens for node '{}'",
                node_name
            )
        })?;

        if let Some(node_connection) = config.nodes.get_mut(node_name) {
            match node_connection {
                crate::config::NodeConnection::Remote { jwt_tokens, .. } => {
                    *jwt_tokens = Some(new_tokens.clone());
                    config.save().await.wrap_err_with(|| {
                        format!(
                            "Failed to save config after updating tokens for remote node '{}'",
                            node_name
                        )
                    })?;
                    return Ok(());
                }
                crate::config::NodeConnection::Local { jwt_tokens, .. } => {
                    // Local nodes can also have auth tokens now
                    *jwt_tokens = Some(new_tokens.clone());
                    config.save().await.wrap_err_with(|| {
                        format!(
                            "Failed to save config after updating tokens for local node '{}'",
                            node_name
                        )
                    })?;
                    return Ok(());
                }
            }
        }

        bail!("Node '{}' not found in configuration", node_name)
    }
}
