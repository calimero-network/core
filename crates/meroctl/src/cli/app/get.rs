use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{get_response, load_config, load_multiaddr, multiaddr_to_url, RequestType};

#[derive(Parser, Debug)]
pub struct GetCommand {
    #[arg(long, short)]
    pub method: GetValues,

    #[arg(long, short)]
    pub app_id: String,
}
#[derive(ValueEnum, Debug, Clone)]
pub enum GetValues {
    Details,
}

impl GetCommand {
    #[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let path = args.home.join(&args.node_name);
        let config = load_config(&path)?;
        let multiaddr = load_multiaddr(&config)?;
        let client = Client::new();

        let url = multiaddr_to_url(
            &multiaddr,
            &format!("admin-api/dev/applications/{}", self.app_id),
        )?;

        let response =
            get_response(&client, url, None::<()>, &config.identity, RequestType::Get).await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        println!("{}", response.text().await?);

        Ok(())
    }
}
