use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

/// The devices each member of a group speaks with.
///
/// The account is the authorization subject and the device is what signs, so a
/// membership list answers "who may" and this answers "which keys will actually
/// appear on their ops". Reconciling the two is why it exists: a signature
/// names a device, and nothing else maps that back to a member.
///
/// Paginated because the device count is unbounded where the member count is
/// not -- one person may hold many. A caller that needs the whole set walks
/// `--offset` rather than assuming one response is complete.
#[derive(Clone, Debug, Parser)]
#[command(about = "List the devices of each member of a group")]
pub struct MemberDevicesCommand {
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,

    #[clap(long, help = "Skip this many members before listing")]
    pub offset: Option<usize>,

    #[clap(long, help = "Return at most this many members; the node caps it")]
    pub limit: Option<usize>,
}

impl MemberDevicesCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client
            .list_member_devices(&self.group_id, self.offset, self.limit)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
