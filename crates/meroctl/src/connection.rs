use chrono::Utc;
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::identity::Keypair;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::cli::ApiError;
use crate::common::RequestType;

#[derive(Debug)]
pub struct ConnectionInfo {
    pub api_url: Url,
    pub auth_key: Option<Keypair>,
    pub client: Client,
}

impl ConnectionInfo {
    pub async fn new(api_url: Url, auth_key: Option<Keypair>) -> Self {
        Self {
            api_url,
            auth_key,
            client: Client::new(),
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

        let mut builder = match req_type {
            RequestType::Get => self.client.get(url),
            RequestType::Post => self.client.post(url).json(&body),
            RequestType::Delete => self.client.delete(url),
        };

        if let Some(keypair) = &self.auth_key {
            let timestamp = Utc::now().timestamp().to_string();
            let signature = keypair.sign(timestamp.as_bytes())?;

            builder = builder
                .header("X-Signature", bs58::encode(signature).into_string())
                .header("X-Timestamp", timestamp);
        }

        let response = builder.send().await?;

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
}
