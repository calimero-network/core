use std::borrow::Cow;
use std::fmt::Debug;

use either::Either;
use env::Method;
use thiserror::Error;

pub mod config;
pub mod env;
pub mod protocol;
pub mod relayer;
pub mod transport;

use config::{ClientConfig, ClientSelectedSigner, Credentials};
use protocol::{near, starknet, Protocol};
use transport::{Both, Transport, TransportRequest};

pub type AnyTransport = Either<
    relayer::RelayerTransport,
    Both<near::NearTransport<'static>, starknet::StarknetTransport<'static>>,
>;

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

            ClientSelectedSigner::Local => Either::Right(Both {
                left: near::NearTransport::new(&near::NearConfig {
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
                right: starknet::StarknetTransport::new(&starknet::StarknetConfig {
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
pub enum ClientError<T: Transport> {
    #[error("transport error: {0}")]
    Transport(T::Error),
    #[error("codec error: {0}")]
    Codec(#[from] eyre::Report),
    #[error("unsupported protocol: {0}")]
    UnsupportedProtocol(String),
}

impl<T: Transport> Client<T> {
    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, T::Error> {
        println!("send {:?}", request);
        self.transport.send(request, payload).await
    }

    pub fn query<'a, E: Environment<'a, T>>(
        &'a self,
        protocol: Cow<'a, str>,
        network_id: Cow<'a, str>,
        contract_id: Cow<'a, str>,
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
        protocol: Cow<'a, str>,
        network_id: Cow<'a, str>,
        contract_id: Cow<'a, str>,
    ) -> E::Mutate {
        E::mutate(CallClient {
            protocol,
            network_id,
            contract_id,
            client: self,
        })
    }
}

#[derive(Debug)]
pub struct CallClient<'a, T> {
    protocol: Cow<'a, str>,
    network_id: Cow<'a, str>,
    contract_id: Cow<'a, str>,
    client: &'a Client<T>,
}

#[derive(Debug)]
pub enum Operation<M> {
    Read(M),
    Write(M),
}

impl<'a, T: Transport> CallClient<'a, T> {
    async fn send<P, M: Method<P>>(
        &self,
        params: Operation<M>,
    ) -> Result<M::Returns, ClientError<T>>
    where
        P: Protocol,
    {
        let method = Cow::Borrowed(M::METHOD);

        let (operation, payload) = match params {
            Operation::Read(params) => (transport::Operation::Read { method }, params.encode()?),
            Operation::Write(params) => (transport::Operation::Write { method }, params.encode()?),
        };

        let request = TransportRequest {
            protocol: Cow::Borrowed(&self.protocol),
            network_id: Cow::Borrowed(&self.network_id),
            contract_id: Cow::Borrowed("vukiproxy.testnet"),
            operation,
        };

        println!("send req {:?}", request);

        let response = self
            .client
            .send(request, payload)
            .await
            .map_err(ClientError::Transport)?;

        println!("send response {:?}", response);

        M::decode(response).map_err(ClientError::Codec)
    }
}

pub trait Environment<'a, T> {
    type Query;
    type Mutate;

    fn query(client: CallClient<'a, T>) -> Self::Query;
    fn mutate(client: CallClient<'a, T>) -> Self::Mutate;
}
