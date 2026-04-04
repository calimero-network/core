use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::UpdateGroupSettingsApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::upgrade_policy::{to_upgrade_policy, UpgradePolicyArg};
use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Update group settings")]
pub struct UpdateCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,

    #[clap(
        long,
        value_enum,
        default_value = "lazy-on-access",
        help = "New upgrade policy"
    )]
    pub upgrade_policy: UpgradePolicyArg,

    #[clap(long, help = "Deadline in seconds for coordinated upgrade policy")]
    pub deadline_secs: Option<u64>,
}

impl UpdateCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let upgrade_policy = to_upgrade_policy(self.upgrade_policy, self.deadline_secs);

        let request = UpdateGroupSettingsApiRequest {
            requester: self.requester,
            upgrade_policy,
        };

        let client = environment.client()?;
        let response = client
            .update_group_settings(&self.group_id, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
