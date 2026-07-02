use clap::Parser;
use eyre::Result;

use crate::cli::Environment;
use crate::confirm::confirm;
use crate::output::InfoLine;

#[derive(Clone, Debug, Parser)]
#[command(about = "Voluntarily leave a group (publishes MemberLeft). \
             Rejected if you are the Owner (transfer ownership first), \
             the only admin, or only an inherited member of an Open subgroup.")]
pub struct LeaveCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID to leave")]
    pub group_id: String,

    #[clap(long, short = 'y', help = "Skip the confirmation prompt")]
    pub yes: bool,
}

impl LeaveCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        if !confirm(&format!("Leave group '{}'?", self.group_id), self.yes)? {
            environment.output.write(&InfoLine("Aborted."));
            return Ok(());
        }

        let client = environment.client()?;
        let response = client.leave_group(&self.group_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}
