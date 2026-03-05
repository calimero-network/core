use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{RetryGroupUpgradeApiRequest, UpgradeGroupApiRequest};
use clap::{Parser, Subcommand};
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Manage group upgrades")]
pub struct UpgradeCommand {
    #[command(subcommand)]
    pub subcommand: UpgradeSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum UpgradeSubCommands {
    #[command(about = "Trigger an upgrade for a group")]
    Trigger(TriggerUpgradeCommand),
    #[command(about = "Get the current upgrade status of a group")]
    Status(UpgradeStatusCommand),
    #[command(about = "Retry a failed group upgrade")]
    Retry(RetryUpgradeCommand),
}

impl UpgradeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            UpgradeSubCommands::Trigger(cmd) => cmd.run(environment).await,
            UpgradeSubCommands::Status(cmd) => cmd.run(environment).await,
            UpgradeSubCommands::Retry(cmd) => cmd.run(environment).await,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Trigger an upgrade for a group to a new application version")]
pub struct TriggerUpgradeCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(long, help = "Target application ID to upgrade to")]
    pub target_application_id: ApplicationId,

    #[clap(
        long,
        help = "Public key of the requester (group admin) (defaults to node NEAR identity)"
    )]
    pub requester: Option<PublicKey>,

    #[clap(long, help = "Optional migration method name to call on each context")]
    pub migrate_method: Option<String>,
}

impl TriggerUpgradeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = UpgradeGroupApiRequest {
            target_application_id: self.target_application_id,
            requester: self.requester,
            migrate_method: self.migrate_method,
        };

        let client = environment.client()?;
        let response = client.upgrade_group(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Get the current upgrade status of a group")]
pub struct UpgradeStatusCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,
}

impl UpgradeStatusCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.get_group_upgrade_status(&self.group_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Retry a failed group upgrade")]
pub struct RetryUpgradeCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        long,
        help = "Public key of the requester (group admin) (defaults to node NEAR identity)"
    )]
    pub requester: Option<PublicKey>,
}

impl RetryUpgradeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = RetryGroupUpgradeApiRequest {
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.retry_group_upgrade(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
