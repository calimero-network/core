use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{get_response, multiaddr_to_url};
use crate::config_file::ConfigFile;

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
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let path = args.home.join(&args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Config file does not exist")
        };

        let Ok(config) = ConfigFile::load(&path) else {
            bail!("Failed to load config file")
        };

        let Some(multiaddr) = config.network.server.listen.first() else {
            bail!("No address.")
        };

        let client = Client::new();

        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/applications/{}", self.app_id),
        )?;

        let response = get_response(&client, url, None::<()>, &config.identity).await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        println!("{}", response.text().await?);

        Ok(())
    }
}
