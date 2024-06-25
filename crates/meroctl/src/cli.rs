use clap::{Parser, Subcommand};

use crate::defaults;

mod config;
mod init;
mod run;

#[derive(Debug, Parser)]
#[command(author, about, version)]
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
    Run(run::RunCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_chat_dir())]
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
            SubCommands::Config(config) => config.run(self.args),
            SubCommands::Run(run) => run.run(self.args).await,
        }
    }
}
