use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::defaults;

mod app;
mod context;
mod jsonrpc;

use app::AppCommand;
use context::ContextCommand;
use jsonrpc::CallCommand;

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl -- --node-name node1 app ls

  # List all contexts
  $ meroctl -- --home data --node-name node1 context ls
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
    Context(ContextCommand),
    App(AppCommand),
    JsonRpc(CallCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: Utf8PathBuf,

    /// Name of node
    #[arg(short, long, value_name = "NAME")]
    pub node_name: String,
}

impl RootCommand {
    pub async fn run(self) -> EyreResult<()> {
        match self.action {
            SubCommands::Context(context) => context.run(self.args).await,
            SubCommands::App(application) => application.run(self.args).await,
            SubCommands::JsonRpc(jsonrpc) => jsonrpc.run(self.args).await,
        }
    }
}
