mod alias;
mod call;
mod create;
mod list;
mod remove;

use alias::ContextAliasCommand;
use call::CallCommand;
use create::CreateCommand;
use list::ListCommand;
use remove::RemoveCommand;

use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::Result as EyreResult;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Manage contexts")]
pub struct ContextCommand {
    #[command(subcommand)]
    command: ContextSubcommand,
}

#[derive(Debug, Parser)]
pub enum ContextSubcommand {
    #[command(about = "Create a new context")]
    Create(CreateCommand),
    
    #[command(about = "Call a context function")]
    Call(CallCommand),
    
    #[command(about = "Manage context aliases")]
    Alias(ContextAliasCommand),
    
    #[command(about = "List all contexts")]
    List(ListCommand),
    
    #[command(about = "Remove a context")]
    Remove(RemoveCommand),
}

impl ContextCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        self.command.run(environment).await
    }
}

impl ContextSubcommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        match self {
            Self::Create(cmd) => cmd.run(environment).await,
            Self::Call(cmd) => cmd.run(environment).await,
            Self::Alias(cmd) => cmd.run(environment).await,
            Self::List(cmd) => cmd.run(environment).await,
            Self::Remove(cmd) => cmd.run(environment).await,
        }
    }
} 