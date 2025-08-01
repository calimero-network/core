use calimero_sandbox::config::{DevnetConfig, ProtocolConfigs};
use calimero_sandbox::protocol::ethereum::EthereumProtocolConfig;
use calimero_sandbox::protocol::icp::IcpProtocolConfig;
use calimero_sandbox::protocol::near::NearProtocolConfig;
use calimero_sandbox::protocol::stellar::StellarProtocolConfig;
use calimero_sandbox::Devnet;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use const_format::concatcp;
use serde::{Deserialize, Serialize};
use tokio::fs;

use super::RootArgs;

pub const EXAMPLES: &str = r"
# Initialize a devnet config file
$ merod devnet init --node-count 5 -o devnet.json

# Run devnet from config file  
$ merod devnet run -c devnet.json

# Run devnet with inline config
$ merod devnet run --node-count 3 --swarm-port 2428 --server-port 2528
";

#[derive(Debug, Parser)]
#[command(after_help = concatcp!(
    "Environment variables:\n",
    "Examples:",
    EXAMPLES
))]
pub struct DevnetCommand {
    #[command(subcommand)]
    pub action: DevnetSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum DevnetSubcommand {
    /// Initialize a devnet config file
    Init(InitCommand),
    /// Run a devnet
    Run(RunCommand),
}

#[derive(Debug, Parser)]
pub struct InitCommand {
    /// Number of nodes to start
    #[clap(long, default_value = "3")]
    pub node_count: u32,

    /// List of protocols to enable
    #[clap(long, value_delimiter = ',')]
    pub protocols: Vec<String>,

    /// Host for swarm connections
    #[clap(long, default_value = "127.0.0.1")]
    pub swarm_host: String,

    /// Starting port for swarm
    #[clap(long, default_value = "2428")]
    pub swarm_port: u16,

    /// Host for RPC servers
    #[clap(long, default_value = "127.0.0.1")]
    pub server_host: String,

    /// Starting port for RPC servers  
    #[clap(long, default_value = "2528")]
    pub server_port: u16,

    /// Output config file path
    #[clap(short, long, default_value = "devnet.json")]
    pub output: Utf8PathBuf,
}

#[derive(Debug, Parser)]
pub struct RunCommand {
    /// Config file path
    #[clap(short, long)]
    pub config: Option<Utf8PathBuf>,

    /// Number of nodes to start
    #[clap(long)]
    pub node_count: Option<u32>,

    /// Host for swarm connections
    #[clap(long)]
    pub swarm_host: Option<String>,

    /// Starting port for swarm
    #[clap(long)]
    pub swarm_port: Option<u16>,

    /// Host for RPC servers
    #[clap(long)]
    pub server_host: Option<String>,

    /// Starting port for RPC servers
    #[clap(long)]
    pub server_port: Option<u16>,

    /// Directory for node data
    #[clap(long)]
    pub home_dir: Option<Utf8PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DevnetConfigFile {
    node_count: u32,
    protocols: Vec<String>,
    swarm_host: String,
    swarm_port: u16,
    server_host: String,
    server_port: u16,
}

impl DevnetCommand {
    pub async fn run(self, root_args: Option<RootArgs>) -> eyre::Result<()> {
        match self.action {
            DevnetSubcommand::Init(cmd) => {
                let config = DevnetConfigFile {
                    node_count: cmd.node_count,
                    protocols: cmd.protocols,
                    swarm_host: cmd.swarm_host,
                    swarm_port: cmd.swarm_port,
                    server_host: cmd.server_host,
                    server_port: cmd.server_port,
                };

                let config_str = serde_json::to_string_pretty(&config)?;
                fs::write(&cmd.output, config_str).await?;
                Ok(())
            }
            DevnetSubcommand::Run(cmd) => {
                let config = if let Some(config_path) = cmd.config {
                    let config_str = fs::read_to_string(&config_path).await?;
                    let config_file: DevnetConfigFile = serde_json::from_str(&config_str)?;

                    // Load protocol configs from separate files
                    let protocol_configs = ProtocolConfigs {
                        near: load_protocol_config(
                            &config_path.parent().unwrap().join("near.json"),
                        )
                        .await?,
                        icp: load_protocol_config(&config_path.parent().unwrap().join("icp.json"))
                            .await?,
                        stellar: load_protocol_config(
                            &config_path.parent().unwrap().join("stellar.json"),
                        )
                        .await?,
                        ethereum: load_protocol_config(
                            &config_path.parent().unwrap().join("ethereum.json"),
                        )
                        .await?,
                    };

                    DevnetConfig {
                        node_count: config_file.node_count,
                        protocols: config_file.protocols,
                        protocol_configs,
                        swarm_host: config_file.swarm_host,
                        start_swarm_port: config_file.swarm_port,
                        server_host: config_file.server_host,
                        start_server_port: config_file.server_port,
                        home_dir: cmd.home_dir.unwrap_or_else(|| {
                            root_args
                                .as_ref()
                                .map_or_else(|| Utf8PathBuf::from("."), |args| args.home.clone())
                        }),
                        node_name: root_args
                            .as_ref()
                            .map_or_else(|| "devnet".into(), |args| args.node_name.clone()),
                    }
                } else {
                    // Default configs for inline usage
                    DevnetConfig {
                        node_count: cmd.node_count.unwrap_or(3),
                        protocols: vec!["near".into()], // Default to near
                        protocol_configs: ProtocolConfigs {
                            near: NearProtocolConfig::default(),
                            icp: IcpProtocolConfig::default(),
                            stellar: StellarProtocolConfig::default(),
                            ethereum: EthereumProtocolConfig::default(),
                        },
                        swarm_host: cmd.swarm_host.unwrap_or_else(|| "127.0.0.1".into()),
                        start_swarm_port: cmd.swarm_port.unwrap_or(2428),
                        server_host: cmd.server_host.unwrap_or_else(|| "127.0.0.1".into()),
                        start_server_port: cmd.server_port.unwrap_or(2528),
                        home_dir: cmd.home_dir.unwrap_or_else(|| {
                            root_args
                                .as_ref()
                                .map_or_else(|| Utf8PathBuf::from("."), |args| args.home.clone())
                        }),
                        node_name: root_args
                            .as_ref()
                            .map_or_else(|| "devnet".into(), |args| args.node_name.clone()),
                    }
                };

                let mut devnet = Devnet::new(config)?;
                devnet.start().await?;

                tokio::signal::ctrl_c().await?;
                Ok(())
            }
        }
    }
}

async fn load_protocol_config<T: serde::de::DeserializeOwned>(
    path: &Utf8PathBuf,
) -> eyre::Result<T> {
    let config_str = fs::read_to_string(path).await?;
    Ok(serde_json::from_str(&config_str)?)
}
