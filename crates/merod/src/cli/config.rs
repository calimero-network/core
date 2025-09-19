#![allow(unused_results, reason = "Occurs in macro")]

use std::env::temp_dir;
use std::str::FromStr;

use calimero_config::{ConfigFile, CONFIG_FILE};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, eyre, Result as EyreResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs::{read_to_string, write};
use toml_edit::{DocumentMut, Item, Value};
use tracing::info;

use crate::cli;

/// Configure the node
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// Key-value pairs to be added or updated in the TOML file, or keys with ? for hints
    #[clap(value_name = "ARGS")]
    args: Vec<String>,

    /// Output format for printing
    #[clap(long, value_name = "FORMAT", default_value = "default")]
    #[clap(value_enum)]
    print: PrintFormat,

    /// Save modifications to config file
    #[clap(short, long)]
    save: bool,
}

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
enum PrintFormat {
    Default,
    Toml,
    Json,
    Human,
}

#[derive(Clone, Debug)]
enum ConfigArg {
    Mutation { key: String, value: Value },
    Hint { key: String },
}

impl FromStr for ConfigArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.ends_with('?') {
            let key = s.trim_end_matches('?').to_owned();
            if key.is_empty() {
                return Err("Empty key for hint".to_owned());
            }
            return Ok(ConfigArg::Hint { key });
        }

        let mut parts = s.splitn(2, '=');
        let key = parts.next().ok_or("Missing key")?.to_owned();

        let value = parts.next().ok_or("Missing value")?;
        let value = Value::from_str(value).map_err(|e| e.to_string())?;

        Ok(ConfigArg::Mutation { key, value })
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct ConfigSchema {
    identity: IdentitySchema,
    network: NetworkSchema,
    sync: SyncSchema,
    datastore: DataStoreSchema,
    blobstore: BlobStoreSchema,
    context: ContextSchema,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct IdentitySchema {
    peer_id: String,
    keypair: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct NetworkSchema {
    swarm: SwarmSchema,
    server: ServerSchema,
    bootstrap: BootstrapSchema,
    discovery: DiscoverySchema,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct SwarmSchema {
    listen: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct ServerSchema {
    listen: Vec<String>,
    admin: Option<AdminSchema>,
    jsonrpc: Option<JsonRpcSchema>,
    websocket: Option<WsSchema>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct AdminSchema {
    enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct JsonRpcSchema {
    enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct WsSchema {
    enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct BootstrapSchema {
    nodes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct DiscoverySchema {
    mdns: bool,
    advertise_address: bool,
    rendezvous: RendezvousSchema,
    relay: RelaySchema,
    autonat: AutonatSchema,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct RendezvousSchema {
    registrations_limit: usize,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct RelaySchema {
    registrations_limit: usize,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct AutonatSchema {
    confidence_threshold: usize,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct SyncSchema {
    timeout_ms: u64,
    interval_ms: u64,
    frequency_ms: u64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct DataStoreSchema {
    path: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct BlobStoreSchema {
    path: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct ContextSchema {
    client: ClientSchema,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct ClientSchema {
    #[serde(flatten)]
    params: std::collections::BTreeMap<String, ClientParamsSchema>,
    signer: SignerSchema,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct ClientParamsSchema {
    signer: String,
    network: String,
    contract_id: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct SignerSchema {
    relayer: RelayerSchema,
    #[serde(rename = "self")]
    local: LocalSchema,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct RelayerSchema {
    url: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct LocalSchema {
    #[serde(flatten)]
    protocols: std::collections::BTreeMap<String, ProtocolSchema>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct ProtocolSchema {
    #[serde(flatten)]
    signers: std::collections::BTreeMap<String, SignerConfigSchema>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct SignerConfigSchema {
    rpc_url: String,
    #[serde(flatten)]
    credentials: CredentialsSchema,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
enum CredentialsSchema {
    Near(NearCredentialsSchema),
    Starknet(StarknetCredentialsSchema),
    Icp(IcpCredentialsSchema),
    Ethereum(EthereumCredentialsSchema),
    Raw(RawCredentialsSchema),
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct NearCredentialsSchema {
    account_id: String,
    public_key: String,
    secret_key: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct StarknetCredentialsSchema {
    account_id: String,
    public_key: String,
    secret_key: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct IcpCredentialsSchema {
    account_id: String,
    public_key: String,
    secret_key: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct EthereumCredentialsSchema {
    account_id: String,
    secret_key: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
struct RawCredentialsSchema {
    account_id: Option<String>,
    public_key: String,
    secret_key: String,
}

impl ConfigCommand {
    pub async fn run(self, root_args: &cli::RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config_path = path.join(CONFIG_FILE);

        // Load the existing TOML file
        let toml_str = read_to_string(&config_path)
            .await
            .map_err(|_| eyre!("Node is not initialized in {:?}", config_path))?;

        let mut doc = toml_str.parse::<DocumentMut>()?;

        // Parse arguments
        let mut mutations = Vec::new();
        let mut hints = Vec::new();
        let mut has_hints = false;

        // Use a reference to avoid moving self.args
        for arg in &self.args {
            match ConfigArg::from_str(arg) {
                Ok(ConfigArg::Mutation { key, value }) => {
                    if has_hints {
                        eprintln!(
                            "Warning: Ignoring mutation '{}' because hints are present",
                            key
                        );
                        continue;
                    }
                    mutations.push((key, value));
                }
                Ok(ConfigArg::Hint { key }) => {
                    has_hints = true;
                    hints.push(key);
                }
                Err(err) => {
                    bail!("Invalid argument '{}': {}", arg, err);
                }
            }
        }

        // Handle hints
        if has_hints {
            return self.handle_hints(&hints).await;
        }

        // Handle printing only (no mutations)
        if mutations.is_empty() {
            return self.print_config(&doc, &hints).await;
        }

        // Handle mutations
        let original_doc = doc.clone();

        for (key, value) in mutations {
            let key_parts: Vec<&str> = key.split('.').collect();
            let mut current = doc.as_item_mut();

            for key in &key_parts[..key_parts.len() - 1] {
                current = &mut current[key];
            }

            current[key_parts[key_parts.len() - 1]] = Item::Value(value);
        }

        // Validate the modified config
        self.validate_toml(&doc).await?;

        // Show diff
        self.show_diff(&original_doc, &doc).await?;

        // Save if requested
        if self.save {
            write(&config_path, doc.to_string()).await?;
            info!("Node configuration has been updated");
        } else {
            eprintln!(
                "\nnote: if this looks right, use `-s, --save` to persist these modifications"
            );
        }

        Ok(())
    }

    async fn handle_hints(&self, hints: &[String]) -> EyreResult<()> {
        // For now, provide basic hints without complex schema traversal
        for hint_key in hints {
            println!(
                "{}: <config value> # Use '?' suffix to get hints about config keys",
                hint_key
            );
            println!("  Available top-level keys: identity, network, sync, datastore, blobstore, context");
            println!("  Example: merod config network?");
            println!("  Example: merod config sync.interval_ms?");
        }
        Ok(())
    }

    async fn print_config(&self, doc: &DocumentMut, keys: &[String]) -> EyreResult<()> {
        if keys.is_empty() {
            // Print full config
            match self.print {
                PrintFormat::Default | PrintFormat::Toml => {
                    println!("{}", doc.to_string());
                }
                PrintFormat::Json => {
                    let value: serde_json::Value = toml::from_str(&doc.to_string())?;
                    println!("{}", serde_json::to_string_pretty(&value)?);
                }
                PrintFormat::Human => {
                    // For human-readable format, fall back to TOML
                    println!("{}", doc.to_string());
                }
            }
        } else {
            for key in keys {
                let key_parts: Vec<&str> = key.split('.').collect();
                let mut current = doc.as_item();

                for part in &key_parts {
                    current = &current[*part];
                }

                if !current.is_none() {
                    let mut result = DocumentMut::new();

                    let last_part = key_parts.last().unwrap();
                    result[last_part] = current.clone();

                    match self.print {
                        PrintFormat::Default | PrintFormat::Toml => {
                            println!("{} = {}", key, result.to_string().trim());
                        }
                        PrintFormat::Json => {
                            let value: serde_json::Value = toml::from_str(&result.to_string())?;
                            println!("{}: {}", key, serde_json::to_string(&value[last_part])?);
                        }
                        PrintFormat::Human => {
                            println!("{} = {}", key, result.to_string().trim());
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn show_diff(&self, original: &DocumentMut, modified: &DocumentMut) -> EyreResult<()> {
        let original_str = original.to_string();
        let modified_str = modified.to_string();

        match self.print {
            PrintFormat::Default | PrintFormat::Human => {
                let original_lines: Vec<&str> = original_str.lines().collect();
                let modified_lines: Vec<&str> = modified_str.lines().collect();

                for i in 0..std::cmp::max(original_lines.len(), modified_lines.len()) {
                    let original_line = original_lines.get(i).unwrap_or(&"");
                    let modified_line = modified_lines.get(i).unwrap_or(&"");

                    if original_line != modified_line {
                        if !original_line.is_empty() {
                            println!("-{}", original_line);
                        }
                        if !modified_line.is_empty() {
                            println!("+{}", modified_line);
                        }
                    } else {
                        println!(" {}", original_line);
                    }
                }
            }
            PrintFormat::Toml => {
                println!("{}", modified_str);
            }
            PrintFormat::Json => {
                let value: serde_json::Value = toml::from_str(&modified_str)?;
                println!("{}", serde_json::to_string_pretty(&value)?);
            }
        }

        Ok(())
    }

    pub async fn validate_toml(&self, doc: &DocumentMut) -> EyreResult<()> {
        let tmp_dir = temp_dir();
        let tmp_path = tmp_dir.join(CONFIG_FILE);

        write(&tmp_path, doc.to_string()).await?;

        let tmp_path_utf8 = Utf8PathBuf::try_from(tmp_dir)?;

        drop(ConfigFile::load(&tmp_path_utf8).await?);

        Ok(())
    }
}
