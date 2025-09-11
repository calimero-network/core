//! Standalone relayer configuration

use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use url::Url;

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

        // Default NEAR configuration with testnet credentials
        drop(protocols.insert("near".to_string(), ProtocolConfig {
            enabled: true,
            network: "testnet".to_string(),
            rpc_url: "https://rpc.testnet.near.org".parse().unwrap(),
            contract_id: "calimero-context-config.testnet".to_string(),
            credentials: Some(ProtocolCredentials::Near {
                account_id: "dev-1642425627065-33437663923179".to_string(),
                public_key: "ed25519:98GtfF5gBPUvBWNgz8N8WNEjXRgBhFLuSQ5MnFDEjJ8x".to_string(),
                secret_key: "ed25519:4YdVWc7hgBUWwE9kXd4SPKmCztbGkMdHfZL2fDWw8L7gCJmrYcWAjcvK5Wek94aKSGBdLKHb7DaKoXudp6BnTqCb".to_string(),
            }),
        }));

        // Default Starknet configuration (disabled by default, but with working credentials when enabled)
        drop(
            protocols.insert(
                "starknet".to_string(),
                ProtocolConfig {
                    enabled: false,
                    network: "sepolia".to_string(),
                    rpc_url: "https://free-rpc.nethermind.io/sepolia-juno/"
                        .parse()
                        .unwrap(),
                    contract_id:
                        "0x1b991ee006e2d1e372ab96d0a957401fa200358f317b681df2948f30e17c29c"
                            .to_string(),
                    credentials: Some(ProtocolCredentials::Starknet {
                        account_id:
                            "0x01cf4d57ba01109f018dec3ea079a38fc08b0f8a78eed0d4c5e5fb22928dbc8c"
                                .to_string(),
                        public_key:
                            "0x02c5dbad71c92a45cc4b40573ae661f8147869a91d57b8d9b8f48c8af7f83159"
                                .to_string(),
                        secret_key:
                            "0x0178eb2a625c0a8d85b0a5fd69fc879f9884f5205ad9d1ba41db0d7d1a77950a"
                                .to_string(),
                    }),
                },
            ),
        );

        // Default ICP configuration (disabled by default, but with working credentials when enabled)
        drop(
            protocols.insert(
                "icp".to_string(),
                ProtocolConfig {
                    enabled: false,
                    network: "local".to_string(),
                    rpc_url: "http://127.0.0.1:4943".parse().unwrap(),
                    contract_id: "bkyz2-fmaaa-aaaaa-qaaaq-cai".to_string(),
                    credentials: Some(ProtocolCredentials::Icp {
                        account_id: "rdmx6-jaaaa-aaaaa-aaadq-cai".to_string(),
                        public_key: "MCowBQYDK2VwAyEAL8XDEY1gGOWvv/0h01tW/ZV14qYY7GrHJF3pZoNxmHE="
                            .to_string(),
                        secret_key:
                            "MFECAQEwBQYDK2VwBCIEIJKDIfd1Ybt7xliQlRmXZGRWG8dJ1Dl9qKGT0pOhMwPjaE30"
                                .to_string(),
                    }),
                },
            ),
        );

        // Default Ethereum configuration (disabled by default, but with working credentials when enabled)
        drop(
            protocols.insert(
                "ethereum".to_string(),
                ProtocolConfig {
                    enabled: false,
                    network: "sepolia".to_string(),
                    rpc_url: "https://sepolia.drpc.org".parse().unwrap(),
                    contract_id: "0x83365DE41E1247511F4C5D10Fb1AFe59b96aD4dB".to_string(),
                    credentials: Some(ProtocolCredentials::Ethereum {
                        account_id: "0x8ba1f109551bD432803012645Hac136c22C177ec".to_string(),
                        secret_key:
                            "0ac1e735c1ca39db4a9c54d4edf2c6a50a75a3b3dce1cd2a64e8f5a44d1e2d2c"
                                .to_string(),
                    }),
                },
            ),
        );

        Self {
            listen: "0.0.0.0:63529".parse().unwrap(),
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
        }

        config
    }

    /// Get enabled protocols
    pub fn enabled_protocols(&self) -> impl Iterator<Item = (&String, &ProtocolConfig)> {
        self.protocols.iter().filter(|(_, config)| config.enabled)
    }
}
