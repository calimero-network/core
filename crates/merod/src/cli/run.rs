use calimero_blobstore::config::BlobStoreConfig;
use calimero_config::ConfigFile;
use calimero_network_primitives::config::NetworkConfig;
use calimero_node::sync::SyncConfig;
use calimero_node::{start, NodeConfig, NodeMode, SpecializedNodeConfig};
use calimero_server::config::{AuthMode, ServerConfig};
use calimero_store::config::StoreConfig;
use calimero_store::Store;
use calimero_store_encryption::EncryptedDatabase;
use calimero_store_rocksdb::RocksDB;
use clap::Parser;
use eyre::{bail, Result as EyreResult, WrapErr};
use mero_auth::config::StorageConfig as AuthStorageConfig;
use mero_auth::embedded::default_config;
use tracing::info;

use super::auth_mode::AuthModeArg;
use super::validation::validate_config;
use crate::cli::RootArgs;
use crate::kms;
use crate::node_identity;

/// Run a node
#[derive(Debug, Parser)]
pub struct RunCommand {
    /// Override the authentication mode configured in config.toml
    #[arg(long, value_enum)]
    pub auth_mode: Option<AuthModeArg>,
}

impl RunCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let mut config = ConfigFile::load(&path).await?;

        // Apply CLI auth_mode override before validation
        if let Some(mode) = self.auth_mode {
            config.network.server.auth_mode = mode.into();
        }

        // Validate configuration at startup (after CLI overrides are applied)
        validate_config(&config, &path).wrap_err(
            "Configuration validation failed - please fix the configuration and try again",
        )?;

        let peer_id = config.identity.peer_id.clone();

        // Fetch storage encryption key from KMS if configured
        let encryption_key = if let Some(ref tee_config) = config.tee {
            info!("TEE configured, fetching storage key for peer {}", peer_id);

            let key = kms::fetch_storage_key(&tee_config.kms, &peer_id)
                .await
                .wrap_err(
                    "TEE storage encryption is configured but failed to fetch key from KMS. \
                     The node cannot start without the encryption key to prevent unencrypted data storage.",
                )?;

            info!(
                "Storage encryption key fetched successfully (key_len={})",
                key.len()
            );
            Some(key)
        } else {
            None
        };

        // Load identity from datastore (or migrate from config)
        let datastore_path = path.join(config.datastore.path);
        let identity = if let Some(ref key) = encryption_key {
            let store_config = StoreConfig::with_encryption(datastore_path.clone(), key.clone());
            let inner_db = RocksDB::open(&store_config)?;
            let encrypted_db = EncryptedDatabase::wrap(inner_db, key.clone())?;
            let mut store = Store::new(std::sync::Arc::new(encrypted_db));

            match node_identity::load_from_store(&store)? {
                Some(kp) => {
                    info!("Loaded node identity from datastore");
                    kp
                }
                None => {
                    let kp = config.identity.to_keypair().map_err(|e| {
                        eyre::eyre!(
                            "No identity in datastore and config has no keypair: {e}. \
                             Run merod init first, or restore from backup."
                        )
                    })?;
                    node_identity::save_to_store(&mut store, &kp)?;
                    info!("Migrated node identity from config to datastore");

                    config.identity = calimero_config::IdentityConfig::peer_id_only(peer_id.clone());
                    config.save(&path).await?;

                    kp
                }
            }
        } else {
            let store_config = StoreConfig::new(datastore_path.clone());
            let mut store = Store::open::<RocksDB>(&store_config)?;

            match node_identity::load_from_store(&store)? {
                Some(kp) => {
                    info!("Loaded node identity from datastore");
                    kp
                }
                None => {
                    let kp = config.identity.to_keypair().map_err(|e| {
                        eyre::eyre!(
                            "No identity in datastore and config has no keypair: {e}. \
                             Run merod init first, or restore from backup."
                        )
                    })?;
                    node_identity::save_to_store(&mut store, &kp)?;
                    info!("Migrated node identity from config to datastore");

                    config.identity = calimero_config::IdentityConfig::peer_id_only(peer_id.clone());
                    config.save(&path).await?;

                    kp
                }
            }
        };

        // Read node mode from config
        let node_mode = config.mode;

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
            identity.clone(),
            server_source.admin,
            server_source.jsonrpc,
            server_source.websocket,
            server_source.sse,
            server_source.auth_mode,
            server_source.embedded_auth,
        );

        // Create store config with optional encryption
        let datastore_path = path.join(config.datastore.path);
        let datastore_config = match encryption_key {
            Some(key) => {
                info!("Storage encryption enabled");
                StoreConfig::with_encryption(datastore_path, key)
            }
            None => StoreConfig::new(datastore_path),
        };

        start(NodeConfig {
            home: path.clone(),
            identity: identity.clone(),
            network: NetworkConfig::new(
                identity.clone(),
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
            datastore: datastore_config,
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
