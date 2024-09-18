use calimero_server_primitives::admin::ListApplicationsResponse;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{get_response, multiaddr_to_url, RequestType};
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct ListCommand;

impl ListCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);
        if !ConfigFile::exists(&path) {
            bail!("Config file does not exist")
        }
        let Ok(config) = ConfigFile::load(&path) else {
            bail!("Failed to load config file");
        };
        let Some(multiaddr) = config.network.server.listen.first() else {
            bail!("No address.")
        };

        let url = multiaddr_to_url(multiaddr, "admin-api/dev/applications")?;
        let client = Client::new();
        let response =
            get_response(&client, url, None::<()>, &config.identity, RequestType::Get).await?;

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
