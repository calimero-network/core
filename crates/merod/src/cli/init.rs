use core::net::IpAddr;
use core::time::Duration;
use std::collections::BTreeMap;
use std::fs::{create_dir, create_dir_all};

use calimero_config::{
    BlobStoreConfig, ConfigFile, DataStoreConfig as StoreConfigFile, NetworkConfig, ServerConfig,
    SyncConfig,
};
use calimero_context::config::ContextConfig;
use calimero_context_config::client::config::{
    ClientConfig, ClientConfigParams, ClientLocalConfig, ClientLocalSigner, ClientRelayerSigner,
    ClientSelectedSigner, ClientSigner, Credentials, LocalConfig,
};
use calimero_context_config::client::protocol::{
    icp as icp_protocol, near as near_protocol, starknet as starknet_protocol,
    stellar as stellar_protocol,
};
use calimero_network::config::{
    BootstrapConfig, BootstrapNodes, DiscoveryConfig, RelayConfig, RendezvousConfig, SwarmConfig,
};
use calimero_server::admin::service::AdminConfig;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::ws::WsConfig;
use calimero_store::config::StoreConfig;
use calimero_store::db::RocksDB;
use calimero_store::Store;
use clap::{Parser, ValueEnum};
use ed25519_consensus::SigningKey as IcpSigningKey;
use eyre::{bail, Result as EyreResult, WrapErr};
use hex::encode;
use ic_agent::export::Principal;
use ic_agent::identity::{BasicIdentity, Identity};
use libp2p::identity::Keypair;
use multiaddr::{Multiaddr, Protocol};
use near_crypto::{KeyType, SecretKey};
use rand::rngs::OsRng;
use soroban_client::keypair::{Keypair as StellarKeypair, KeypairBehavior};
use starknet::signers::SigningKey;
use tracing::{info, warn};
use url::Url;

use crate::{cli, defaults};

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ConfigProtocol {
    Near,
    Starknet,
    Icp,
    Stellar,
}

/// Initialize node configuration
#[derive(Debug, Parser)]
pub struct InitCommand {
    /// List of bootstrap nodes
    #[clap(long, value_name = "ADDR")]
    pub boot_nodes: Vec<Multiaddr>,

    /// Use nodes from a known network
    #[clap(long, value_name = "NETWORK", default_value = "calimero-dev")]
    pub boot_network: Option<BootstrapNetwork>,

    /// Host to listen on
    #[clap(long, value_name = "HOST")]
    #[clap(default_value = "0.0.0.0,::")]
    #[clap(use_value_delimiter = true)]
    pub swarm_host: Vec<IpAddr>,

    /// Port to listen on
    #[clap(long, value_name = "PORT")]
    #[clap(default_value_t = calimero_network::config::DEFAULT_PORT)]
    pub swarm_port: u16,

    /// Host to listen on for RPC
    #[clap(long, value_name = "HOST")]
    #[clap(default_value = "127.0.0.1,::1")]
    #[clap(use_value_delimiter = true)]
    pub server_host: Vec<IpAddr>,

    /// Port to listen on for RPC
    #[clap(long, value_name = "PORT")]
    #[clap(default_value_t = calimero_server::config::DEFAULT_PORT)]
    pub server_port: u16,

    /// URL of the relayer for submitting NEAR transactions
    #[clap(long, value_name = "URL")]
    pub relayer_url: Option<Url>,

    /// Name of protocol
    #[clap(long, value_name = "PROTOCOL", default_value = "near")]
    #[clap(value_enum)]
    pub protocol: ConfigProtocol,

    /// Enable mDNS discovery
    #[clap(long, default_value_t = true)]
    #[clap(overrides_with("no_mdns"))]
    pub mdns: bool,

    #[clap(long, hide = true)]
    #[clap(overrides_with("mdns"))]
    pub no_mdns: bool,

    #[clap(long, default_value = "3")]
    pub rendezvous_registrations_limit: usize,

    #[clap(long, default_value = "3")]
    pub relay_registrations_limit: usize,

    /// Force initialization even if the directory already exists
    #[clap(long)]
    pub force: bool,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum BootstrapNetwork {
    CalimeroDev,
    Ipfs,
}

impl InitCommand {
    // TODO: Consider splitting this function up to reduce complexity.
    #[expect(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "TODO: Will be refactored"
    )]
    pub fn run(self, root_args: cli::RootArgs) -> EyreResult<()> {
        let mdns = self.mdns && !self.no_mdns;

        let path = root_args.home.join(root_args.node_name);

        if !path.exists() {
            if root_args.home == defaults::default_node_dir() {
                create_dir_all(&path)
            } else {
                create_dir(&path)
            }
            .wrap_err_with(|| format!("failed to create directory {path:?}"))?;
        }

        if ConfigFile::exists(&path) {
            if let Err(err) = ConfigFile::load(&path) {
                if self.force {
                    warn!(
                        "Failed to load existing configuration, overwriting: {}",
                        err
                    );
                } else {
                    bail!("Failed to load existing configuration: {}", err);
                }
            }
            if !self.force {
                warn!("Node is already initialized in {:?}", path);
                return Ok(());
            }
        }

        let identity = Keypair::generate_ed25519();
        info!("Generated identity: {:?}", identity.public().to_peer_id());

        let mut listen: Vec<Multiaddr> = vec![];

        for host in self.swarm_host {
            let host = format!(
                "/{}/{}",
                match host {
                    IpAddr::V4(_) => "ip4",
                    IpAddr::V6(_) => "ip6",
                },
                host,
            );
            listen.push(format!("{}/tcp/{}", host, self.swarm_port).parse()?);
            listen.push(format!("{}/udp/{}/quic-v1", host, self.swarm_port).parse()?);
        }

        let mut boot_nodes = self.boot_nodes;
        if let Some(network) = self.boot_network {
            match network {
                BootstrapNetwork::CalimeroDev => {
                    boot_nodes.extend(BootstrapNodes::calimero_dev().list);
                }
                BootstrapNetwork::Ipfs => boot_nodes.extend(BootstrapNodes::ipfs().list),
            }
        }

        let mut local_signers = LocalConfig {
            protocols: BTreeMap::default(),
        };

        let mut client_params = BTreeMap::default();

        {
            let _ignored = client_params.insert(
                "near".to_owned(),
                ClientConfigParams {
                    network: "testnet".into(),
                    protocol: "near".into(),
                    contract_id: "calimero-context-config.testnet".parse()?,
                    signer: ClientSelectedSigner::Relayer,
                },
            );

            let mut local_config = ClientLocalConfig {
                signers: Default::default(),
            };

            let _ignored = local_config.signers.insert(
                "mainnet".to_owned(),
                generate_local_signer(
                    "https://rpc.mainnet.near.org".parse()?,
                    ConfigProtocol::Near,
                )?,
            );

            let _ignored = local_config.signers.insert(
                "testnet".to_owned(),
                generate_local_signer(
                    "https://rpc.testnet.near.org".parse()?,
                    ConfigProtocol::Near,
                )?,
            );

            let _ignored = local_signers
                .protocols
                .insert("near".to_owned(), local_config);
        }

        {
            let _ignored = client_params.insert(
                "starknet".to_owned(),
                ClientConfigParams {
                    network: "sepolia".into(),
                    protocol: "starknet".into(),
                    contract_id:
                        "0x1b991ee006e2d1e372ab96d0a957401fa200358f317b681df2948f30e17c29c"
                            .parse()?,
                    signer: ClientSelectedSigner::Relayer,
                },
            );

            let mut local_config = ClientLocalConfig {
                signers: Default::default(),
            };

            let _ignored = local_config.signers.insert(
                "mainnet".to_owned(),
                generate_local_signer(
                    "https://cloud.argent-api.com/v1/starknet/mainnet/rpc/v0.7".parse()?,
                    ConfigProtocol::Starknet,
                )?,
            );

            let _ignored = local_config.signers.insert(
                "sepolia".to_owned(),
                generate_local_signer(
                    "https://free-rpc.nethermind.io/sepolia-juno/".parse()?,
                    ConfigProtocol::Starknet,
                )?,
            );

            let _ignored = local_signers
                .protocols
                .insert("starknet".to_owned(), local_config);
        }

        {
            let _ignored = client_params.insert(
                "icp".to_owned(),
                ClientConfigParams {
                    network: "local".into(),
                    protocol: "icp".into(),
                    contract_id: "bkyz2-fmaaa-aaaaa-qaaaq-cai".parse()?,
                    signer: ClientSelectedSigner::Local,
                },
            );

            let mut local_config = ClientLocalConfig {
                signers: Default::default(),
            };

            let _ignored = local_config.signers.insert(
                "ic".to_owned(),
                generate_local_signer("https://ic0.app".parse()?, ConfigProtocol::Icp)?,
            );

            let _ignored = local_config.signers.insert(
                "local".to_owned(),
                generate_local_signer("http://127.0.0.1:4943".parse()?, ConfigProtocol::Icp)?,
            );

            let _ignored = local_signers
                .protocols
                .insert("icp".to_owned(), local_config);
        }

        {
            let _ignored = client_params.insert(
                "stellar".to_owned(),
                ClientConfigParams {
                    network: "testnet".into(),
                    protocol: "stellar".into(),
                    contract_id: "CDZ25SJ65YRXTCWMJNLTNZXPFPBGHOOB7BUBYQE7W3PU7I357BTX6QZY"
                        .parse()?,
                    signer: ClientSelectedSigner::Relayer,
                },
            );

            let mut local_config = ClientLocalConfig {
                signers: Default::default(),
            };

            let _ignored = local_config.signers.insert(
                "mainnet".to_owned(),
                generate_local_signer(
                    "https://soroban.stellar.org".parse()?,
                    ConfigProtocol::Stellar,
                )?,
            );

            let _ignored = local_config.signers.insert(
                "testnet".to_owned(),
                generate_local_signer(
                    "https://soroban-testnet.stellar.org".parse()?,
                    ConfigProtocol::Stellar,
                )?,
            );

            let _ignored = local_signers
                .protocols
                .insert("stellar".to_owned(), local_config);
        }

        let relayer = self
            .relayer_url
            .unwrap_or_else(defaults::default_relayer_url);

        let client_config = ClientConfig {
            signer: ClientSigner {
                relayer: ClientRelayerSigner { url: relayer },
                local: local_signers,
            },
            params: client_params,
        };

        let config = ConfigFile::new(
            identity,
            NetworkConfig::new(
                SwarmConfig::new(listen),
                BootstrapConfig::new(BootstrapNodes::new(boot_nodes)),
                DiscoveryConfig::new(
                    mdns,
                    RendezvousConfig::new(self.rendezvous_registrations_limit),
                    RelayConfig::new(self.relay_registrations_limit),
                ),
                ServerConfig::new(
                    self.server_host
                        .into_iter()
                        .map(|host| Multiaddr::from(host).with(Protocol::Tcp(self.server_port)))
                        .collect(),
                    Some(AdminConfig::new(true)),
                    Some(JsonRpcConfig::new(true)),
                    Some(WsConfig::new(true)),
                ),
            ),
            SyncConfig {
                timeout: Duration::from_secs(30),
                interval: Duration::from_secs(30),
            },
            StoreConfigFile::new("data".into()),
            BlobStoreConfig::new("blobs".into()),
            ContextConfig {
                client: client_config,
            },
        );

        config.save(&path)?;

        drop(Store::open::<RocksDB>(&StoreConfig::new(
            path.join(config.datastore.path),
        ))?);

        info!("Initialized a node in {:?}", path);

        Ok(())
    }
}

fn generate_local_signer(
    rpc_url: Url,
    config_protocol: ConfigProtocol,
) -> EyreResult<ClientLocalSigner> {
    match config_protocol {
        ConfigProtocol::Near => {
            let secret_key = SecretKey::from_random(KeyType::ED25519);
            let public_key = secret_key.public_key();
            let account_id = public_key.unwrap_as_ed25519().0;

            Ok(ClientLocalSigner {
                rpc_url,
                credentials: Credentials::Near(near_protocol::Credentials {
                    account_id: encode(account_id).parse()?,
                    public_key,
                    secret_key,
                }),
            })
        }
        ConfigProtocol::Starknet => {
            let keypair = SigningKey::from_random();
            let secret_key = SigningKey::secret_scalar(&keypair);
            let public_key = keypair.verifying_key().scalar();

            Ok(ClientLocalSigner {
                rpc_url,
                credentials: Credentials::Starknet(starknet_protocol::Credentials {
                    account_id: public_key,
                    public_key,
                    secret_key,
                }),
            })
        }
        ConfigProtocol::Icp => {
            let mut rng = OsRng;

            let signing_key = IcpSigningKey::new(&mut rng);
            let identity = BasicIdentity::from_signing_key(signing_key.clone());

            let public_key = identity.public_key().unwrap();
            let account_id = Principal::self_authenticating(&public_key);

            Ok(ClientLocalSigner {
                rpc_url,
                credentials: Credentials::Icp(icp_protocol::Credentials {
                    account_id,
                    public_key: encode(&account_id),
                    secret_key: encode(&signing_key),
                }),
            })
        }

        ConfigProtocol::Stellar => {
            let keypair = StellarKeypair::random().unwrap();
            let public_key = keypair.public_key();
            let secret_key = keypair.secret_key().unwrap();

            Ok(ClientLocalSigner {
                rpc_url,
                credentials: Credentials::Stellar(stellar_protocol::Credentials {
                    public_key,
                    secret_key,
                }),
            })
        }
    }
}
