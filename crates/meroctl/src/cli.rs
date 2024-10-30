use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::defaults;
use crate::output::{Format, Output};

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

    #[arg(long, value_name = "FORMAT")]
    pub output_format: Format,
}

pub struct CommandContext {
    pub args: RootArgs,
    pub output: Output,
}

impl CommandContext {
    pub fn new(args: RootArgs, output: Output) -> Self {
        CommandContext { args, output }
    }
}

impl RootCommand {
    pub async fn run(self) -> EyreResult<()> {
        let output = Output::new(self.args.output_format);
        let cmd_context = CommandContext::new(self.args, output);

        match self.action {
            SubCommands::Context(context) => context.run(cmd_context).await,
            SubCommands::App(application) => application.run(cmd_context).await,
            SubCommands::JsonRpc(jsonrpc) => jsonrpc.run(cmd_context).await,
        }
    }
}
