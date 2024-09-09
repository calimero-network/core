use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    InstallApplicationRequest, InstallApplicationResponse, InstallDevApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, Result};
use reqwest::Client;
use semver::Version;
use tracing::info;
use url::Url;

use crate::cli::RootArgs;
use crate::common::RequestType::POST;
use crate::common::{get_response, multiaddr_to_url};
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct InstallCommand {
    /// Path to the application
    #[arg(long, short, conflicts_with = "url")]
    pub path: Option<Utf8PathBuf>,

    /// Url of the application
    #[clap(long, short, conflicts_with = "path", requires = "metadata")]
    pub url: Option<String>,

    #[clap(short, long, help = "Version of the application")]
    pub version: Option<Version>,

    #[clap(short, long, help = "Metadata for the application")]
    pub metadata: Option<Vec<u8>>,

    #[clap(long, help = "Hash of the application")]
    pub hash: Option<Hash>,
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

        let mut is_dev_installation = false;

        let install_request = if let Some(app_path) = self.path {
            let install_dev_request = InstallDevApplicationRequest::new(
                app_path.canonicalize_utf8()?,
                self.version,
                self.metadata.unwrap_or_default(),
            );
            is_dev_installation = true;
            serde_json::to_value(install_dev_request)?
        } else if let Some(app_url) = self.url {
            let install_request = InstallApplicationRequest::new(
                Url::parse(&app_url)?,
                self.version,
                self.hash,
                self.metadata.unwrap_or_default(),
            );
            serde_json::to_value(install_request)?
        } else {
            bail!("Either path or url must be provided");
        };

        let install_url = if is_dev_installation {
            multiaddr_to_url(multiaddr, "admin-api/dev/install-dev-application")?
        } else {
            multiaddr_to_url(multiaddr, "admin-api/dev/install-application")?
        };

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
