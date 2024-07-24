use camino::Utf8PathBuf;
use clap::Parser;
use eyre::Result;
use semver::Version;
use sha2::{Digest, Sha256};
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
    #[clap(
        short,
        long,
        help = "Version of the application (requires --dev and --path)",
        default_value = "0.0.0"
    )]
    pub version: Version,
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

        let install_url = multiaddr_to_url(&multiaddr, "admin-api/dev/install-application")?;

        let id = format!("{}:{}", self.version, self.path);
        let mut hasher = Sha256::new();
        hasher.update(id.as_bytes());
        let application_id = hex::encode(hasher.finalize());

        let install_request = calimero_server_primitives::admin::InstallDevApplicationRequest {
            application_id: calimero_primitives::application::ApplicationId(application_id.clone()),
            version: self.version,
            path: self.path,
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

        info!(
            "Application installed successfully. Application ID: {}",
            application_id
        );

        Ok(())
    }
}
