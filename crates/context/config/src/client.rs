use std::borrow::Cow;
use std::fmt::Debug;
use std::ops::Deref;

use either::Either;
use env::Method;
use thiserror::Error;

pub mod config;
pub mod env;
pub mod protocol;
pub mod relayer;
pub mod transport;
pub mod utils;

use config::{ClientConfig, ClientSelectedSigner, Credentials, LocalConfig};
use protocol::{icp, near, starknet, Protocol};
use transport::{Both, Transport, TransportArguments, TransportRequest, UnsupportedProtocol};

pub type LocalTransports = Both<
    near::NearTransport<'static>,
    Both<starknet::StarknetTransport<'static>, icp::IcpTransport<'static>>,
>;

pub type AnyTransport = Both<LocalTransports, relayer::RelayerTransport>;

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
        // Initialize relayer transport
        let relayer = relayer::RelayerTransport::new(&relayer::RelayerConfig {
            url: config.signer.relayer.url.clone(),
        });

        // Initialize local transport
        let local = Self::from_local_config(&config.signer.local)
            .expect("validation error")
            .transport;

        // Create Both structure containing both transports
        let transport = Both {
            left: local,
            right: relayer,
        };

        Self::new(transport)
    }

    pub fn from_local_config(config: &LocalConfig) -> eyre::Result<Client<LocalTransports>> {
        let near_transport = near::NearTransport::new(&near::NearConfig {
            networks: config
                .near
                .iter()
                .map(|(network, config)| {
                    let (account_id, secret_key) = match &config.credentials {
                        Credentials::Near(credentials) => (
                            credentials.account_id.clone(),
                            credentials.secret_key.clone(),
                        ),
                        Credentials::Starknet(_) | Credentials::Icp(_) => {
                            eyre::bail!(
                                "Expected Near credentials but got {:?}",
                                config.credentials
                            )
                        }
                    };
                    Ok((
                        network.clone().into(),
                        near::NetworkConfig {
                            rpc_url: config.rpc_url.clone(),
                            account_id,
                            access_key: secret_key,
                        },
                    ))
                })
                .collect::<eyre::Result<_>>()?,
        });

        let starknet_transport = starknet::StarknetTransport::new(&starknet::StarknetConfig {
            networks: config
                .starknet
                .iter()
                .map(|(network, config)| {
                    let (account_id, secret_key) = match &config.credentials {
                        Credentials::Starknet(credentials) => {
                            (credentials.account_id, credentials.secret_key)
                        }
                        Credentials::Near(_) | Credentials::Icp(_) => {
                            eyre::bail!(
                                "Expected Starknet credentials but got {:?}",
                                config.credentials
                            )
                        }
                    };
                    Ok((
                        network.clone().into(),
                        starknet::NetworkConfig {
                            rpc_url: config.rpc_url.clone(),
                            account_id,
                            access_key: secret_key,
                        },
                    ))
                })
                .collect::<eyre::Result<_>>()?,
        });

        let icp_transport = icp::IcpTransport::new(&icp::IcpConfig {
            networks: config
                .icp
                .iter()
                .map(|(network, config)| {
                    let (account_id, secret_key) = match &config.credentials {
                        Credentials::Icp(credentials) => (
                            credentials.account_id.clone(),
                            credentials.secret_key.clone(),
                        ),
                        Credentials::Near(_) | Credentials::Starknet(_) => {
                            eyre::bail!("Expected ICP credentials but got {:?}", config.credentials)
                        }
                    };
                    Ok((
                        network.clone().into(),
                        icp::NetworkConfig {
                            rpc_url: config.rpc_url.clone(),
                            account_id,
                            secret_key,
                        },
                    ))
                })
                .collect::<eyre::Result<_>>()?,
        });

        let all_transports = Both {
            left: near_transport,
            right: Both {
                left: starknet_transport,
                right: icp_transport,
            },
        };

        Ok(Client::new(all_transports))
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ClientError<T: Transport> {
    #[error("transport error: {0}")]
    Transport(T::Error),
    #[error("codec error: {0}")]
    Codec(#[from] eyre::Report),
    #[error(
        "unsupported protocol: `{found}`, expected {}",
        utils::humanize_iter(expected.deref())
    )]
    UnsupportedProtocol {
        found: String,
        expected: Cow<'static, [Cow<'static, str>]>,
    },
}

impl<'a, T: Transport> From<UnsupportedProtocol<'a>> for ClientError<T> {
    fn from(err: UnsupportedProtocol<'a>) -> Self {
        Self::UnsupportedProtocol {
            found: err.args.protocol.into_owned(),
            expected: err.expected,
        }
    }
}

impl<T: Transport> Client<T> {
    async fn send(
        &self,
        protocol: Cow<'_, str>,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, ClientError<T>> {
        let res: Result<_, _> = self
            .transport
            .try_send(TransportArguments {
                protocol,
                request,
                payload,
            })
            .await
            .into();

        res?.map_err(ClientError::Transport)
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

impl<T: Transport> Transport for Client<T> {
    type Error = T::Error;

    async fn try_send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, Self::Error>, UnsupportedProtocol<'a>> {
        self.transport.try_send(args).await
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
            network_id: Cow::Borrowed(&self.network_id),
            contract_id: Cow::Borrowed(&self.contract_id),
            operation,
        };

        let response = self
            .client
            .send(self.protocol.as_ref().into(), request, payload)
            .await?;

        M::decode(response).map_err(ClientError::Codec)
    }
}

pub trait Environment<'a, T> {
    type Query;
    type Mutate;

    fn query(client: CallClient<'a, T>) -> Self::Query;
    fn mutate(client: CallClient<'a, T>) -> Self::Mutate;
}
