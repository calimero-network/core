use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(
    about = "Voluntarily leave a namespace (publishes MemberLeft at the root \
             and cascades through every descendant where you have a direct row). \
             Rejected with `MustTransferOwnership` if you own any group in the subtree."
)]
pub struct LeaveCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID to leave")]
    pub namespace_id: String,

    #[clap(
        long,
        help = "Also take the namespace off every device of your account, once an admin \
                confirmed the leave"
    )]
    pub purge: bool,
}

impl LeaveCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client
            .leave_namespace(&self.namespace_id, self.purge)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
