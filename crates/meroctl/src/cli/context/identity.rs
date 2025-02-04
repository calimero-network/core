use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Managing your identity and alias")]
pub struct IdentityCommand {
    #[command(subcommand)]
    command: IdentitySubcommand,
}

#[derive(Debug, Parser)]
pub enum IdentitySubcommand {
    #[command(about = "Create public/private key pair used for context identity")]
    New,
}

impl Report for GenerateContextIdentityResponse {
    fn report(&self) {
        println!("public_key: {}", self.data.public_key);
        println!("private_key: {}", self.data.private_key);
    }
}

impl IdentityCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let url = multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/identity/context")?;

        let response: GenerateContextIdentityResponse = do_request(
            &Client::new(),
            url,
            None::<()>,
            &config.identity,
            RequestType::Post,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
