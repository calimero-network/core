use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Generate public/private key pair used for context identity")]
pub struct GenerateCommand;

impl Report for GenerateContextIdentityResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Generated Identity").fg(Color::Blue)]);
        let _ = table.add_row(vec![format!("Public Key: {}", self.data.public_key)]);
        println!("{table}");
    }
}

impl GenerateCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name).await?;
        let multiaddr = fetch_multiaddr(&config)?;
        let url = multiaddr_to_url(multiaddr, "admin-api/dev/identity/context")?;

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
