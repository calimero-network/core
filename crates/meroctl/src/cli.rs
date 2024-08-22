use clap::{Parser, Subcommand};
use const_format::concatcp;

use crate::defaults;

mod app;
mod config;
mod context;
mod init;
mod run;

pub const EXAMPLES: &str = r"
  # Initialize a new node
  $ meroctl --home data/ --node-name node1 init

  # Configure an existing node
  $ meroctl --home data/ --node-name node1 config --server-host 143.34.182.202 --server-port 3000

  # Run a node as a peer
  $ meroctl --home data/ --node-name node1 run

  # Run a node as a coordinator
  $ meroctl --home data/ --node-name node1 run --node-type coordinator
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
    Init(init::InitCommand),
    Config(config::ConfigCommand),
    #[command(alias = "up")]
    Run(run::RunCommand),
    Context(context::ContextCommand),
    App(app::AppCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: camino::Utf8PathBuf,

    /// Name of node
    #[arg(short, long, value_name = "NAME")]
    pub node_name: camino::Utf8PathBuf,
}

impl RootCommand {
    pub async fn run(self) -> eyre::Result<()> {
        match self.action {
            SubCommands::Init(init) => init.run(self.args),
            SubCommands::Config(config) => config.run(&self.args),
            SubCommands::Run(run) => run.run(self.args).await,
            SubCommands::Context(context) => context.run(self.args).await,
            SubCommands::App(application) => application.run(self.args).await,
        }
    }
}
