use chrono::Utc;
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::identity::Keypair;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::auth::AuthManager;
use crate::cli::ApiError;
use crate::common::RequestType;

/// Authentication method for requests
#[derive(Debug, Clone)]
pub enum AuthMethod {
    /// No authentication (development mode)
    None,
    /// Legacy keypair-based authentication
    Keypair(Keypair),
    /// JWT token authentication
    JWT(String),
}

pub struct ConnectionInfo {
    pub api_url: Url,
    pub auth_method: AuthMethod,
    pub client: Client,
    pub auth_manager: Option<AuthManager>,
}

impl ConnectionInfo {
    pub async fn new(api_url: Url, auth_key: Option<Keypair>) -> Self {
        let auth_method = match auth_key {
            Some(keypair) => AuthMethod::Keypair(keypair),
            None => AuthMethod::None,
        };

        Self {
            api_url,
            auth_method,
            client: Client::new(),
            auth_manager: None,
        }
    }

    pub async fn new_with_jwt(api_url: Url, token: String) -> Self {
        Self {
            api_url,
            auth_method: AuthMethod::JWT(token),
            client: Client::new(),
            auth_manager: None,
        }
    }

    pub async fn new_with_auth_manager(api_url: Url, auth_manager: AuthManager) -> Self {
        Self {
            api_url,
            auth_method: AuthMethod::None, // Will be set when auth is needed
            client: Client::new(),
            auth_manager: Some(auth_manager),
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
        // Try the request first
        match self.try_request(req_type, path, &body).await {
            Ok(response) => {
                if response.status().is_success() {
                    return Ok(response.json().await?);
                }
                
                // Handle 401 responses with automatic authentication
                if response.status() == 401 {
                    if let Some(auth_manager) = &self.auth_manager {
                        // Try to get a valid token
                        if let Ok(Some(token)) = auth_manager.get_valid_token().await {
                            // Update auth method and retry
                            let updated_self = self.clone_with_token(token);
                            return updated_self.try_request(req_type, path, &body).await?
                                .json().await
                                .map_err(Into::into);
                        } else {
                            // Need to authenticate - this will trigger browser auth
                            return Err(eyre!("Authentication required. Please run: meroctl auth login"));
                        }
                    }
                }
                
                // Handle other error responses
                bail!(ApiError {
                    status_code: response.status().as_u16(),
                    message: response
                        .text()
                        .await
                        .map_err(|e| eyre!("Failed to get response text: {e}"))?,
                });
            }
            Err(e) => Err(e),
        }
    }

    async fn try_request<I>(
        &self,
        req_type: RequestType,
        path: &str,
        body: &Option<I>,
    ) -> EyreResult<reqwest::Response>
    where
        I: Serialize,
    {
        let mut url = self.api_url.clone();
        url.set_path(path);

        let mut builder = match req_type {
            RequestType::Get => self.client.get(url),
            RequestType::Post => self.client.post(url).json(body),
            RequestType::Delete => self.client.delete(url),
        };

        // Add authentication based on method
        builder = match &self.auth_method {
            AuthMethod::None => builder,
            AuthMethod::Keypair(keypair) => {
                let timestamp = Utc::now().timestamp().to_string();
                let signature = keypair.sign(timestamp.as_bytes())?;
                builder
                    .header("X-Signature", bs58::encode(signature).into_string())
                    .header("X-Timestamp", timestamp)
            }
            AuthMethod::JWT(token) => {
                builder.header("Authorization", format!("Bearer {}", token))
            }
        };

        builder.send().await.map_err(Into::into)
    }

    fn clone_with_token(&self, token: String) -> Self {
        Self {
            api_url: self.api_url.clone(),
            auth_method: AuthMethod::JWT(token),
            client: self.client.clone(),
            auth_manager: self.auth_manager.clone(),
        }
    }
}
