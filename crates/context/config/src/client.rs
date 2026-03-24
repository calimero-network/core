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

use config::ClientConfig;
#[cfg(feature = "near_client")]
use config::{ClientSelectedSigner, Credentials};
use env::Method;
use macros::transport;
#[cfg(feature = "near_client")]
use protocol::near;
use protocol::Protocol;
#[cfg(not(feature = "near_client"))]
use transport::EmptyNearSlot;
use transport::{Both, Transport, TransportArguments, TransportRequest, UnsupportedProtocol};

#[cfg(feature = "near_client")]
type MaybeNear = Option<near::NearTransport<'static>>;
#[cfg(not(feature = "near_client"))]
type MaybeNear = crate::client::transport::EmptyNearSlot;

transport! {
    pub type LocalTransports = (
        MaybeNear
    );
}

transport! {
    pub type AnyTransport = (
        LocalTransports,
        Option<relayer::RelayerTransport>
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
        let relayer = config.signer.relayer.as_ref().map(|r| {
            relayer::RelayerTransport::new(&relayer::RelayerConfig { url: r.url.clone() })
        });

        let local = Self::from_local_config(&config).expect("validation error");

        let transport = transport!(local.transport, relayer);

        Self::new(transport)
    }

    pub fn from_local_config(config: &ClientConfig) -> eyre::Result<Client<LocalTransports>> {
        #[cfg(feature = "near_client")]
        {
            let mut near_transport = None;

            'skipped: {
                if let Some(near_config) = config.signer.local.protocols.get("near") {
                    let Some(e) = config.params.get("near") else {
                        eyre::bail!("missing config specification for `{}` signer", "near");
                    };

                    if !matches!(e.signer, ClientSelectedSigner::Local) {
                        break 'skipped;
                    }

                    let mut near_cfg = near::NearConfig {
                        networks: Default::default(),
                    };

                    for (network, signer) in &near_config.signers {
                        let Credentials::Near(credentials) = &signer.credentials;

                        let _ignored = near_cfg.networks.insert(
                            network.clone().into(),
                            near::NetworkConfig {
                                rpc_url: signer.rpc_url.clone(),
                                account_id: credentials.account_id.clone(),
                                access_key: credentials.secret_key.clone(),
                            },
                        );
                    }

                    near_transport = Some(near::NearTransport::new(&near_cfg));
                }
            }

            let all_transports = transport!(near_transport);

            return Ok(Client::new(all_transports));
        }

        #[cfg(not(feature = "near_client"))]
        {
            if !config.signer.local.protocols.is_empty() {
                eyre::bail!(
                    "NEAR protocol blocks in context client config require the `near_client` \
                     Cargo feature on `calimero-context-config` (e.g. enable the `client` feature)"
                );
            }

            let all_transports = transport!(EmptyNearSlot::default());

            return Ok(Client::new(all_transports));
        }
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

// `client-base` / `client`: `cargo test -p calimero-context-config --features client-base`
#[cfg(all(test, feature = "client-base"))]
mod from_config_tests {
    use super::Client;
    use crate::client::config::{ClientConfig, ClientSigner, LocalConfig};

    #[test]
    fn from_config_accepts_missing_relayer_signer() {
        let config = ClientConfig {
            params: Default::default(),
            signer: ClientSigner {
                relayer: None,
                local: LocalConfig {
                    protocols: Default::default(),
                },
            },
        };

        let _ = Client::from_config(&config);
    }
}
