use calimero_server_primitives::admin::{InstallApplicationResponse, InstallDevApplicationRequest};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, Result};
use reqwest::Client;
use semver::Version;
use tracing::info;

use crate::cli::RootArgs;
use crate::common::RequestType::POST;
use crate::common::{get_response, multiaddr_to_url};
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct InstallCommand {
    /// Path to the application
    #[arg(long, short)]
    pub path: Utf8PathBuf,

    /// Version of the application
    #[clap(short, long, help = "Version of the application")]
    pub version: Option<Version>,
    pub metadata: Option<Vec<u8>>,
}

impl InstallCommand {
    pub async fn run(self, args: RootArgs) -> Result<()> {
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

        let install_url = multiaddr_to_url(multiaddr, "admin-api/dev/install-application")?;

        let install_request = InstallDevApplicationRequest::new(
            self.path.canonicalize_utf8()?,
            self.version,
            self.metadata.unwrap_or_default(),
        );

        let install_response = get_response(
            &client,
            install_url,
            Some(install_request),
            &config.identity,
            POST,
        )
        .await?;

        if !install_response.status().is_success() {
            let status = install_response.status();
            let error_text = install_response.text().await?;
            bail!(
                "Application installation failed with status: {}. Error: {}",
                status,
                error_text
            )
        }

        let body = install_response
            .json::<InstallApplicationResponse>()
            .await?;

        info!(
            "Application installed successfully. Application ID: {}",
            body.data.application_id
        );

        Ok(())
    }
}
