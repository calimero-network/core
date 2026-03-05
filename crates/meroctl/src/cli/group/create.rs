use std::time::Duration;

use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::UpgradePolicy;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::CreateGroupApiRequest;
use clap::{Parser, ValueEnum};
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, ValueEnum)]
pub enum UpgradePolicyArg {
    Automatic,
    LazyOnAccess,
    Coordinated,
}

#[derive(Debug, Parser)]
#[command(about = "Create a new group")]
pub struct CreateCommand {
    #[clap(
        long,
        help = "Hex-encoded 32-byte app key for the group (auto-generated if not provided)"
    )]
    pub app_key: Option<String>,

    #[clap(long, help = "The application ID to associate with the group")]
    pub application_id: ApplicationId,

    #[clap(
        long,
        value_enum,
        default_value = "lazy-on-access",
        help = "Upgrade policy for the group"
    )]
    pub upgrade_policy: UpgradePolicyArg,

    #[clap(long, help = "Deadline in seconds for coordinated upgrade policy")]
    pub deadline_secs: Option<u64>,

    #[clap(
        long,
        help = "Admin identity public key for the group (defaults to node NEAR identity)"
    )]
    pub admin_identity: Option<PublicKey>,

    #[clap(
        long,
        help = "Optional group ID (hex-encoded 32 bytes); generated if not provided"
    )]
    pub group_id: Option<String>,

    #[clap(
        long,
        help = "Requester private key (hex). Deprecated: register a signing key instead"
    )]
    pub requester_secret: Option<String>,
}

impl CreateCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let upgrade_policy = match self.upgrade_policy {
            UpgradePolicyArg::Automatic => UpgradePolicy::Automatic,
            UpgradePolicyArg::LazyOnAccess => UpgradePolicy::LazyOnAccess,
            UpgradePolicyArg::Coordinated => UpgradePolicy::Coordinated {
                deadline: self.deadline_secs.map(Duration::from_secs),
            },
        };

        let request = CreateGroupApiRequest {
            group_id: self.group_id,
            app_key: self.app_key,
            application_id: self.application_id,
            upgrade_policy,
            admin_identity: self.admin_identity,
            requester_secret: self.requester_secret,
        };

        let client = environment.client()?;
        let response = client.create_group(request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
