use clap::Parser;
use eyre::{bail, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::CommandContext;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Debug, Parser)]
#[command(about = "Delete an context")]
pub struct DeleteCommand {
    #[clap(name = "CONTEXT_ID", help = "The context ID to delete")]
    pub context_id: String,
}

impl DeleteCommand {
    pub async fn run(self, context: CommandContext) -> EyreResult<()> {
        let config = load_config(&context.args.home, &context.args.node_name)?;

        self.delete_context(fetch_multiaddr(&config)?, &Client::new(), &config.identity)
            .await
    }

    #[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
    async fn delete_context(
        &self,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}", self.context_id),
        )?;
        let response = get_response(client, url, None::<()>, keypair, RequestType::Delete).await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        let text = response.text().await?;
        println!("Context deleted successfully: {text}");
        Ok(())
    }
}
