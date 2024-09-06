use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{Operation, Transport, TransportRequest};

#[derive(Debug)]
pub struct RelayerConfig<'a> {
    pub url: Cow<'a, str>,
}

#[derive(Debug)]
pub struct RelayerTransport<'a> {
    client: reqwest::Client,
    url: Cow<'a, str>,
}

impl<'a> RelayerTransport<'a> {
    pub fn new(config: &RelayerConfig<'a>) -> Self {
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

impl Transport for RelayerTransport<'_> {
    type Error = reqwest::Error;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let response = self
            .client
            .post(&*self.url)
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
