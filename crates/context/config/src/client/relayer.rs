#![allow(clippy::multiple_inherent_impl, reason = "it's fine")]

use std::borrow::Cow;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use super::transport::{
    Operation, Transport, TransportArguments, TransportRequest, UnsupportedProtocol,
};

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
    Response { status: StatusCode, body: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum ServerError {
    UnsupportedProtocol {
        found: Cow<'static, str>,
        expected: Cow<'static, [Cow<'static, str>]>,
    },
}

impl RelayerTransport {
    async fn send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, UnsupportedProtocol<'a>>, RelayerError> {
        let request = RelayRequest {
            protocol: args.protocol,
            network_id: args.request.network_id,
            contract_id: args.request.contract_id,
            operation: args.request.operation,
            payload: args.payload,
        };

        let response = self
            .client
            .post(self.url.clone())
            .json(&request)
            .send()
            .await?;

        match response.status() {
            status if status.is_success() => {
                return response
                    .bytes()
                    .await
                    .map(|v| Ok(v.into()))
                    .map_err(|e| e.into());
            }
            status if status == StatusCode::BAD_REQUEST => {}
            status => {
                return Err(RelayerError::Response {
                    status,
                    body: response.text().await?,
                })
            }
        }

        let error = response.json::<ServerError>().await?;

        match error {
            ServerError::UnsupportedProtocol { found: _, expected } => {
                let args = TransportArguments {
                    protocol: request.protocol,
                    request: TransportRequest {
                        network_id: request.network_id,
                        contract_id: request.contract_id,
                        operation: request.operation,
                    },
                    payload: request.payload,
                };

                Ok(Err(UnsupportedProtocol { args, expected }))
            }
        }
    }
}

impl Transport for RelayerTransport {
    type Error = RelayerError;

    async fn try_send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, Self::Error>, UnsupportedProtocol<'a>> {
        self.send(args)
            .await
            .map_or_else(|e| Ok(Err(e)), |v| v.map(Ok))
    }
}
