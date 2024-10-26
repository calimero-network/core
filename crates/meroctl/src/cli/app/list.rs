use calimero_server_primitives::admin::ListApplicationsResponse;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Debug, Parser)]
#[command(about = "List installed applications")]
pub struct ListCommand;

impl ListCommand {
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let config = load_config(&args.home, &args.node_name)?;

        let response = get_response(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/applications")?,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        let api_response: ListApplicationsResponse = response.json().await?;
        let app_list = api_response.data.apps;

        #[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
        for app in app_list {
            println!("{}", app.id);
        }

        Ok(())
    }
}
