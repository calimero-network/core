//! Standalone relayer configuration

use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::constants::{protocols, DEFAULT_ADDR};
use crate::credentials::{CredentialBuilder, RelayerCredentials};

/// Standalone relayer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayerConfig {
    /// Address to listen on for incoming HTTP requests
    pub listen: SocketAddr,
    /// Blockchain protocol configurations
    pub protocols: BTreeMap<String, ProtocolConfig>,
}

/// Configuration for a specific blockchain protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolConfig {
    /// Whether this protocol is enabled
    pub enabled: bool,
    /// Network name (e.g., "testnet", "mainnet", "sepolia")
    pub network: String,
    /// RPC endpoint URL
    pub rpc_url: Url,
    /// Contract address for this protocol
    pub contract_id: String,
    /// Optional signing credentials for local signing
    pub credentials: Option<ProtocolCredentials>,
}

/// Protocol-specific signing credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProtocolCredentials {
    Near {
        account_id: String,
        public_key: String,
        secret_key: String,
    },
    Starknet {
        account_id: String,
        public_key: String,
        secret_key: String,
    },
    Icp {
        account_id: String,
        public_key: String,
        secret_key: String,
    },
    Ethereum {
        account_id: String,
        secret_key: String,
    },
}

impl Default for RelayerConfig {
    fn default() -> Self {
        let mut protocols = BTreeMap::new();

        // Default NEAR configuration - credentials must come from environment
        drop(protocols.insert(
            protocols::near::NAME.to_owned(),
            ProtocolConfig {
                enabled: true,
                network: protocols::near::DEFAULT_NETWORK.to_owned(),
                rpc_url: protocols::near::DEFAULT_RPC_URL.parse().unwrap(),
                contract_id: protocols::near::DEFAULT_CONTRACT_ID.to_owned(),
                credentials: RelayerCredentials::default_credentials(protocols::near::NAME),
            },
        ));

        // Default Starknet configuration (disabled by default)
        drop(protocols.insert(
            protocols::starknet::NAME.to_owned(),
            ProtocolConfig {
                enabled: false,
                network: protocols::starknet::DEFAULT_NETWORK.to_owned(),
                rpc_url: protocols::starknet::DEFAULT_RPC_URL.parse().unwrap(),
                contract_id: protocols::starknet::DEFAULT_CONTRACT_ID.to_owned(),
                credentials: RelayerCredentials::default_credentials(protocols::starknet::NAME),
            },
        ));

        // Default ICP configuration (disabled by default)
        drop(protocols.insert(
            protocols::icp::NAME.to_owned(),
            ProtocolConfig {
                enabled: false,
                network: protocols::icp::DEFAULT_NETWORK.to_owned(),
                rpc_url: protocols::icp::DEFAULT_RPC_URL.parse().unwrap(),
                contract_id: protocols::icp::DEFAULT_CONTRACT_ID.to_owned(),
                credentials: RelayerCredentials::default_credentials(protocols::icp::NAME),
            },
        ));

        // Default Ethereum configuration (disabled by default)
        drop(protocols.insert(
            protocols::ethereum::NAME.to_owned(),
            ProtocolConfig {
                enabled: false,
                network: protocols::ethereum::DEFAULT_NETWORK.to_owned(),
                rpc_url: protocols::ethereum::DEFAULT_RPC_URL.parse().unwrap(),
                contract_id: protocols::ethereum::DEFAULT_CONTRACT_ID.to_owned(),
                credentials: RelayerCredentials::default_credentials(protocols::ethereum::NAME),
            },
        ));

        Self {
            listen: DEFAULT_ADDR,
            protocols,
        }
    }
}

impl RelayerConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // Override listen address from environment
        if let Ok(listen) = std::env::var("RELAYER_LISTEN") {
            if let Ok(addr) = listen.parse() {
                config.listen = addr;
            }
        }

        // Configure protocols from environment variables
        for (protocol_name, protocol_config) in &mut config.protocols {
            let prefix = protocol_name.to_uppercase();

            // Check if protocol is enabled
            if let Ok(enabled) = std::env::var(format!("ENABLE_{}", prefix)) {
                protocol_config.enabled = enabled.to_lowercase() == "true";
            }

            // Override network
            if let Ok(network) = std::env::var(format!("{}_NETWORK", prefix)) {
                protocol_config.network = network;
            }

            // Override RPC URL
            if let Ok(rpc_url) = std::env::var(format!("{}_RPC_URL", prefix)) {
                if let Ok(url) = rpc_url.parse() {
                    protocol_config.rpc_url = url;
                }
            }

            // Override contract ID
            if let Ok(contract_id) = std::env::var(format!("{}_CONTRACT_ID", prefix)) {
                protocol_config.contract_id = contract_id;
            }

            // Override credentials from environment variables if available
            if let Some(env_credentials) = RelayerCredentials::from_env(protocol_name) {
                protocol_config.credentials = Some(env_credentials);
            }
        }

        config
    }

    /// Get enabled protocols
    pub fn enabled_protocols(&self) -> impl Iterator<Item = (&String, &ProtocolConfig)> {
        self.protocols.iter().filter(|(_, config)| config.enabled)
    }
}
