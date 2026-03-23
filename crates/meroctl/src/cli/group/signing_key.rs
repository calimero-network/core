use calimero_server_primitives::admin::RegisterGroupSigningKeyApiRequest;
use clap::{Parser, Subcommand};
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Manage signing keys for a group")]
pub struct SigningKeyCommand {
    #[command(subcommand)]
    pub subcommand: SigningKeySubCommands,
}

#[derive(Debug, Subcommand)]
pub enum SigningKeySubCommands {
    #[command(about = "Register a signing key for a group admin")]
    Register(RegisterSigningKeyCommand),
}

impl SigningKeyCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            SigningKeySubCommands::Register(cmd) => cmd.run(environment).await,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Register a signing key for a group admin on this node")]
pub struct RegisterSigningKeyCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        name = "SIGNING_KEY",
        help = "The hex-encoded private signing key to register"
    )]
    pub signing_key: String,
}

impl RegisterSigningKeyCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = RegisterGroupSigningKeyApiRequest {
            signing_key: self.signing_key,
        };

        let client = environment.client()?;
        let response = client
            .register_group_signing_key(&self.group_id, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
