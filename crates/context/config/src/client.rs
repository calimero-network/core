use std::borrow::Cow;
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
use protocol::{ethereum, icp, near, starknet, Protocol};
use transport::{Both, Transport, TransportArguments, TransportRequest, UnsupportedProtocol};

type MaybeNear = Option<near::NearTransport<'static>>;
type MaybeStarknet = Option<starknet::StarknetTransport<'static>>;
type MaybeIcp = Option<icp::IcpTransport<'static>>;
type MaybeEthereum = Option<ethereum::EthereumTransport<'static>>;

transport! {
    pub type LocalTransports = (
        MaybeNear,
        MaybeStarknet,
        MaybeIcp,
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

        'skipped: {
            let Some(protocol_config) = config.params.get("near") else {
                break 'skipped;
            };

            if !matches!(protocol_config.signer, ClientSelectedSigner::Local) {
                break 'skipped;
            }

            let Some(near_config) = config.signer.local.protocols.get("near") else {
                eyre::bail!("NEAR signer is set to 'local' but no local NEAR credentials found");
            };

            if near_config.signers.is_empty() {
                eyre::bail!("NEAR signer is set to 'local' but no NEAR credentials provided");
            }

            let mut config = near::NearConfig {
                networks: Default::default(),
            };

            for (network, signer) in &near_config.signers {
                let credentials: near::Credentials = serde_json::from_value(signer.credentials.clone())
                    .map_err(|e| eyre::eyre!("Failed to parse NEAR credentials: {}", e))?;

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

        let mut starknet_transport = None;

        'skipped: {
            let Some(protocol_config) = config.params.get("starknet") else {
                break 'skipped;
            };

            if !matches!(protocol_config.signer, ClientSelectedSigner::Local) {
                break 'skipped;
            }

            let Some(starknet_config) = config.signer.local.protocols.get("starknet") else {
                eyre::bail!("Starknet signer is set to 'local' but no local Starknet credentials found");
            };

            if starknet_config.signers.is_empty() {
                eyre::bail!("Starknet signer is set to 'local' but no Starknet credentials provided");
            }

            let mut config = starknet::StarknetConfig {
                networks: Default::default(),
            };

            for (network, signer) in &starknet_config.signers {
                let credentials: starknet::Credentials = serde_json::from_value(signer.credentials.clone())
                    .map_err(|e| eyre::eyre!("Failed to parse Starknet credentials: {}", e))?;

                let _ignored = config.networks.insert(
                    network.clone().into(),
                    starknet::NetworkConfig {
                        rpc_url: signer.rpc_url.clone(),
                        account_id: credentials.account_id.clone(),
                        access_key: credentials.secret_key.clone(),
                    },
                );
            }

            starknet_transport = Some(starknet::StarknetTransport::new(&config));
        }

        let mut icp_transport = None;

        'skipped: {
            let Some(protocol_config) = config.params.get("icp") else {
                break 'skipped;
            };

            if !matches!(protocol_config.signer, ClientSelectedSigner::Local) {
                break 'skipped;
            }

            let Some(icp_config) = config.signer.local.protocols.get("icp") else {
                eyre::bail!("ICP signer is set to 'local' but no local ICP credentials found");
            };

            if icp_config.signers.is_empty() {
                eyre::bail!("ICP signer is set to 'local' but no ICP credentials provided");
            }

            let mut config = icp::IcpConfig {
                networks: Default::default(),
            };

            for (network, signer) in &icp_config.signers {
                let credentials: icp::Credentials = serde_json::from_value(signer.credentials.clone())
                    .map_err(|e| eyre::eyre!("Failed to parse ICP credentials: {}", e))?;

                let _ignored = config.networks.insert(
                    network.clone().into(),
                    icp::NetworkConfig {
                        rpc_url: signer.rpc_url.clone(),
                        account_id: credentials.account_id.clone(),
                        secret_key: credentials.secret_key.clone(),
                    },
                );
            }

            icp_transport = Some(icp::IcpTransport::new(&config));
        }

        let mut stellar_transport = None;

        'skipped: {
            let Some(protocol_config) = config.params.get("stellar") else {
                break 'skipped;
            };

            if !matches!(protocol_config.signer, ClientSelectedSigner::Local) {
                break 'skipped;
            }

            let Some(stellar_config) = config.signer.local.protocols.get("stellar") else {
                eyre::bail!("Stellar signer is set to 'local' but no local Stellar credentials found");
            };

            if stellar_config.signers.is_empty() {
                eyre::bail!("Stellar signer is set to 'local' but no Stellar credentials provided");
            }

            let mut config = stellar::StellarConfig {
                networks: Default::default(),
            };

            for (network, signer) in &stellar_config.signers {
                let credentials: stellar::Credentials = serde_json::from_value(signer.credentials.clone())
                    .map_err(|e| eyre::eyre!("Failed to parse Stellar credentials: {}", e))?;

                let _ignored = config.networks.insert(
                    network.clone().into(),
                    stellar::NetworkConfig {
                        network: network.clone().into(),
                        rpc_url: signer.rpc_url.clone(),
                        public_key: credentials.public_key.clone(),
                        secret_key: credentials.secret_key.clone(),
                    },
                );
            }

            stellar_transport = Some(stellar::StellarTransport::new(&config));
        }

        let mut ethereum_transport = None;

        'skipped: {
            let Some(protocol_config) = config.params.get("ethereum") else {
                break 'skipped;
            };

            if !matches!(protocol_config.signer, ClientSelectedSigner::Local) {
                break 'skipped;
            }

            let Some(ethereum_config) = config.signer.local.protocols.get("ethereum") else {
                eyre::bail!("Ethereum signer is set to 'local' but no local Ethereum credentials found");
            };

            if ethereum_config.signers.is_empty() {
                eyre::bail!("Ethereum signer is set to 'local' but no Ethereum credentials provided");
            }

            let mut config = ethereum::EthereumConfig {
                networks: Default::default(),
            };
            for (network, signer) in &ethereum_config.signers {
                let credentials: ethereum::Credentials = serde_json::from_value(signer.credentials.clone())
                    .map_err(|e| eyre::eyre!("Failed to parse Ethereum credentials: {}", e))?;

                let access_key: PrivateKeySigner =
                    PrivateKeySigner::from_str(&credentials.secret_key)
                        .wrap_err("failed to convert secret key to PrivateKeySigner")?;

                let _ignored = config.networks.insert(
                    network.clone().into(),
                    ethereum::NetworkConfig {
                        rpc_url: signer.rpc_url.clone(),
                        account_id: credentials.account_id.clone(),
                        access_key,
                    },
                );
            }

            ethereum_transport = Some(ethereum::EthereumTransport::new(&config));
        }

        let all_transports = transport!(
            near_transport,
            starknet_transport,
            icp_transport,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::config::*;
    use serde_json::json;
    use url::Url;
    use std::collections::BTreeMap;

    #[test]
    fn test_credentials_parsing_with_json_value() {
        // Test that credentials are stored as serde_json::Value for flexible parsing
        let json_data = json!({
            "rpc_url": "https://rpc.example.com",
            "account_id": "test.near",
            "private_key": "ed25519:..."
        });
        
        let signer: ClientLocalSigner = serde_json::from_value(json_data).unwrap();
        assert_eq!(signer.rpc_url.as_str(), "https://rpc.example.com/");
        assert_eq!(signer.credentials["account_id"], "test.near");
        assert_eq!(signer.credentials["private_key"], "ed25519:...");
    }

    #[test]
    fn test_protocol_selection_logic() {
        // Test that protocol selection checks signer type first, then validates credentials only for local signers
        let mut params = BTreeMap::new();
        params.insert("near".to_string(), ClientConfigParams {
            signer: ClientSelectedSigner::Relayer,
            network: "testnet".to_string(),
            contract_id: "app.testnet".to_string(),
        });

        let config = ClientConfig {
            params,
            signer: ClientSigner {
                relayer: ClientRelayerSigner {
                    url: Url::parse("http://localhost:3000").unwrap(),
                },
                local: LocalConfig {
                    protocols: BTreeMap::new(), // No local credentials needed for relayer
                },
            },
        };

        let result = Client::from_config(&config);
        // Should work since from_config creates the client successfully for relayer configs
    }
}
