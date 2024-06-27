use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::Context;
use calimero_server_primitives::admin::ApplicationListResult;
use camino::Utf8PathBuf;
use clap::{ArgGroup, Parser};
use reqwest::{Client, Url};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;

use crate::cli::context::common::get_ip;
use crate::cli::RootArgs;
use crate::config_file::ConfigFile;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextRequest {
    application_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateContextResponse {
    data: Context,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstallApplicationRequest {
    pub application: ApplicationId,
    pub version: Version,
    pub path: Option<Utf8PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListApplicationsResponse {
    data: ApplicationListResult,
}

#[derive(Debug, Parser)]
#[clap(group(
    ArgGroup::new("dev_args")
        .multiple(true)
        .requires_all(&["dev", "path", "version"])
))]
pub struct CreateCommand {
    /// The application ID to attach to the context
    #[clap(
        long,
        short = 'a',
        default_value = "",
        exclusive = true,
        value_name = "APP_ID"
    )]
    application_id: String,

    /// Enable dev mode
    #[clap(long, group = "dev_args")]
    dev: bool,

    /// Path to use in dev mode
    #[clap(
        short,
        long,
        group = "dev_args",
        default_value = "",
        value_name = "PATH"
    )]
    path: Utf8PathBuf,

    /// Version of the application
    #[clap(
        short,
        long,
        group = "dev_args",
        default_value = "0.0.0",
        value_name = "VERSION"
    )]
    version: Version,
}

impl CreateCommand {
    pub async fn run(self, root_args: RootArgs) -> eyre::Result<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            eyre::bail!("Config file does not exist")
        };

        let Ok(config) = ConfigFile::load(&path) else {
            eyre::bail!("Failed to load config file")
        };

        let Some(multiaddr) = config.network.server.listen.first() else {
            eyre::bail!("No address.")
        };

        let base_url = get_ip(multiaddr, None)?;

        if self.dev {
            return link_local_app(base_url, self.path, self.version).await;
        }
        create_context(base_url, self.application_id).await
    }
}

async fn create_context(base_url: Url, application_id: String) -> eyre::Result<()> {
    app_installed(&base_url, &application_id).await?;

    let url = format!("{}admin-api/contexts-dev", base_url);
    let client = Client::new();
    let request = CreateContextRequest { application_id };

    let response = client.post(&url).json(&request).send().await?;

    if response.status().is_success() {
        let context_response: CreateContextResponse = response.json().await?;
        let context = context_response.data;

        println!("Context created successfully:");
        println!("ID: {}", context.id);
        println!("Application ID: {}", context.application_id);
    } else {
        let status = response.status();
        let error_text = response.text().await?;
        eyre::bail!(
            "Request failed with status: {}. Error: {}",
            status,
            error_text
        );
    }
    Ok(())
}

async fn app_installed(base_url: &Url, application_id: &String) -> eyre::Result<()> {
    let url = format!("{}admin-api/applications-dev", base_url);
    let client = Client::new();
    let response = client.get(&url).send().await?;
    if response.status().is_success() {
        let api_response: ListApplicationsResponse = response.json().await?;
        let app_list = api_response.data.apps;
        let x = app_list
            .iter()
            .any(|app| app.id.as_ref() == *application_id);
        if x {
            return Ok(());
        } else {
            eyre::bail!("The app with the id {} is not installed\nTo create a context, install an app first", application_id)
        }
    } else {
        eyre::bail!("Request failed with status: {}", response.status())
    }
}

async fn link_local_app(base_url: Url, path: Utf8PathBuf, version: Version) -> eyre::Result<()> {
    let install_url = format!("{}admin-api/install-dev-application", base_url);

    let id = format!("{}:{}", version, path);
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    let application_id = hex::encode(hasher.finalize());

    let client = Client::new();
    let install_request = InstallApplicationRequest {
        application: ApplicationId(application_id.clone()),
        version: version,
        path: Some(path),
    };

    let install_response = client
        .post(&install_url)
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

    info!("Application installed successfully.");

    create_context(base_url, application_id).await?;

    Ok(())
}
