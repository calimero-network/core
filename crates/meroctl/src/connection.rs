use eyre::{bail, eyre, OptionExt, Result as EyreResult};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::cli::storage::create_storage;
use crate::cli::ApiError;
use crate::common::RequestType;

pub struct ConnectionInfo {
    pub api_url: Url,
    pub client: Client,
    pub profile: Option<String>,
    // Store the current active profile config to avoid keychain accesses
    pub profile_config: Option<crate::cli::storage::ProfileConfig>,
}

impl ConnectionInfo {
    /// Create a new connection with optional profile config
    /// If profile_config is None, this is a no-auth connection
    /// If profile_config is Some, this is an authenticated connection
    pub fn new(
        api_url: Url,
        profile: Option<String>,
        profile_config: Option<crate::cli::storage::ProfileConfig>,
    ) -> Self {
        Self {
            api_url,
            client: Client::new(),
            profile,
            profile_config,
        }
    }

    /// Update the active profile and its config in the connection
    /// This avoids needing to reload from storage when switching profiles
    pub fn set_profile(
        &mut self,
        profile: Option<String>,
        profile_config: Option<crate::cli::storage::ProfileConfig>,
    ) {
        self.profile = profile;
        self.profile_config = profile_config;
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> EyreResult<T> {
        self.request(RequestType::Get, path, None::<()>).await
    }

    pub async fn post<I, O>(&self, path: &str, body: I) -> EyreResult<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::Post, path, Some(body)).await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> EyreResult<T> {
        self.request(RequestType::Delete, path, None::<()>).await
    }

    async fn request<I, O>(
        &self,
        req_type: RequestType,
        path: &str,
        body: Option<I>,
    ) -> EyreResult<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        let mut url = self.api_url.clone();
        url.set_path(path);

        // Build the request
        let mut builder = match req_type {
            RequestType::Get => self.client.get(url),
            RequestType::Post => self.client.post(url).json(&body),
            RequestType::Delete => self.client.delete(url),
        };

        // Add JWT auth header if available from cached profile config
        if let Some(ref profile_config) = self.profile_config {
            if let Some(ref token) = profile_config.token {
                builder = builder.header("Authorization", format!("Bearer {}", token.access_token));
            }
        }

        // Execute the request
        let response = builder.send().await?;

        // Handle authentication errors
        if response.status() == 401 {
            // Check if this is a token expiration error
            if let Some(auth_error) = response.headers().get("x-auth-error") {
                if auth_error.to_str().unwrap_or("") == "token_expired" {
                    println!("🔄 Token expired, attempting refresh...");
                    // Try to refresh the token
                    match self.refresh_token().await {
                        Ok(new_token) => {
                            println!("✅ Token refreshed successfully");
                            // Retry the request with the new token
                            let mut retry_url = self.api_url.clone();
                            retry_url.set_path(path);

                            let mut retry_builder = match req_type {
                                RequestType::Get => self.client.get(retry_url),
                                RequestType::Post => self.client.post(retry_url).json(&body),
                                RequestType::Delete => self.client.delete(retry_url),
                            };

                            retry_builder = retry_builder
                                .header("Authorization", format!("Bearer {}", new_token));

                            let retry_response = retry_builder.send().await?;

                            if retry_response.status().is_success() {
                                return retry_response.json::<O>().await.map_err(Into::into);
                            }
                            // If retry also fails, fall through to normal error handling
                        }
                        Err(refresh_err) => {
                            println!("❌ Token refresh failed: {}", refresh_err);
                            // Fall through to normal error handling
                        }
                    }
                }
            }

            return Err(eyre!(
                "Authentication required. Please run 'meroctl auth login --profile <name>'"
            ));
        }

        if response.status() == 403 {
            return Err(eyre!(
                "Access denied. Your authentication may not have sufficient permissions."
            ));
        }

        // Handle other errors
        if !response.status().is_success() {
            bail!(ApiError {
                status_code: response.status().as_u16(),
                message: response
                    .text()
                    .await
                    .map_err(|e| eyre!("Failed to get response text: {e}"))?,
            });
        }

        response.json::<O>().await.map_err(Into::into)
    }

    /// Refresh an expired access token using the refresh token
    async fn refresh_token(&self) -> EyreResult<String> {
        // Use the cached profile config to get the refresh token
        let profile_config = self
            .profile_config
            .as_ref()
            .ok_or_eyre("Profile not found")?;

        let tokens = profile_config
            .token
            .as_ref()
            .ok_or_eyre("No token found in profile")?;

        // Call the refresh endpoint
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

        let refresh_request = RefreshRequest {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone().unwrap_or_default(),
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
                .unwrap_or_else(|_| "Unknown error".to_string());
            bail!("Failed to refresh token: HTTP {} - {}", status, error_text);
        }

        let refresh_response: RefreshResponse = response.json().await?;

        // Update the stored tokens
        let new_jwt_token = crate::cli::storage::JwtToken {
            access_token: refresh_response.access_token.clone(),
            refresh_token: Some(refresh_response.refresh_token),
        };

        let updated_config = crate::cli::storage::ProfileConfig {
            node_url: profile_config.node_url.clone(),
            token: Some(new_jwt_token),
        };

        // Store in storage (note: cached config won't be updated until next request, but that's ok)
        if let Some(ref profile_name) = self.profile {
            let storage = create_storage();
            storage.store_profile(profile_name, &updated_config).await?;
        }

        Ok(refresh_response.access_token)
    }

    /// Detect authentication mode for this connection
    pub async fn detect_auth_mode(&self) -> EyreResult<String> {
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
                .unwrap_or_else(|| "none".to_string()))
        } else if response.status() == 401 {
            // 401 means authentication is required
            Ok("required".to_string())
        } else {
            // Other errors (404, 500, etc.) - assume no auth required (dev mode)
            Ok("none".to_string())
        }
    }
}
