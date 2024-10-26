use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Parser, Debug)]
#[command(about = "Fetch application details")]
pub struct GetCommand {
    #[arg(value_name = "APP_ID", help = "application_id of the application")]
    pub app_id: String,
}

impl GetCommand {
    #[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let config = load_config(&args.home, &args.node_name)?;

        let url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            &format!("admin-api/dev/applications/{}", self.app_id),
        )?;

        let response = get_response(
            &Client::new(),
            url,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        println!("{}", response.text().await?);

        Ok(())
    }
}
