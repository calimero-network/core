use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{Operation, Transport};

#[derive(Debug)]
pub struct RelayerConfig<'a> {
    url: Cow<'a, str>,
    network: Cow<'a, str>,
    contract_id: Cow<'a, str>,
}

#[derive(Debug)]
pub struct RelayerTransport<'a> {
    client: reqwest::Client,
    url: Cow<'a, str>,
    network: Cow<'a, str>,
    contract_id: Cow<'a, str>,
}

impl<'a> RelayerTransport<'a> {
    pub fn new(config: &RelayerConfig<'a>) -> Self {
        let client = reqwest::Client::new();

        Self {
            client,
            url: config.url.clone(),
            network: config.network.clone(),
            contract_id: config.contract_id.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RelayRequest<'a> {
    network: Cow<'a, str>,
    contract_id: Cow<'a, str>,
    operation: Operation<'a>,
    payload: Vec<u8>,
}

impl Transport for RelayerTransport<'_> {
    type Error = reqwest::Error;

    async fn send(
        &self,
        operation: Operation<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let response = self
            .client
            .post(&*self.url)
            .json(&RelayRequest {
                network: Cow::Borrowed(&self.network),
                contract_id: Cow::Borrowed(&self.contract_id),
                operation,
                payload,
            })
            .send()
            .await?;

        response.bytes().await.map(|bytes| bytes.to_vec())
    }
}
