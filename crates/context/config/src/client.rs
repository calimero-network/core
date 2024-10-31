use core::convert::Infallible;
use core::error::Error as CoreError;
use core::marker::PhantomData;
use std::borrow::Cow;
use std::str::FromStr;

use either::Either;
use protocol::Method;
use serde::{Deserialize, Serialize};
use serde_json::Error as JsonError;
use thiserror::Error;

use crate::client::protocol::{near, starknet};
use crate::types::{self};

pub mod config;
pub mod env;
pub mod protocol;
pub mod relayer;

use config::{ClientConfig, ClientSelectedSigner, Credentials};

#[non_exhaustive]
#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Near,
    Starknet,
}

pub enum Error {}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Near => "near",
            Protocol::Starknet => "starknet",
        }
    }
}

#[derive(Debug, Error, Copy, Clone)]
#[error("Failed to parse protocol")]
pub struct ProtocolParseError {
    _priv: (),
}

impl FromStr for Protocol {
    type Err = ProtocolParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.to_lowercase().as_str() {
            "near" => Ok(Protocol::Near),
            "starknet" => Ok(Protocol::Starknet),
            _ => Err(ProtocolParseError { _priv: () }),
        }
    }
}

pub trait Transport {
    type Error: CoreError;

    #[expect(async_fn_in_trait, reason = "Should be fine")]
    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error>;
}

impl<L: Transport, R: Transport> Transport for Either<L, R> {
    type Error = Either<L::Error, R::Error>;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        match self {
            Self::Left(left) => left.send(request, payload).await.map_err(Either::Left),
            Self::Right(right) => right.send(request, payload).await.map_err(Either::Right),
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct TransportRequest<'a> {
    pub protocol: Protocol,
    pub network_id: Cow<'a, str>,
    pub contract_id: Cow<'a, str>,
    pub operation: Operation<'a>,
}

impl<'a> TransportRequest<'a> {
    #[must_use]
    pub const fn new(
        protocol: Protocol,
        network_id: Cow<'a, str>,
        contract_id: Cow<'a, str>,
        operation: Operation<'a>,
    ) -> Self {
        Self {
            protocol,
            network_id,
            contract_id,
            operation,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum Operation<'a> {
    Read { method: Cow<'a, str> },
    Write { method: Cow<'a, str> },
}

pub type AnyTransport = Either<
    relayer::RelayerTransport,
    BothTransport<near::NearTransport<'static>, starknet::StarknetTransport<'static>>,
>;

#[expect(clippy::exhaustive_structs, reason = "this is exhaustive")]
#[derive(Debug, Clone)]
pub struct BothTransport<L, R> {
    pub near: L,
    pub starknet: R,
}

impl<L: Transport, R: Transport> Transport for BothTransport<L, R> {
    type Error = Either<L::Error, R::Error>;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        match request.protocol {
            Protocol::Near => self.near.send(request, payload).await.map_err(Either::Left),
            Protocol::Starknet => self
                .starknet
                .send(request, payload)
                .await
                .map_err(Either::Right),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Client<T> {
    transport: T,
}

impl<T: Transport> Client<T> {
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl Client<AnyTransport> {
    #[must_use]
    pub fn from_config(config: &ClientConfig) -> Self {
        let transport = match config.signer.selected {
            ClientSelectedSigner::Relayer => {
                // If the selected signer is Relayer, use the Left variant.
                Either::Left(relayer::RelayerTransport::new(&relayer::RelayerConfig {
                    url: config.signer.relayer.url.clone(),
                }))
            }

            ClientSelectedSigner::Local => Either::Right(BothTransport {
                near: near::NearTransport::new(&near::NearConfig {
                    networks: config
                        .signer
                        .local
                        .near
                        .iter()
                        .map(|(network, config)| {
                            let (account_id, secret_key) = match &config.credentials {
                                Credentials::Near(credentials) => (
                                    credentials.account_id.clone(),
                                    credentials.secret_key.clone(),
                                ),
                                Credentials::Starknet(_) => {
                                    panic!("Expected Near credentials but got something else.")
                                }
                            };
                            (
                                network.clone().into(),
                                near::NetworkConfig {
                                    rpc_url: config.rpc_url.clone(),
                                    account_id,
                                    access_key: secret_key,
                                },
                            )
                        })
                        .collect(),
                }),
                starknet: starknet::StarknetTransport::new(&starknet::StarknetConfig {
                    networks: config
                        .signer
                        .local
                        .starknet
                        .iter()
                        .map(|(network, config)| {
                            let (account_id, secret_key) = match &config.credentials {
                                Credentials::Starknet(credentials) => {
                                    (credentials.account_id, credentials.secret_key)
                                }
                                Credentials::Near(_) => {
                                    panic!("Expected Starknet credentials but got something else.")
                                }
                            };
                            (
                                network.clone().into(),
                                starknet::NetworkConfig {
                                    rpc_url: config.rpc_url.clone(),
                                    account_id,
                                    access_key: secret_key,
                                },
                            )
                        })
                        .collect(),
                }),
            }),
        };

        Self::new(transport)
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError<T: Transport> {
    #[error("transport error: {0}")]
    Transport(T::Error),
    #[error(transparent)]
    Other(#[from] types::ConfigError<Infallible>),
}

#[derive(Debug)]
pub struct Response<T> {
    bytes: Vec<u8>,
    _priv: PhantomData<T>,
}

impl<T> Response<T> {
    const fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            _priv: PhantomData,
        }
    }

    pub fn parse<'a>(&'a self) -> Result<T, JsonError>
    where
        T: Deserialize<'a>,
    {
        serde_json::from_slice(&self.bytes)
    }
}

#[derive(Debug)]
pub struct CallClient<'a, T> {
    protocol: Protocol,
    network_id: String,
    contract_id: String,
    client: &'a Client<T>,
}

impl<'a, T: Transport> CallClient<'a, T> {
    async fn query<M: Method<P>, P>(&self, params: P) -> Result<M::Returns, ConfigError<T>> {
        let payload = M::encode(&params).map_err(Error::from)?;

        let request = TransportRequest {
            protocol: self.protocol,
            network_id: Cow::Borrowed(&self.network_id),
            contract_id: Cow::Borrowed(&self.contract_id),
            operation: Operation::Read {
                method: Cow::Borrowed(M::METHOD),
            },
        };

        let response = self
            .client
            .transport
            .send(request, payload)
            .await
            .map_err(ConfigError::Transport)?;

        let response_decoded = M::decode(response.as_ref())?;

        Ok(response_decoded)
    }

    async fn mutate<M: Method<P>, P>(&self, params: P) -> Result<M::Returns, Error> {
        let payload = M::encode(&params).map_err(Error::from)?;

        let request = TransportRequest {
            protocol: self.protocol,
            network_id: Cow::Borrowed(&self.network_id),
            contract_id: Cow::Borrowed(&self.contract_id),
            operation: Operation::Read {
                method: Cow::Borrowed(M::METHOD),
            },
        };

        let response = self
            .client
            .transport
            .send(request, payload)
            .await
            .map_err(ConfigError::Transport)?;

        let response_decoded = M::decode(response.as_ref())?;

        Ok(response_decoded)
    }
}

impl<T> Client<T> {
    pub fn query<'a, E: Environment<'a, T>>(
        &'a self,
        protocol: Protocol,
        network_id: String,
        contract_id: String,
    ) -> E::Query {
        E::query(CallClient {
            protocol,
            network_id,
            contract_id,
            client: self,
        })
    }

    pub fn mutate<'a, E: Environment<'a, T>>(
        &'a self,
        protocol: Protocol,
        network_id: String,
        contract_id: String,
    ) -> E::Mutate {
        E::mutate(CallClient {
            protocol,
            network_id,
            contract_id,
            client: self,
        })
    }
}

trait Environment<'a, T> {
    type Query;
    type Mutate;

    fn query(client: CallClient<'a, T>) -> Self::Query;
    fn mutate(client: CallClient<'a, T>) -> Self::Mutate;
}
