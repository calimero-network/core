use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::RemoveContextResponse;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType, lookup_alias};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Remove a context")]
pub struct RemoveCommand {
    #[clap(help = "The context ID or alias to remove")]
    context: String,
}

impl Report for RemoveContextResponse {
    fn report(&self) {
        println!("Context removed successfully");
    }
}

impl RemoveCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        // Resolve context ID if it's an alias
        let context_id = if let Ok(context_id) = self.context.parse::<ContextId>() {
            context_id
        } else if let Ok(alias) = self.context.parse::<Alias<ContextId>>() {
            match lookup_alias(multiaddr.clone(), &config.identity, alias, None).await {
                Ok(response) => {
                    if let Some(context_id) = response.data.value {
                        context_id
                    } else {
                        bail!("Context alias '{}' not found", self.context);
                    }
                }
                Err(e) => bail!("Error looking up context alias '{}': {}", self.context, e),
            }
        } else {
            bail!("Invalid context ID or alias format: {}", self.context);
        };

        let url = multiaddr_to_url(&multiaddr, &format!("admin-api/dev/contexts/{}", context_id))?;

        let response: RemoveContextResponse =
            do_request(&client, url, None::<()>, &config.identity, RequestType::Delete).await?;

        environment.output.write(&response);

        Ok(())
    }
} 