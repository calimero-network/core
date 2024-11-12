use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::defaults;

mod config;
mod init;
mod relay;
mod run;

use config::ConfigCommand;
use init::InitCommand;
use relay::RelayCommand;
use run::RunCommand;

pub const EXAMPLES: &str = r"
  # Initialize a new node
  $ merod --home data/ --node-name node1 init

  # Configure an existing node
  $ merod --home data/ --node-name node1 config --server-host 143.34.182.202 --server-port 3000

  # Run a node as a peer
  $ merod --home data/ --node-name node1 run

  # Run a node as a coordinator
  $ merod --home data/ --node-name node1 run --node-type coordinator
";

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
#[command(after_help = concatcp!(
    "Environment variables:\n",
    "  CALIMERO_HOME    Directory for config and data\n\n",
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
    #[command(alias = "call")]
    Relay(RelayCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: Utf8PathBuf,

    /// Name of node
    #[arg(short, long, value_name = "NAME")]
    pub node_name: Utf8PathBuf,
}

impl RootCommand {
    pub async fn run(self) -> EyreResult<()> {
        match self.action {
            SubCommands::Config(config) => config.run(&self.args),
            SubCommands::Init(init) => init.run(self.args),
            SubCommands::Run(run) => run.run(self.args).await,
            SubCommands::Relay(relay) => relay.run(self.args).await,
        }
    }
}
