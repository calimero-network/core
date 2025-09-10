use std::collections::BTreeMap;

use calimero_config::ConfigFile;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSchema {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<IdentitySchema>,
    pub network: NetworkSchema,
    pub sync: SyncSchema,
    pub datastore: DataStoreSchema,
    pub blobstore: BlobStoreSchema,
    pub context: ContextSchema,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocols: Option<ProtocolsSchema>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolsSchema {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ethereum: Option<EthereumProtocolSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icp: Option<IcpProtocolSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near: Option<NearProtocolSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stellar: Option<StellarProtocolSchema>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct EthereumProtocolSchema {
    #[schemars(description = "Ethereum network name (e.g. sepolia, mainnet)")]
    pub network: String,
    #[schemars(description = "Ethereum contract address")]
    pub contract_id: String,
    #[schemars(description = "Signer type (relayer or local)")]
    pub signer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional RPC URL for direct connection")]
    pub rpc_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional account ID")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional secret key")]
    pub secret_key: Option<String>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct IcpProtocolSchema {
    #[schemars(description = "Icp network name (e.g. sepolia, mainnet)")]
    pub network: String,
    #[schemars(description = "Icp contract address")]
    pub contract_id: String,
    #[schemars(description = "Signer type (relayer or local)")]
    pub signer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional RPC URL for direct connection")]
    pub rpc_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional account ID")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional public key")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional secret key")]
    pub secret_key: Option<String>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct NearProtocolSchema {
    #[schemars(description = "Near network name (e.g. sepolia, mainnet)")]
    pub network: String,
    #[schemars(description = "Near contract address")]
    pub contract_id: String,
    #[schemars(description = "Signer type (relayer or local)")]
    pub signer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional RPC URL for direct connection")]
    pub rpc_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional account ID")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional public key")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional secret key")]
    pub secret_key: Option<String>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct StellarProtocolSchema {
    #[schemars(description = "Stellar network name (e.g. sepolia, mainnet)")]
    pub network: String,
    #[schemars(description = "Stellar contract address")]
    pub contract_id: String,
    #[schemars(description = "Signer type (relayer or local)")]
    pub signer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional RPC URL for direct connection")]
    pub rpc_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional account ID")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional public key")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional secret key")]
    pub secret_key: Option<String>,
}

impl From<schemars::Schema> for ConfigSchema {
    fn from(schema: schemars::Schema) -> Self {
        let schema = schema_for!(ConfigFile);
        let mut config_schema: ConfigSchema =
            serde_json::from_value(serde_json::to_value(schema).unwrap())
                .expect("valid config schema");

        if config_schema.protocols.is_none() {
            config_schema.protocols = Some(ProtocolsSchema {
                ethereum: Some(EthereumProtocolSchema {
                    network: "sepolia".to_owned(),
                    contract_id: "".to_owned(),
                    signer: "".to_owned(),
                    rpc_url: None,
                    account_id: None,
                    secret_key: None,
                }),
                icp: Some(IcpProtocolSchema {
                    network: "local".to_owned(),
                    contract_id: "".to_owned(),
                    signer: "".to_owned(),
                    rpc_url: None,
                    account_id: None,
                    public_key: None,
                    secret_key: None,
                }),
                near: Some(NearProtocolSchema {
                    network: "testnet".to_owned(),
                    contract_id: "".to_owned(),
                    signer: "".to_owned(),
                    rpc_url: None,
                    account_id: None,
                    public_key: None,
                    secret_key: None,
                }),
                stellar: Some(StellarProtocolSchema {
                    network: "testnet".to_owned(),
                    contract_id: "".to_owned(),
                    signer: "".to_owned(),
                    rpc_url: None,
                    account_id: None,
                    public_key: None,
                    secret_key: None,
                }),
            });
        }

        config_schema
    }
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct IdentitySchema {
    #[schemars(description = "Peer ID in base58 format")]
    pub peer_id: String,
    #[schemars(description = "Keypair in protobuf format encoded as base58")]
    pub keypair: String,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct NetworkSchema {
    #[schemars(description = "Swarm configuration")]
    pub swarm: SwarmSchema,
    #[schemars(description = "Server configuration")]
    pub server: ServerSchema,
    #[schemars(description = "Bootstrap configuration")]
    pub bootstrap: BootstrapSchema,
    #[schemars(description = "Discovery configuration")]
    pub discovery: DiscoverySchema,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct SwarmSchema {
    #[schemars(description = "List of multiaddresses to listen on")]
    pub listen: Vec<String>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct ServerSchema {
    #[schemars(description = "List of multiaddresses for RPC server to listen on")]
    pub listen: Vec<String>,
    #[schemars(description = "Admin API configuration")]
    pub admin: Option<AdminSchema>,
    #[schemars(description = "JSON-RPC configuration")]
    pub jsonrpc: Option<JsonRpcSchema>,
    #[schemars(description = "WebSocket configuration")]
    pub websocket: Option<WsSchema>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct AdminSchema {
    #[schemars(description = "Whether admin API is enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct JsonRpcSchema {
    #[schemars(description = "Whether JSON-RPC is enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct WsSchema {
    #[schemars(description = "Whether WebSocket is enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct BootstrapSchema {
    #[schemars(description = "Bootstrap nodes configuration")]
    pub nodes: BootstrapNodesSchema,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct BootstrapNodesSchema {
    #[schemars(description = "List of bootstrap node multiaddresses")]
    pub list: Vec<String>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct DiscoverySchema {
    #[schemars(description = "Whether mDNS discovery is enabled")]
    pub mdns: bool,
    #[schemars(description = "Whether to advertise observed address")]
    pub advertise_address: bool,
    #[schemars(description = "Rendezvous configuration")]
    pub rendezvous: RendezvousSchema,
    #[schemars(description = "Relay configuration")]
    pub relay: RelaySchema,
    #[schemars(description = "Autonat configuration")]
    pub autonat: AutonatSchema,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct RendezvousSchema {
    #[schemars(description = "Rendezvous namespace")]
    pub namespace: String,
    #[schemars(description = "Discovery requests per minute")]
    pub discovery_rpm: f32,
    #[schemars(description = "Discovery interval in seconds")]
    pub discovery_interval: u64,
    #[schemars(description = "Maximum number of rendezvous registrations")]
    pub registrations_limit: usize,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct RelaySchema {
    #[schemars(description = "Maximum number of relay registrations")]
    pub registrations_limit: usize,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct AutonatSchema {
    #[schemars(description = "Minimum successful probes for NAT confidence")]
    pub confidence_threshold: usize,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct SyncSchema {
    #[schemars(description = "Sync timeout in milliseconds")]
    pub timeout_ms: u64,
    #[schemars(description = "Sync interval in milliseconds")]
    pub interval_ms: u64,
    #[schemars(description = "Sync frequency in milliseconds")]
    pub frequency_ms: u64,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct DataStoreSchema {
    #[schemars(description = "Path to data store directory")]
    pub path: String,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct BlobStoreSchema {
    #[schemars(description = "Path to blob store directory")]
    pub path: String,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct ContextSchema {
    #[schemars(description = "Context client configuration")]
    pub client: ClientConfigSchema,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct ClientConfigSchema {
    #[schemars(description = "Client parameters by protocol")]
    pub params: BTreeMap<String, ClientParamsSchema>,
    #[schemars(description = "Client signer configuration")]
    pub signer: ClientSignerSchema,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct ClientParamsSchema {
    #[schemars(description = "Selected signer (relayer or local)")]
    pub signer: String,
    #[schemars(description = "Network identifier")]
    pub network: String,
    #[schemars(description = "Contract ID")]
    pub contract_id: String,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct ClientSignerSchema {
    #[schemars(description = "Relayer signer configuration")]
    pub relayer: RelayerSignerSchema,
    #[schemars(description = "Local signer configuration")]
    pub local: LocalSignerSchema,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct RelayerSignerSchema {
    #[schemars(description = "Relayer URL")]
    pub url: String,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct LocalSignerSchema {
    #[schemars(description = "Local signers by protocol")]
    pub protocols: BTreeMap<String, ProtocolSignerSchema>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct ProtocolSignerSchema {
    #[schemars(description = "Signers by network")]
    pub signers: BTreeMap<String, LocalSignerConfigSchema>,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct LocalSignerConfigSchema {
    #[schemars(description = "RPC URL for the signer")]
    pub rpc_url: String,
    #[schemars(description = "Signer credentials")]
    pub credentials: CredentialsSchema,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CredentialsSchema {
    Near(NearCredentialsSchema),
    Starknet(StarknetCredentialsSchema),
    Icp(IcpCredentialsSchema),
    Ethereum(EthereumCredentialsSchema),
    Raw(RawCredentialsSchema),
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct NearCredentialsSchema {
    pub account_id: String,
    pub public_key: String,
    pub secret_key: String,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct StarknetCredentialsSchema {
    pub account_id: String,
    pub public_key: String,
    pub secret_key: String,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct IcpCredentialsSchema {
    pub account_id: String,
    pub public_key: String,
    pub secret_key: String,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct EthereumCredentialsSchema {
    pub account_id: String,
    pub secret_key: String,
}

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct RawCredentialsSchema {
    pub account_id: Option<String>,
    pub public_key: String,
    pub secret_key: String,
}

pub fn generate_schema() -> ConfigSchema {
    let schema = schema_for!(ConfigFile);
    let mut config_schema: ConfigSchema =
        serde_json::from_value(serde_json::to_value(schema).unwrap()).expect("valid config schema");

    // Ensure protocols section has proper defaults
    if config_schema.protocols.is_none() {
        config_schema.protocols = Some(ProtocolsSchema {
            ethereum: Some(EthereumProtocolSchema {
                network: "sepolia".to_owned(),
                contract_id: "".to_owned(),
                signer: "relayer".to_owned(),
                rpc_url: None,
                account_id: None,
                secret_key: None,
            }),
            icp: Some(IcpProtocolSchema {
                network: "local".to_owned(),
                contract_id: "".to_owned(),
                signer: "relayer".to_owned(),
                rpc_url: None,
                account_id: None,
                public_key: None,
                secret_key: None,
            }),
            near: Some(NearProtocolSchema {
                network: "testnet".to_owned(),
                contract_id: "".to_owned(),
                signer: "relayer".to_owned(),
                rpc_url: None,
                account_id: None,
                public_key: None,
                secret_key: None,
            }),
            stellar: Some(StellarProtocolSchema {
                network: "testnet".to_owned(),
                contract_id: "".to_owned(),
                signer: "relayer".to_owned(),
                rpc_url: None,
                account_id: None,
                public_key: None,
                secret_key: None,
            }),
        });
    }

    config_schema
}

pub fn get_field_hint(path: &[&str], schema: &ConfigSchema) -> Option<String> {
    if path.is_empty() {
        return None;
    }

    let mut current: &dyn std::any::Any = schema;

    // Handle both old context.config.* path and new protocols.* path
    if path.len() >= 3 && path[0] == "context" && path[1] == "config" {
        if let Some(protocol) = path.get(2) {
            if let Some(protocols) = &schema.protocols {
                match *protocol {
                    "ethereum" => {
                        if let Some(ethereum) = &protocols.ethereum {
                            return get_protocol_field_hint(&path[3..], "Ethereum", ethereum);
                        }
                    }
                    "icp" => {
                        if let Some(icp) = &protocols.icp {
                            return get_protocol_field_hint(&path[3..], "ICP", icp);
                        }
                    }
                    "near" => {
                        if let Some(near) = &protocols.near {
                            return get_protocol_field_hint(&path[3..], "NEAR", near);
                        }
                    }
                    "stellar" => {
                        if let Some(stellar) = &protocols.stellar {
                            return get_protocol_field_hint(&path[3..], "Stellar", stellar);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Handle new protocols.* path directly
    if path.len() >= 2 && path[0] == "protocols" {
        if let Some(protocols) = &schema.protocols {
            if let Some(protocol) = path.get(1) {
                let protocol_hint = match *protocol {
                    "ethereum" => {
                        "Ethereum protocol configuration (requires network, contract_id, signer)"
                    }
                    "icp" => "ICP protocol configuration (requires network, contract_id, signer)",
                    "near" => "NEAR protocol configuration (requires network, contract_id, signer)",
                    "stellar" => {
                        "Stellar protocol configuration (requires network, contract_id, signer)"
                    }
                    _ => return Some(format!("Unknown protocol '{}'", protocol)),
                };

                if path.len() == 2 {
                    return Some(protocol_hint.to_owned());
                }

                // Handle specific protocol fields
                if let Some(field) = path.get(2) {
                    let field_hint = match *field {
                        "network" => match *protocol {
                            "ethereum" => {
                                "Ethereum network name (e.g. sepolia, mainnet) - REQUIRED"
                            }
                            "icp" => "ICP network name (e.g. local, ic) - REQUIRED",
                            "near" => "NEAR network name (e.g. testnet, mainnet) - REQUIRED",
                            "stellar" => "Stellar network name (e.g. testnet, mainnet) - REQUIRED",
                            _ => "Network identifier - REQUIRED",
                        },
                        "contract_id" => "Contract address or identifier - REQUIRED",
                        "signer" => "Signer type (relayer or local) - REQUIRED",
                        "rpc_url" => "Optional RPC URL for direct connection",
                        "account_id" => "Optional account ID",
                        "public_key" => "Optional public key",
                        "secret_key" => "Optional secret key (use with caution!)",
                        _ => "Unknown field",
                    };

                    return Some(format!("{}: {}", field, field_hint));
                }
            }
        }
    }

    let mut current_path = Vec::new();

    for part in path {
        current_path.push(*part);

        // First try to downcast to each possible type and then match
        if let Some(config_schema) = current.downcast_ref::<ConfigSchema>() {
            match part.to_lowercase().as_str() {
                "identity" => {
                    current = &config_schema.identity;
                    continue;
                }
                "network" => {
                    current = &config_schema.network;
                    continue;
                }
                "sync" => {
                    current = &config_schema.sync;
                    continue;
                }
                "datastore" => {
                    current = &config_schema.datastore;
                    continue;
                }
                "blobstore" => {
                    current = &config_schema.blobstore;
                    continue;
                }
                "context" => {
                    current = &config_schema.context;
                    continue;
                }
                "protocols" => {
                    if let Some(protocols) = &config_schema.protocols {
                        current = protocols;
                        continue;
                    } else {
                        return Some("Protocol configurations (optional)".to_owned());
                    }
                }
                _ => {}
            }
        } else if let Some(protocols_schema) = current.downcast_ref::<ProtocolsSchema>() {
            match part.to_lowercase().as_str() {
                "ethereum" => {
                    if let Some(ethereum) = &protocols_schema.ethereum {
                        current = ethereum;
                        continue;
                    } else {
                        return Some("Ethereum protocol configuration (optional)".to_owned());
                    }
                }
                "icp" => {
                    if let Some(icp) = &protocols_schema.icp {
                        current = icp;
                        continue;
                    } else {
                        return Some("ICP protocol configuration (optional)".to_owned());
                    }
                }
                "near" => {
                    if let Some(near) = &protocols_schema.near {
                        current = near;
                        continue;
                    } else {
                        return Some("NEAR protocol configuration (optional)".to_owned());
                    }
                }
                "stellar" => {
                    if let Some(stellar) = &protocols_schema.stellar {
                        current = stellar;
                        continue;
                    } else {
                        return Some("Stellar protocol configuration (optional)".to_owned());
                    }
                }
                _ => {}
            }
        } else if let Some(ethereum_schema) = current.downcast_ref::<EthereumProtocolSchema>() {
            match part.to_lowercase().as_str() {
                "network" => {
                    return Some("Ethereum network name (e.g. sepolia, mainnet)".to_owned())
                }
                "contract_id" => return Some("Ethereum contract address".to_owned()),
                "signer" => return Some("Signer type (relayer or local)".to_owned()),
                "rpc_url" => return Some("Optional RPC URL for direct connection".to_owned()),
                "account_id" => return Some("Optional account ID".to_owned()),
                "secret_key" => return Some("Optional secret key".to_owned()),
                _ => {}
            }
        } else if let Some(icp_schema) = current.downcast_ref::<IcpProtocolSchema>() {
            match part.to_lowercase().as_str() {
                "network" => return Some("ICP network name (e.g. local, ic)".to_owned()),
                "contract_id" => return Some("ICP contract address".to_owned()),
                "signer" => return Some("Signer type (relayer or local)".to_owned()),
                "rpc_url" => return Some("Optional RPC URL for direct connection".to_owned()),
                "account_id" => return Some("Optional account ID".to_owned()),
                "public_key" => return Some("Optional public key".to_owned()),
                "secret_key" => return Some("Optional secret key".to_owned()),
                _ => {}
            }
        } else if let Some(near_schema) = current.downcast_ref::<NearProtocolSchema>() {
            match part.to_lowercase().as_str() {
                "network" => return Some("NEAR network name (e.g. testnet, mainnet)".to_owned()),
                "contract_id" => return Some("NEAR contract address".to_owned()),
                "signer" => return Some("Signer type (relayer or local)".to_owned()),
                "rpc_url" => return Some("Optional RPC URL for direct connection".to_owned()),
                "account_id" => return Some("Optional account ID".to_owned()),
                "public_key" => return Some("Optional public key".to_owned()),
                "secret_key" => return Some("Optional secret key".to_owned()),
                _ => {}
            }
        } else if let Some(stellar_schema) = current.downcast_ref::<StellarProtocolSchema>() {
            match part.to_lowercase().as_str() {
                "network" => {
                    return Some("Stellar network name (e.g. testnet, mainnet)".to_owned())
                }
                "contract_id" => return Some("Stellar contract address".to_owned()),
                "signer" => return Some("Signer type (relayer or local)".to_owned()),
                "rpc_url" => return Some("Optional RPC URL for direct connection".to_owned()),
                "account_id" => return Some("Optional account ID".to_owned()),
                "public_key" => return Some("Optional public key".to_owned()),
                "secret_key" => return Some("Optional secret key".to_owned()),
                _ => {}
            }
        } else if let Some(identity_schema) = current.downcast_ref::<IdentitySchema>() {
            match part.to_lowercase().as_str() {
                "peer_id" => return Some("Peer ID in base58 format".to_owned()),
                "keypair" => {
                    return Some("Keypair in protobuf format encoded as base58".to_owned())
                }
                _ => {}
            }
        } else if let Some(network_schema) = current.downcast_ref::<NetworkSchema>() {
            match part.to_lowercase().as_str() {
                "swarm" => {
                    current = &network_schema.swarm;
                    continue;
                }
                "server" => {
                    current = &network_schema.server;
                    continue;
                }
                "bootstrap" => {
                    current = &network_schema.bootstrap;
                    continue;
                }
                "discovery" => {
                    current = &network_schema.discovery;
                    continue;
                }
                _ => {}
            }
        } else if let Some(swarm_schema) = current.downcast_ref::<SwarmSchema>() {
            if part.to_lowercase().as_str() == "listen" {
                return Some("List of multiaddresses to listen on".to_owned());
            }
        } else if let Some(server_schema) = current.downcast_ref::<ServerSchema>() {
            match part.to_lowercase().as_str() {
                "listen" => {
                    return Some("List of multiaddresses for RPC server to listen on".to_owned());
                }
                "admin" => {
                    if let Some(admin) = &server_schema.admin {
                        current = admin;
                        continue;
                    } else {
                        return Some("Admin API configuration (optional)".to_owned());
                    }
                }
                "jsonrpc" => {
                    if let Some(jsonrpc) = &server_schema.jsonrpc {
                        current = jsonrpc;
                        continue;
                    } else {
                        return Some("JSON-RPC configuration (optional)".to_owned());
                    }
                }
                "websocket" => {
                    if let Some(websocket) = &server_schema.websocket {
                        current = websocket;
                        continue;
                    } else {
                        return Some("WebSocket configuration (optional)".to_owned());
                    }
                }
                _ => {}
            }
        } else if let Some(admin_schema) = current.downcast_ref::<AdminSchema>() {
            if part.to_lowercase().as_str() == "enabled" {
                return Some("Whether admin API is enabled (boolean)".to_owned());
            }
        } else if let Some(jsonrpc_schema) = current.downcast_ref::<JsonRpcSchema>() {
            if part.to_lowercase().as_str() == "enabled" {
                return Some("Whether JSON-RPC is enabled (boolean)".to_owned());
            }
        } else if let Some(ws_schema) = current.downcast_ref::<WsSchema>() {
            if part.to_lowercase().as_str() == "enabled" {
                return Some("Whether WebSocket is enabled (boolean)".to_owned());
            }
        } else if let Some(bootstrap_schema) = current.downcast_ref::<BootstrapSchema>() {
            if part.to_lowercase().as_str() == "nodes" {
                current = &bootstrap_schema.nodes;
                continue;
            }
        } else if let Some(bootstrap_nodes_schema) = current.downcast_ref::<BootstrapNodesSchema>()
        {
            if part.to_lowercase().as_str() == "list" {
                return Some("List of bootstrap node multiaddresses".to_owned());
            }
        } else if let Some(discovery_schema) = current.downcast_ref::<DiscoverySchema>() {
            match part.to_lowercase().as_str() {
                "mdns" => return Some("Whether mDNS discovery is enabled (boolean)".to_owned()),
                "advertise_address" => {
                    return Some("Whether to advertise observed address (boolean)".to_owned());
                }
                "rendezvous" => {
                    current = &discovery_schema.rendezvous;
                    continue;
                }
                "relay" => {
                    current = &discovery_schema.relay;
                    continue;
                }
                "autonat" => {
                    current = &discovery_schema.autonat;
                    continue;
                }
                _ => {}
            }
        } else if let Some(rendezvous_schema) = current.downcast_ref::<RendezvousSchema>() {
            match part.to_lowercase().as_str() {
                "namespace" => return Some("Rendezvous namespace (string)".to_owned()),
                "discovery_rpm" => {
                    return Some("Discovery requests per minute (float)".to_owned());
                }
                "discovery_interval" => {
                    return Some("Discovery interval in seconds (integer)".to_owned());
                }
                "registrations_limit" => {
                    return Some("Maximum number of rendezvous registrations (integer)".to_owned());
                }
                _ => {}
            }
        } else if let Some(relay_schema) = current.downcast_ref::<RelaySchema>() {
            if part.to_lowercase().as_str() == "registrations_limit" {
                return Some("Maximum number of relay registrations (integer)".to_owned());
            }
        } else if let Some(autonat_schema) = current.downcast_ref::<AutonatSchema>() {
            if part.to_lowercase().as_str() == "confidence_threshold" {
                return Some("Minimum successful probes for NAT confidence (integer)".to_owned());
            }
        } else if let Some(sync_schema) = current.downcast_ref::<SyncSchema>() {
            match part.to_lowercase().as_str() {
                "timeout_ms" => {
                    return Some("Sync timeout in milliseconds (integer)".to_owned());
                }
                "interval_ms" => {
                    return Some("Sync interval in milliseconds (integer)".to_owned());
                }
                "frequency_ms" => {
                    return Some("Sync frequency in milliseconds (integer)".to_owned());
                }
                _ => {}
            }
        } else if let Some(datastore_schema) = current.downcast_ref::<DataStoreSchema>() {
            if part.to_lowercase().as_str() == "path" {
                return Some("Path to data store directory (string)".to_owned());
            }
        } else if let Some(blobstore_schema) = current.downcast_ref::<BlobStoreSchema>() {
            if part.to_lowercase().as_str() == "path" {
                return Some("Path to blob store directory (string)".to_owned());
            }
        } else if let Some(context_schema) = current.downcast_ref::<ContextSchema>() {
            if part.to_lowercase().as_str() == "client" {
                current = &context_schema.client;
                continue;
            }
        } else if let Some(client_config_schema) = current.downcast_ref::<ClientConfigSchema>() {
            match part.to_lowercase().as_str() {
                "params" => {
                    current = &client_config_schema.params;
                    continue;
                }
                "signer" => {
                    current = &client_config_schema.signer;
                    continue;
                }
                _ => {}
            }
        } else if let Some(client_signer_schema) = current.downcast_ref::<ClientSignerSchema>() {
            match part.to_lowercase().as_str() {
                "relayer" => {
                    current = &client_signer_schema.relayer;
                    continue;
                }
                "local" => {
                    current = &client_signer_schema.local;
                    continue;
                }
                _ => {}
            }
        } else if let Some(relayer_signer_schema) = current.downcast_ref::<RelayerSignerSchema>() {
            if part.to_lowercase().as_str() == "url" {
                return Some("Relayer URL (string)".to_owned());
            }
        } else if let Some(local_signer_schema) = current.downcast_ref::<LocalSignerSchema>() {
            if part.to_lowercase().as_str() == "protocols" {
                current = &local_signer_schema.protocols;
                continue;
            }
        } else if let Some(params_map) =
            current.downcast_ref::<BTreeMap<String, ClientParamsSchema>>()
        {
            if let Some(_) = params_map.get(*part) {
                return Some(format!(
                    "Client parameters for protocol '{}' (network: string, contract_id: string, signer: 'relayer'|'local')",
                    part
                ));
            }
        } else if let Some(protocols_map) =
            current.downcast_ref::<BTreeMap<String, ProtocolSignerSchema>>()
        {
            if let Some(_) = protocols_map.get(*part) {
                return Some(format!(
                    "Local signers for protocol '{}' (network: string, rpc_url: string, credentials: object)",
                    part
                ));
            }
        }

        return Some(format!(
            "Unknown configuration path: {}",
            current_path.join(".")
        ));
    }

    None
}

fn get_protocol_field_hint<T: JsonSchema + Serialize>(
    path: &[&str],
    protocol_name: &str,
    schema: &T,
) -> Option<String> {
    if path.is_empty() {
        return Some(format!("{} protocol configuration", protocol_name));
    }

    let schema_value = serde_json::to_value(schema).ok()?;
    let schema_obj = schema_value.as_object()?;

    for (i, part) in path.iter().enumerate() {
        if let Some(field_schema) = schema_obj.get(*part) {
            if i == path.len() - 1 {
                return field_schema
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_owned());
            }

            if let Some(sub_schema) = field_schema.get("properties") {
                if let Some(sub_schema) = sub_schema.as_object() {
                    if let Some(next_schema) = sub_schema.get(path[i + 1]) {
                        continue;
                    }
                }
            }
        }
    }

    None
}
