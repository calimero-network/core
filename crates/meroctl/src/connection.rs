use eyre::{bail, eyre, Result as EyreResult};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::cli::storage::get_storage;
use crate::cli::ApiError;
use crate::common::RequestType;

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("Token refresh failed")]
    RefreshFailed,
    #[error("Token expired and refresh token is not available")]
    NoRefreshToken,
    #[error("Failed to store refreshed token: {0}")]
    StorageError(#[from] eyre::Error),
}

#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    pub api_url: Url,
    pub client: Client,
    pub requires_auth: bool,
}

impl ConnectionInfo {
    pub fn new(api_url: Url, requires_auth: bool) -> Self {
        Self {
            api_url,
            client: Client::new(),
            requires_auth,
        }
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

        loop {
            let mut builder = match req_type {
                RequestType::Get => self.client.get(url.clone()),
                RequestType::Post => self.client.post(url.clone()).json(&body),
                RequestType::Delete => self.client.delete(url.clone()),
            };

            if self.requires_auth {
                let storage = get_storage();
                if let Ok(Some((_profile_name, profile_config))) =
                    storage.get_current_profile().await
                {
                    if let Some(ref token) = profile_config.token {
                        builder = builder
                            .header("Authorization", format!("Bearer {}", token.access_token));
                    }
                }
            }

            let response = builder.send().await?;

            if response.status() == 401 {
                if self.requires_auth {
                    if let Some(auth_error) = response.headers().get("x-auth-error") {
                        if auth_error.to_str().unwrap_or("") == "token_expired" {
                            println!("🔄 Token expired, attempting refresh...");

                            match self.refresh_token().await {
                                Ok(_) => {
                                    println!("✅ Token refreshed successfully");
                                    continue;
                                }
                                Err(e) => match e {
                                    TokenError::NoRefreshToken => {
                                        return Err(eyre!("Authentication required - no refresh token available. Please run 'meroctl auth login --profile <name>'"));
                                    }
                                    TokenError::RefreshFailed => {
                                        println!("❌ Token refresh failed");
                                        return Err(eyre!("Token refresh failed. Please run 'meroctl auth login --profile <name>' to reauthenticate"));
                                    }
                                    TokenError::StorageError(e) => {
                                        println!("❌ Failed to store refreshed token: {}", e);
                                        return Err(eyre!("Failed to store refreshed token. Please check your keychain access and try again"));
                                    }
                                },
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

    async fn refresh_token(&self) -> Result<(), TokenError> {
        let storage = get_storage();

        let (profile_name, profile_config) = storage
            .get_current_profile()
            .await
            .map_err(|e| TokenError::StorageError(e))?
            .ok_or_else(|| TokenError::StorageError(eyre::eyre!("No active profile found")))?;

        let tokens = profile_config
            .token
            .as_ref()
            .ok_or_else(|| TokenError::StorageError(eyre::eyre!("No token found in profile")))?;

        let refresh_token = tokens
            .refresh_token
            .clone()
            .ok_or(TokenError::NoRefreshToken)?;

        match self
            .try_refresh_token(&tokens.access_token, &refresh_token)
            .await
        {
            Ok(new_token) => {
                let updated_config = crate::cli::storage::ProfileConfig {
                    auth_profile: profile_name.clone(),
                    node_url: profile_config.node_url,
                    token: Some(new_token),
                };

                storage
                    .store_profile(&profile_name, &updated_config)
                    .await
                    .map_err(TokenError::StorageError)?;

                Ok(())
            }
            Err(_) => Err(TokenError::RefreshFailed),
        }
    }

    async fn try_refresh_token(
        &self,
        access_token: &str,
        refresh_token: &str,
    ) -> EyreResult<crate::cli::storage::JwtToken> {
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

        Ok(crate::cli::storage::JwtToken {
            access_token: wrapped_response.data.access_token,
            refresh_token: Some(wrapped_response.data.refresh_token),
        })
    }

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
                .unwrap_or_else(|| "none".to_owned()))
        } else if response.status() == 401 {
            Ok("required".to_owned())
        } else {
            Ok("none".to_owned())
        }
    }
}
