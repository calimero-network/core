use clap::{Parser, Subcommand};
use eyre::Result;

use crate::cli::Environment;

mod create;

/// Account and device management.
///
/// An account is a person or agent; a device is one installation of theirs.
/// Neither is a public key — both are content addresses, so keys rotate without
/// the identity changing. One grant to the account lets any number of its devices
/// write, each as a distinct CRDT replica, and any one of them can be revoked
/// without touching the others.
#[derive(Debug, Parser)]
pub struct AccountCommand {
    #[command(subcommand)]
    pub subcommand: AccountSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum AccountSubCommands {
    Create(create::CreateCommand),
}

impl AccountCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            AccountSubCommands::Create(cmd) => cmd.run(environment).await,
        }
    }
}
