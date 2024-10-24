use core::net::IpAddr;
use core::time::Duration;
use std::fs::{create_dir, create_dir_all};

use calimero_config::{
    BlobStoreConfig, ConfigFile, DataStoreConfig as StoreConfigFile, NetworkConfig, ServerConfig,
};
use calimero_context::config::ContextConfig;
use calimero_context_config::client::config::{
    ContextConfigClientConfig, ContextConfigClientLocalSigner, ContextConfigClientNew,
    ContextConfigClientRelayerSigner, ContextConfigClientSelectedSigner, ContextConfigClientSigner,
    Credentials,
};
use calimero_network::config::{
    BootstrapConfig, BootstrapNodes, CatchupConfig, DiscoveryConfig, RelayConfig, RendezvousConfig,
    SwarmConfig,
};
use calimero_server::admin::service::AdminConfig;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::ws::WsConfig;
use calimero_store::config::StoreConfig;
use calimero_store::db::RocksDB;
use calimero_store::Store;
use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult, WrapErr};
use libp2p::identity::Keypair;
use multiaddr::{Multiaddr, Protocol};
use near_crypto::{KeyType, SecretKey};
use rand::{thread_rng, Rng};
use tracing::{info, warn};
use url::Url;

use crate::{cli, defaults};

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
                bail!("Node is already initialized in {:?}", path);
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

        let relayer = self
            .relayer_url
            .unwrap_or_else(defaults::default_relayer_url);

        let config = ConfigFile::new(
            identity,
            StoreConfigFile::new("data".into()),
            BlobStoreConfig::new("blobs".into()),
            ContextConfig {
                client: ContextConfigClientConfig {
                    signer: ContextConfigClientSigner {
                        selected: ContextConfigClientSelectedSigner::Relayer,
                        relayer: ContextConfigClientRelayerSigner { url: relayer },
                        local: [
                            (
                                "mainnet".to_owned(),
                                generate_local_signer("https://rpc.mainnet.near.org".parse()?)?,
                            ),
                            (
                                "testnet".to_owned(),
                                generate_local_signer("https://rpc.testnet.near.org".parse()?)?,
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    },
                    new: ContextConfigClientNew {
                        network: "testnet".into(),
                        contract_id: "calimero-context-config.testnet".parse()?,
                    },
                },
            },
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
                CatchupConfig::new(
                    50,
                    Duration::from_secs(2),
                    Duration::from_secs(2),
                    Duration::from_millis(thread_rng().gen_range(0..1001)),
                ),
            ),
        );

        config.save(&path)?;

        drop(Store::open::<RocksDB>(&StoreConfig::new(
            path.join(config.datastore.path),
        ))?);

        info!("Initialized a node in {:?}", path);

        Ok(())
    }
}

fn generate_local_signer(rpc_url: Url) -> EyreResult<ContextConfigClientLocalSigner> {
    let secret_key = SecretKey::from_random(KeyType::ED25519);

    let public_key = secret_key.public_key();

    let account_id = public_key.unwrap_as_ed25519().0;

    Ok(ContextConfigClientLocalSigner {
        rpc_url,
        credentials: Credentials {
            account_id: hex::encode(account_id).parse()?,
            public_key,
            secret_key,
        },
    })
}
