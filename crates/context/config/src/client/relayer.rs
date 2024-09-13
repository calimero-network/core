use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use url::Url;

use super::{Operation, Transport, TransportRequest};

#[derive(Debug)]
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
pub struct RelayRequest<'a> {
    pub network_id: Cow<'a, str>,
    pub contract_id: Cow<'a, str>,
    pub operation: Operation<'a>,
    pub payload: Vec<u8>,
}

impl Transport for RelayerTransport {
    type Error = reqwest::Error;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let response = self
            .client
            .post(self.url.clone())
            .json(&RelayRequest {
                network_id: request.network_id,
                contract_id: request.contract_id,
                operation: request.operation,
                payload,
            })
            .send()
            .await?;

        response.bytes().await.map(Into::into)
    }
}
