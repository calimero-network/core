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
}

impl From<schemars::Schema> for ConfigSchema {
    fn from(schema: schemars::Schema) -> Self {
        let schema = schema_for!(ConfigFile);
        serde_json::from_value(serde_json::to_value(schema).unwrap()).expect("valid config schema")
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
    schema_for!(ConfigFile).into()
}

pub fn get_field_hint(path: &[&str], schema: &ConfigSchema) -> Option<String> {
    if path.is_empty() {
        return None;
    }

    let mut current: &dyn std::any::Any = schema;

    if path[0] == "identity" {
        if path.len() == 1 {
            return Some("Peer identity configuration (optional)".to_owned());
        }
        if let Some(identity) = &schema.identity {
            current = identity;
        } else {
            return Some("Peer identity configuration (currently not set)".to_owned());
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
