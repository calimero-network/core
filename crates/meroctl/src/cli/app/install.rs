use camino::Utf8PathBuf;
use clap::Parser;
use eyre::Result;
use semver::Version;
use tracing::info;

use crate::cli::RootArgs;
use crate::common::multiaddr_to_url;
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct InstallCommand {
    /// Path to the application
    #[arg(long, short)]
    pub path: Utf8PathBuf,

    /// Version of the application
    #[clap(short, long, help = "Version of the application")]
    pub version: Option<Version>,
}

impl InstallCommand {
    pub async fn run(self, args: RootArgs) -> Result<()> {
        let path = args.home.join(&args.node_name);

        if !ConfigFile::exists(&path) {
            eyre::bail!("Config file does not exist")
        };

        let Ok(config) = ConfigFile::load(&path) else {
            eyre::bail!("Failed to load config file")
        };
        let Some(multiaddr) = config.network.server.listen.first() else {
            eyre::bail!("No address.")
        };

        let client = reqwest::Client::new();

        let install_url = multiaddr_to_url(multiaddr, "admin-api/dev/install-application")?;

        let install_request = calimero_server_primitives::admin::InstallDevApplicationRequest {
            path: self.path.canonicalize_utf8()?,
            version: self.version,
        };

        let install_response = client
            .post(install_url)
            .json(&install_request)
            .send()
            .await?;

        if !install_response.status().is_success() {
            let status = install_response.status();
            let error_text = install_response.text().await?;
            eyre::bail!(
                "Application installation failed with status: {}. Error: {}",
                status,
                error_text
            )
        }

        let body = install_response
            .json::<calimero_server_primitives::admin::InstallApplicationResponse>()
            .await?;

        info!(
            "Application installed successfully. Application ID: {}",
            body.data.application_id
        );

        Ok(())
    }
}
