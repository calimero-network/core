use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::CreateNamespaceApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::upgrade_policy::{to_upgrade_policy, UpgradePolicyArg};
use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Create a new namespace")]
pub struct CreateCommand {
    #[clap(long, help = "The application ID to associate with the namespace")]
    pub application_id: ApplicationId,

    #[clap(
        long,
        value_enum,
        default_value = "automatic",
        help = "Upgrade policy for the namespace"
    )]
    pub upgrade_policy: UpgradePolicyArg,

    #[clap(long, help = "Optional human-readable name for the namespace")]
    pub name: Option<String>,

    #[clap(
        long,
        help = "Pin the namespace to a specific installed version (hex bytecode blob id); \
                defaults to the latest installed"
    )]
    pub app_key: Option<String>,
}

impl CreateCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let upgrade_policy = to_upgrade_policy(self.upgrade_policy);

        let request = CreateNamespaceApiRequest {
            application_id: self.application_id,
            upgrade_policy,
            name: self.name,
            app_key: self.app_key,
        };

        let client = environment.client()?;
        let response = client.create_namespace(request).await?;
        environment.output.write(&response);

        Ok(())
    }
}
