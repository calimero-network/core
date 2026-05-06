use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Voluntarily leave a group (publishes MemberLeft). \
             Rejected if you are the Owner (transfer ownership first), \
             the only admin, or only an inherited member of an Open subgroup.")]
pub struct LeaveCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID to leave")]
    pub group_id: String,
}

impl LeaveCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.leave_group(&self.group_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}
