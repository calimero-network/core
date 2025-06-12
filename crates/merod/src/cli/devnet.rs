use std::path::PathBuf;

use calimero_sandbox::config::DevnetConfig;
use calimero_sandbox::Devnet;
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::Result as EyreResult;

use super::RootArgs;

#[derive(Debug, Parser)]
pub struct DevnetCommand {
    #[command(subcommand)]
    pub action: DevnetSubcommand,
}

impl DevnetCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        match self.action {
            DevnetSubcommand::Start(cmd) => cmd.run(root_args).await,
            DevnetSubcommand::Stop(cmd) => cmd.run(root_args).await,
            DevnetSubcommand::Status(cmd) => cmd.run(root_args).await,
        }
    }
}

#[derive(Debug, Parser)]
pub enum DevnetSubcommand {
    /// Start the devnet
    Start(DevnetStartCommand),
    /// Stop the devnet
    Stop(DevnetStopCommand),
    /// Get devnet status
    Status(DevnetStatusCommand),
}

#[derive(Debug, Parser)]
pub struct DevnetStartCommand {
    /// Number of nodes to start
    #[clap(long, default_value = "3")]
    pub node_count: u32,

    /// Protocols to enable
    #[clap(
        long,
        value_delimiter = ',',
        default_value = "near,ethereum,icp,stellar"
    )]
    pub protocols: Vec<String>,

    /// Host to listen on for swarm
    #[clap(long, default_value = "127.0.0.1")]
    pub swarm_host: String,

    /// Starting port for swarm
    #[clap(long, default_value = "2528")]
    pub swarm_port: u16,

    /// Host to listen on for RPC
    #[clap(long, default_value = "127.0.0.1")]
    pub server_host: String,

    /// Starting port for RPC
    #[clap(long, default_value = "2428")]
    pub server_port: u16,

    /// Directory to store node data
    #[clap(long)]
    pub home_dir: Option<PathBuf>,
}

impl DevnetStartCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let home_dir = self
            .home_dir
            .map(|p| Utf8PathBuf::from_path_buf(p).unwrap())
            .unwrap_or(root_args.home);

        let config = DevnetConfig {
            node_count: self.node_count,
            protocols: self.protocols,
            swarm_host: self.swarm_host,
            start_swarm_port: self.swarm_port,
            server_host: self.server_host,
            start_server_port: self.server_port,
            home_dir,
            node_name: root_args.node_name,
        };

        let mut devnet = Devnet::new(config);
        devnet.start().await
    }
}

#[derive(Debug, Parser)]
pub struct DevnetStopCommand {
    /// Node names to stop (default: all)
    #[clap(long, value_delimiter = ',')]
    pub nodes: Vec<String>,
}

impl DevnetStopCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let devnet = Devnet::load(root_args.home.into()).await?;
        devnet.stop().await
    }
}

#[derive(Debug, Parser)]
pub struct DevnetStatusCommand {
    /// Node names to check (default: all)
    #[clap(long, value_delimiter = ',')]
    pub nodes: Vec<String>,
}

impl DevnetStatusCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let devnet = Devnet::load(root_args.home.into()).await?;
        let status = devnet.status().await?;

        println!("Devnet status:");
        for (node, running) in status {
            println!(
                "  {}: {}",
                node,
                if running { "running" } else { "stopped" }
            );
        }

        Ok(())
    }
}

#[async_trait::async_trait]
trait DevnetLoader {
    async fn load(home_dir: PathBuf) -> EyreResult<Devnet>;
}

#[async_trait::async_trait]
impl DevnetLoader for Devnet {
    async fn load(home_dir: PathBuf) -> EyreResult<Devnet> {
        let home_dir = Utf8PathBuf::from_path_buf(home_dir)
            .map_err(|e| eyre::eyre!("Invalid UTF-8 path: {}", e.display()))?;
        Ok(Devnet::new(DevnetConfig::new(
            0,
            vec![],
            "127.0.0.1".to_owned(),
            0,
            "127.0.0.1".to_owned(),
            0,
            home_dir,
            "devnet".into(),
        )))
    }
}
