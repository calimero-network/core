use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::DeleteGroupApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;
use crate::confirm::confirm;
use crate::output::InfoLine;

#[derive(Clone, Debug, Parser)]
#[command(about = "Delete a group")]
pub struct DeleteCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,

    #[clap(long, short = 'y', help = "Skip the confirmation prompt")]
    pub yes: bool,
}

impl DeleteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        if !confirm(
            &format!(
                "Delete group '{}'? This cascades to its subgroups and cannot be undone.",
                self.group_id
            ),
            self.yes,
        )? {
            environment.output.write(&InfoLine("Aborted."));
            return Ok(());
        }

        let request = DeleteGroupApiRequest {
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.delete_group(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
