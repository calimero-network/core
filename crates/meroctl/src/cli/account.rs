use clap::{Parser, Subcommand};
use eyre::Result;

use crate::cli::Environment;

mod pair_complete;
mod pair_init;
mod revoke;
mod show;

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
    PairInit(pair_init::PairInitCommand),
    PairComplete(pair_complete::PairCompleteCommand),
    Revoke(revoke::RevokeCommand),
    Show(show::ShowCommand),
}

impl AccountCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            AccountSubCommands::Show(cmd) => cmd.run(environment).await,
            AccountSubCommands::PairInit(cmd) => cmd.run(environment).await,
            AccountSubCommands::PairComplete(cmd) => cmd.run(environment).await,
            AccountSubCommands::Revoke(cmd) => cmd.run(environment).await,
        }
    }
}
