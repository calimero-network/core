use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::CreateNamespaceApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Create a new namespace")]
pub struct CreateCommand {
    #[clap(long, help = "The application ID to associate with the namespace")]
    pub application_id: ApplicationId,

    #[clap(long, help = "Optional human-readable name for the namespace")]
    pub name: Option<String>,

    #[clap(
        long,
        alias = "app-key",
        help = "Pin the namespace to a specific installed version (hex bytecode blob id); \
                defaults to the latest installed"
    )]
    pub bytecode_id: Option<String>,
}

impl CreateCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = CreateNamespaceApiRequest {
            application_id: self.application_id,
            name: self.name,
            bytecode_id: self.bytecode_id,
        };

        let client = environment.client()?;
        let response = client.create_namespace(request).await?;
        environment.output.write(&response);

        Ok(())
    }
}
