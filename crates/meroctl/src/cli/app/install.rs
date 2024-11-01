use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    InstallApplicationRequest, InstallApplicationResponse, InstallDevApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, Result};
use reqwest::Client;
use url::Url;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Install an application")]
pub struct InstallCommand {
    #[arg(long, short, conflicts_with = "url", help = "Path to the application")]
    pub path: Option<Utf8PathBuf>,

    #[clap(long, short, conflicts_with = "path", help = "Url of the application")]
    pub url: Option<String>,

    #[clap(short, long, help = "Metadata for the application")]
    pub metadata: Option<String>,

    #[clap(long, help = "Hash of the application")]
    pub hash: Option<Hash>,
}

impl Report for InstallApplicationResponse {
    fn report(&self) {
        println!("id: {}", self.data.application_id);
    }
}

impl InstallCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let mut is_dev_installation = false;
        let metadata = self.metadata.map(String::into_bytes).unwrap_or_default();

        let request = if let Some(app_path) = self.path {
            is_dev_installation = true;
            serde_json::to_value(InstallDevApplicationRequest::new(
                app_path.canonicalize_utf8()?,
                metadata,
            ))?
        } else if let Some(app_url) = self.url {
            serde_json::to_value(InstallApplicationRequest::new(
                Url::parse(&app_url)?,
                self.hash,
                metadata,
            ))?
        } else {
            bail!("Either path or url must be provided");
        };

        let url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            if is_dev_installation {
                "admin-api/dev/install-dev-application"
            } else {
                "admin-api/dev/install-application"
            },
        )?;

        let response: InstallApplicationResponse = do_request(
            &Client::new(),
            url,
            Some(request),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
