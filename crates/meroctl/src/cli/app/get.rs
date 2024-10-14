use calimero_server_primitives::admin::GetApplicationResponse;
use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Parser, Debug)]
pub struct GetCommand {
    #[arg(long, short)]
    pub method: GetValues,

    #[arg(long, short)]
    pub app_id: String,

    #[arg(long, short)]
    pub test: bool,
}
#[derive(ValueEnum, Debug, Clone)]
pub enum GetValues {
    Details,
}

impl GetCommand {
    #[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let config = load_config(&args.node_name)?;

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

        let response: GetApplicationResponse = response.json().await?;
        println!("{:#?}", response);

        Ok(())
    }
}
