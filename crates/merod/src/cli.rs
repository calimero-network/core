use calimero_version::CalimeroVersion;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::defaults;

mod auth_mode;
mod config;
mod init;
mod run;

use config::ConfigCommand;
pub use init::ConfigProtocol;
use init::InitCommand;
use run::RunCommand;

pub const EXAMPLES: &str = concat!(
    r"
  # Initialize node
  $ merod --node node1 init --server-port 2428 --swarm-port 2528

  # Initialize node with a custom home directory data
  $ mkdir data
  $ merod --home data/ --node node1 init

  # Configure an existing node (key=value; use TOML paths).
  # Quote the argument in zsh so [ ] are not globbed:
  $ merod --node node1 config ",
    "\"",
    r"server.listen=['/ip4/127.0.0.2/tcp/3000', '/ip6/::1/tcp/3000']",
    "\"",
    r"

  # Run a node
  $ merod --node node1 run
",
);

#[derive(Debug, Parser)]
#[command(author, version = CalimeroVersion::current_str(), about, long_about = None)]
#[command(after_help = concatcp!(
    "Environment variables:\n",
    "  CALIMERO_HOME    Directory for config and data\n\n",
    "  NEAR_API_KEY     NEAR API key for blockchain operations\n\n",
    "Examples:",
    EXAMPLES
))]
pub struct RootCommand {
    #[command(flatten)]
    pub args: RootArgs,

    #[command(subcommand)]
    pub action: SubCommands,
}

#[derive(Debug, Subcommand)]
pub enum SubCommands {
    Config(ConfigCommand),
    Init(InitCommand),
    #[command(alias = "up")]
    Run(RunCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: Utf8PathBuf,

    /// Name of node
    #[arg(short = 'n', long = "node", value_name = "NAME")]
    pub node_name: Utf8PathBuf,
}

impl RootCommand {
    pub async fn run(self) -> EyreResult<()> {
        match self.action {
            SubCommands::Config(config) => config.run(&self.args).await,
            SubCommands::Init(init) => init.run(self.args).await,
            SubCommands::Run(run) => run.run(self.args).await,
        }
    }
}
