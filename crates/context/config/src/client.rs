use std::borrow::Cow;
use std::fmt::Debug;
use std::ops::Deref;

use thiserror::Error;

pub mod config;
pub mod env;
mod macros;
pub mod protocol;
pub mod relayer;
pub mod transport;
pub mod utils;

use config::{ClientConfig, ClientSelectedSigner, Credentials};
use env::Method;
use macros::transport;
use protocol::{mock_relayer, near, Protocol};
use transport::{Both, Transport, TransportArguments, TransportRequest, UnsupportedProtocol};

type MaybeNear = Option<near::NearTransport<'static>>;
type MaybeMockRelayer = Option<mock_relayer::MockRelayerTransport<'static>>;

transport! {
    pub type LocalTransports = (
        MaybeNear,
        MaybeMockRelayer
    );
}

transport! {
    pub type AnyTransport = (
        LocalTransports,
        relayer::RelayerTransport
    );
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
        let relayer = relayer::RelayerTransport::new(&relayer::RelayerConfig {
            url: config.signer.relayer.url.clone(),
        });

        let local = Self::from_local_config(&config).expect("validation error");

        let transport = transport!(local.transport, relayer);

        Self::new(transport)
    }

    pub fn from_local_config(config: &ClientConfig) -> eyre::Result<Client<LocalTransports>> {
        let mut near_transport = None;

        'skipped: {
            if let Some(near_config) = config.signer.local.protocols.get("near") {
                let Some(e) = config.params.get("near") else {
                    eyre::bail!("missing config specification for `{}` signer", "near");
                };

                if !matches!(e.signer, ClientSelectedSigner::Local) {
                    break 'skipped;
                }

                let mut config = near::NearConfig {
                    networks: Default::default(),
                };

                for (network, signer) in &near_config.signers {
                    let Credentials::Near(credentials) = &signer.credentials else {
                        eyre::bail!("expected Near credentials but got {:?}", signer.credentials)
                    };

                    let _ignored = config.networks.insert(
                        network.clone().into(),
                        near::NetworkConfig {
                            rpc_url: signer.rpc_url.clone(),
                            account_id: credentials.account_id.clone(),
                            access_key: credentials.secret_key.clone(),
                        },
                    );
                }

                near_transport = Some(near::NearTransport::new(&config));
            }
        }

        let mut mock_relayer_transport = None;

        'skipped: {
            if let Some(mock_relayer_config) = config.signer.local.protocols.get("mock-relayer") {
                let Some(e) = config.params.get("mock-relayer") else {
                    eyre::bail!(
                        "missing config specification for `{}` signer",
                        "mock-relayer"
                    );
                };

                if !matches!(e.signer, ClientSelectedSigner::Local) {
                    break 'skipped;
                }

                let mut config = mock_relayer::MockRelayerConfig {
                    networks: Default::default(),
                };

                for (network, signer) in &mock_relayer_config.signers {
                    let Credentials::Raw(credentials) = &signer.credentials else {
                        eyre::bail!("expected Raw credentials but got {:?}", signer.credentials)
                    };

                    let _ignored = config.networks.insert(
                        network.clone().into(),
                        mock_relayer::NetworkConfig {
                            rpc_url: signer.rpc_url.clone(),
                            credentials: credentials.clone(),
                        },
                    );
                }

                mock_relayer_transport = Some(mock_relayer::MockRelayerTransport::new(&config));
            }
        }

        let all_transports = transport!(
            near_transport,
            mock_relayer_transport
        );

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
