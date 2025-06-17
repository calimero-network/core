use calimero_sandbox::config::DevnetConfig;
use calimero_sandbox::Devnet;
use camino::Utf8PathBuf;
use clap::Parser;
use const_format::concatcp;

use super::RootArgs;

pub const EXAMPLES: &str = r"
# Start a local devnet with 5 nodes
$ merod devnet start --node-count 5

# Check devnet status
$ merod devnet status

# Stop devnet
$ merod devnet stop
";

#[derive(Debug, Parser)]
#[command(after_help = concatcp!(
    "Environment variables:\n",
    "Examples:",
    EXAMPLES
))]
pub struct DevnetCommand {
    /// Number of nodes to start
    #[clap(long, default_value = "3")]
    pub node_count: u32,

    /// Host to listen on for swarm
    #[clap(long, default_value = "127.0.0.1")]
    pub swarm_host: String,

    /// Starting port for swarm
    #[clap(long, default_value = "2428")]
    pub swarm_port: u16,

    /// Host to listen on for RPC
    #[clap(long, default_value = "127.0.0.1")]
    pub server_host: String,

    /// Starting port for RPC
    #[clap(long, default_value = "2528")]
    pub server_port: u16,

    /// Directory to store node data
    #[clap(long)]
    pub home_dir: Option<Utf8PathBuf>,
}

impl DevnetCommand {
    pub async fn run(self, root_args: RootArgs) -> eyre::Result<()> {
        let home_dir = self.home_dir.unwrap_or(root_args.home);

        let config = DevnetConfig {
            node_count: self.node_count,
            protocols: vec![],
            swarm_host: self.swarm_host,
            start_swarm_port: self.swarm_port,
            server_host: self.server_host,
            start_server_port: self.server_port,
            home_dir,
            node_name: root_args.node_name,
        };

        let mut devnet = Devnet::new(config);
        devnet.start().await?;

        tokio::signal::ctrl_c().await?;
        Ok(())
    }
}
