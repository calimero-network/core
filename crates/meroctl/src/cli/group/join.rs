use calimero_context_config::types::SignedGroupOpenInvitation;
use calimero_server_primitives::admin::JoinGroupApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Join a group using an invitation")]
pub struct JoinCommand {
    #[clap(
        name = "INVITATION_JSON",
        help = "The invitation JSON (obtained from 'meroctl group invite')"
    )]
    pub invitation_json: String,
}

impl JoinCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let invitation: SignedGroupOpenInvitation = serde_json::from_str(&self.invitation_json)
            .map_err(|e| eyre::eyre!("invalid invitation JSON: {e}"))?;

        let request = JoinGroupApiRequest {
            invitation,
            group_alias: None,
        };

        let client = environment.client()?;
        let response = client.join_group(request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
