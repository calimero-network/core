use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::ListContextsResponse;
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "List all contexts")]
pub struct ListCommand {}

impl Report for ListContextsResponse {
    fn report(&self) {
        if self.data.contexts.is_empty() {
            println!("No contexts found");
            return;
        }

        println!("Contexts:");
        for context in &self.data.contexts {
            println!("  ID: {}", context.id);
            println!("  Protocol: {}", context.protocol);
            println!("  Application ID: {}", context.application_id);
            println!("  Created at: {}", context.created_at);
            println!();
        }
    }
}

impl ListCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let url = multiaddr_to_url(&multiaddr, "admin-api/dev/contexts")?;

        let response: ListContextsResponse =
            do_request(&client, url, None::<()>, &config.identity, RequestType::Get).await?;

        environment.output.write(&response);

        Ok(())
    }
}
