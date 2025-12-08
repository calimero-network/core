use calimero_blobstore::config::BlobStoreConfig;
use calimero_config::ConfigFile;
use calimero_network_primitives::config::NetworkConfig;
use calimero_node::sync::SyncConfig;
use calimero_node::{start, NodeConfig, NodeMode, SpecializedNodeConfig};
use calimero_server::config::{AuthMode, ServerConfig};
use calimero_store::config::StoreConfig;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use mero_auth::config::StorageConfig as AuthStorageConfig;
use mero_auth::embedded::default_config;
use tracing::info;

use super::auth_mode::AuthModeArg;
use super::node_mode::NodeModeArg;
use crate::cli::RootArgs;

/// Run a node
#[derive(Debug, Parser)]
pub struct RunCommand {
    /// Override the authentication mode configured in config.toml
    #[arg(long, value_enum)]
    pub auth_mode: Option<AuthModeArg>,

    /// Node operation mode (standard or read-only)
    #[arg(long, value_enum, default_value_t = NodeModeArg::Standard)]
    pub mode: NodeModeArg,
}

impl RunCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let mut config = ConfigFile::load(&path).await?;

        if let Some(mode) = self.auth_mode {
            config.network.server.auth_mode = mode.into();
        }

        // Convert CLI mode to NodeMode
        let node_mode: NodeMode = self.mode.into();

        // In read-only mode, disable JSON-RPC to prevent execution requests
        if node_mode == NodeMode::ReadOnly {
            info!("Starting node in read-only mode - JSON-RPC execution disabled");
            config.network.server.jsonrpc = None;
        }

        let network = config.network;
        let mut server_source = network.server;

        // Ensure embedded_auth config exists with resolved paths when embedded mode is active
        if matches!(server_source.auth_mode, AuthMode::Embedded) {
            let mut auth_config = server_source
                .embedded_auth
                .take()
                .unwrap_or_else(default_config);

            // Resolve relative RocksDB paths against the node's home directory
            if let AuthStorageConfig::RocksDB { path: storage_path } = &mut auth_config.storage {
                if storage_path.is_relative() {
                    let joined = path.as_std_path().join(&*storage_path);
                    *storage_path = joined.try_into().expect("Invalid UTF-8 path");
                }
            }

            server_source.embedded_auth = Some(auth_config);
        } else if let Some(cfg) = server_source.embedded_auth.as_mut() {
            // Also resolve paths for proxy mode if config exists
            if let AuthStorageConfig::RocksDB { path: storage_path } = &mut cfg.storage {
                if storage_path.is_relative() {
                    let joined = path.as_std_path().join(&*storage_path);
                    *storage_path = joined.try_into().expect("Invalid UTF-8 path");
                }
            }
        }
        let server_config = ServerConfig::with_auth(
            server_source.listen,
            config.identity.clone(),
            server_source.admin,
            server_source.jsonrpc,
            server_source.websocket,
            server_source.sse,
            server_source.auth_mode,
            server_source.embedded_auth,
        );

        start(NodeConfig {
            home: path.clone(),
            identity: config.identity.clone(),
            network: NetworkConfig::new(
                config.identity.clone(),
                network.swarm,
                network.bootstrap,
                network.discovery,
            ),
            sync: SyncConfig {
                timeout: config.sync.timeout,
                interval: config.sync.interval,
                frequency: config.sync.frequency,
                ..Default::default() // Use defaults for new fields
            },
            datastore: StoreConfig::new(path.join(config.datastore.path)),
            blobstore: BlobStoreConfig::new(path.join(config.blobstore.path)),
            context: config.context,
            server: server_config,
            gc_interval_secs: None, // Use default (12 hours)
            mode: node_mode,
            specialized_node: SpecializedNodeConfig {
                invite_topic: network.specialized_node.invite_topic,
                accept_mock_tee: network.specialized_node.accept_mock_tee,
            },
        })
        .await
    }
}
