use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use super::transport::{Operation, Transport, TransportRequest};

#[derive(Debug)]
#[non_exhaustive]
pub struct RelayerConfig {
    pub url: Url,
}

#[derive(Clone, Debug)]
pub struct RelayerTransport {
    client: reqwest::Client,
    url: Url,
}

impl RelayerTransport {
    #[must_use]
    pub fn new(config: &RelayerConfig) -> Self {
        let client = reqwest::Client::new();

        Self {
            client,
            url: config.url.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RelayRequest<'a> {
    pub protocol: Cow<'a, str>,
    pub network_id: Cow<'a, str>,
    pub contract_id: Cow<'a, str>,
    pub operation: Operation<'a>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum RelayerError {
    #[error(transparent)]
    Raw(#[from] reqwest::Error),
    #[error(
        "relayer response ({status}): {}",
        body.is_empty().then_some("<empty>").unwrap_or(body)
    )]
    Response {
        status: reqwest::StatusCode,
        body: String,
    },
}

impl Transport for RelayerTransport {
    type Error = RelayerError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let response = self
            .client
            .post(self.url.clone())
            .json(&RelayRequest {
                protocol: request.protocol,
                network_id: request.network_id,
                contract_id: request.contract_id,
                operation: request.operation,
                payload,
            })
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(RelayerError::Response {
                status: response.status(),
                body: response.text().await?,
            });
        }

        response.bytes().await.map(Into::into).map_err(Into::into)
    }
}
