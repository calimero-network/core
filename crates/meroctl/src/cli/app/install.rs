use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    InstallApplicationRequest, InstallApplicationResponse, InstallDevApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, Result};
use reqwest::Client;
use tracing::info;
use url::Url;

use crate::cli::RootArgs;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Debug, Parser)]
pub struct InstallCommand {
    /// Path to the application
    #[arg(long, short, conflicts_with = "url")]
    pub path: Option<Utf8PathBuf>,

    /// Url of the application
    #[clap(long, short, conflicts_with = "path")]
    pub url: Option<String>,

    #[clap(short, long, help = "Metadata for the application")]
    pub metadata: Option<String>,

    #[clap(long, help = "Hash of the application")]
    pub hash: Option<Hash>,
}

impl InstallCommand {
    pub async fn run(self, args: RootArgs) -> Result<()> {
        let config = load_config(&args.home, &args.node_name)?;
        let mut is_dev_installation = false;
        let metadata = self.metadata.map(String::into_bytes).unwrap_or_default();

        let install_request = if let Some(app_path) = self.path {
            let install_dev_request =
                InstallDevApplicationRequest::new(app_path.canonicalize_utf8()?, metadata);
            is_dev_installation = true;
            serde_json::to_value(install_dev_request)?
        } else if let Some(app_url) = self.url {
            let install_request =
                InstallApplicationRequest::new(Url::parse(&app_url)?, self.hash, metadata);
            serde_json::to_value(install_request)?
        } else {
            bail!("Either path or url must be provided");
        };

        let install_url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            if is_dev_installation {
                "admin-api/dev/install-dev-application"
            } else {
                "admin-api/dev/install-application"
            },
        )?;

        let install_response = get_response(
            &Client::new(),
            install_url,
            Some(install_request),
            &config.identity,
            RequestType::Post,
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
