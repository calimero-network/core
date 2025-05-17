use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::Deref;
use std::str::FromStr;

use alloy::signers::local::PrivateKeySigner;
use eyre::Context;
use thiserror::Error;

pub mod config;
pub mod env;
mod macros;
pub mod protocol;
pub mod relayer;
pub mod transport;
pub mod utils;

use config::{ClientConfig, ClientSelectedSigner};
use env::Method;
use macros::transport;
use protocol::{ethereum, icp, near, starknet, stellar, Protocol};
use transport::{Both, Transport, TransportArguments, TransportRequest, UnsupportedProtocol};

type MaybeNear = Option<near::NearTransport<'static>>;
type MaybeStarknet = Option<starknet::StarknetTransport<'static>>;
type MaybeIcp = Option<icp::IcpTransport<'static>>;
type MaybeStellar = Option<stellar::StellarTransport<'static>>;
type MaybeEthereum = Option<ethereum::EthereumTransport<'static>>;

transport! {
    pub type LocalTransports = (
        MaybeNear,
        MaybeStarknet,
        MaybeIcp,
        MaybeStellar,
        MaybeEthereum
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
        let mut starknet_transport = None;
        let mut ethereum_transport = None;
        let mut icp_transport = None;
        let mut stellar_transport = None;

        'near: {
            let Some(params) = config.params.get("near") else {
                break 'near;
            };

            // Strict local signer check
            if !matches!(params.signer, ClientSelectedSigner::Local) {
                break 'near;
            }

            let near_config = config.signer.local.protocols.get("near").ok_or_else(|| {
                eyre::eyre!("Local signer selected for Near but no configuration found")
            })?;

            let mut network_config = near::NearConfig {
                networks: BTreeMap::new(),
            };

            for (network, signer) in &near_config.signers {
                // Direct credential access
                let credentials = signer.near_credentials.as_ref().ok_or_else(|| {
                    eyre::eyre!("Near credentials missing for network {}", network)
                })?;

                // Handle public key redundancy
                let public_key = credentials.public_key.to_string();

                let _ignored = network_config.networks.insert(
                    network.clone().into(),
                    near::NetworkConfig {
                        rpc_url: signer.rpc_url.clone(),
                        account_id: credentials.account_id.clone(),
                        public_key: Some(public_key),
                        access_key: credentials.secret_key.clone(),
                    },
                );
            }

            near_transport = Some(near::NearTransport::new(&network_config));
        }

        'starknet: {
            let Some(params) = config.params.get("starknet") else {
                break 'starknet;
            };

            if !matches!(params.signer, ClientSelectedSigner::Local) {
                break 'starknet;
            }

            let starknet_config =
                config
                    .signer
                    .local
                    .protocols
                    .get("starknet")
                    .ok_or_else(|| {
                        eyre::eyre!("Local signer selected for Starknet but no config found")
                    })?;

            let mut network_config = starknet::StarknetConfig {
                networks: BTreeMap::new(),
            };

            for (network, signer) in &starknet_config.signers {
                let credentials = signer
                    .starknet_credentials
                    .as_ref()
                    .ok_or_else(|| eyre::eyre!("Starknet credentials missing"))?;

                // Convert both values to strings for comparison
                let account_id_str = credentials.account_id.to_string();
                let public_key_str = credentials.public_key.to_string();

                let public_key = if account_id_str == public_key_str {
                    None
                } else {
                    Some(public_key_str)
                };

                let _ignored = network_config.networks.insert(
                    network.clone().into(),
                    starknet::NetworkConfig {
                        rpc_url: signer.rpc_url.clone(),
                        account_id: credentials.account_id.clone(), // Keep as Felt for storage
                        public_key,                                 // Option<String>
                        access_key: credentials.secret_key.clone(),
                    },
                );
            }

            starknet_transport = Some(starknet::StarknetTransport::new(&network_config));
        }

        'ethereum: {
            let Some(params) = config.params.get("ethereum") else {
                break 'ethereum;
            };

            if !matches!(params.signer, ClientSelectedSigner::Local) {
                break 'ethereum;
            }

            let ethereum_config =
                config
                    .signer
                    .local
                    .protocols
                    .get("ethereum")
                    .ok_or_else(|| {
                        eyre::eyre!("Local signer selected for Ethereum but no configuration found")
                    })?;

            let mut network_config = ethereum::EthereumConfig {
                networks: BTreeMap::new(),
            };

            for (network, signer) in &ethereum_config.signers {
                let credentials = signer
                    .ethereum_credentials
                    .as_ref()
                    .ok_or_else(|| eyre::eyre!("Ethereum credentials missing"))?;

                // Convert secret key to signer and derive public key
                let access_key = PrivateKeySigner::from_str(&credentials.secret_key)
                    .wrap_err("Failed to convert Ethereum secret key")?;

                let public_key = access_key.address().to_string();
                let _ignored = network_config.networks.insert(
                    network.clone().into(),
                    ethereum::NetworkConfig {
                        rpc_url: signer.rpc_url.clone(),
                        account_id: credentials.account_id.clone(),
                        access_key,
                        public_key,
                    },
                );
            }

            ethereum_transport = Some(ethereum::EthereumTransport::new(&network_config));
        }

        'icp: {
            let Some(params) = config.params.get("icp") else {
                break 'icp;
            };

            if !matches!(params.signer, ClientSelectedSigner::Local) {
                break 'icp;
            }

            let icp_config = config.signer.local.protocols.get("icp").ok_or_else(|| {
                eyre::eyre!("Local signer selected for ICP but no configuration found")
            })?;

            let mut network_config = icp::IcpConfig {
                networks: BTreeMap::new(),
            };

            for (network, signer) in &icp_config.signers {
                let credentials = signer.icp_credentials.as_ref().ok_or_else(|| {
                    eyre::eyre!("ICP credentials missing for network {}", network)
                })?;

                let _ignored = network_config.networks.insert(
                    network.clone().into(),
                    icp::NetworkConfig {
                        rpc_url: signer.rpc_url.clone(),
                        account_id: credentials.account_id.clone(),
                        secret_key: credentials.secret_key.clone(),
                    },
                );
            }

            icp_transport = Some(icp::IcpTransport::new(&network_config));
        }

        'stellar: {
            let Some(params) = config.params.get("stellar") else {
                break 'stellar;
            };

            if !matches!(params.signer, ClientSelectedSigner::Local) {
                break 'stellar;
            }

            let stellar_config = config
                .signer
                .local
                .protocols
                .get("stellar")
                .ok_or_else(|| {
                    eyre::eyre!("Local signer selected for Stellar but no configuration found")
                })?;

            let mut network_config = stellar::StellarConfig {
                networks: BTreeMap::new(),
            };

            for (network, signer) in &stellar_config.signers {
                let credentials = signer
                    .stellar_credentials
                    .as_ref()
                    .ok_or_else(|| eyre::eyre!("Stellar credentials missing"))?;

                // Stellar uses public_key directly as account identifier
                let public_key = credentials.public_key.clone();

                let _ignored = network_config.networks.insert(
                    network.clone().into(),
                    stellar::NetworkConfig {
                        network: network.clone().into(),
                        rpc_url: signer.rpc_url.clone(),
                        public_key, // Direct String value
                        secret_key: credentials.secret_key.clone(),
                    },
                );
            }

            stellar_transport = Some(stellar::StellarTransport::new(&network_config));
        }

        let all_transports = transport!(
            near_transport,
            starknet_transport,
            icp_transport,
            stellar_transport,
            ethereum_transport
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
