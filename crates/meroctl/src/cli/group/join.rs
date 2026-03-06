use calimero_server_primitives::admin::JoinGroupApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Join a group using an invitation payload")]
pub struct JoinCommand {
    #[clap(
        name = "INVITATION_PAYLOAD",
        help = "The invitation payload (obtained from 'meroctl group invite')"
    )]
    pub invitation_payload: String,
}

impl JoinCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = JoinGroupApiRequest {
            invitation_payload: self.invitation_payload,
        };

        let client = environment.client()?;
        let response = client.join_group(request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
