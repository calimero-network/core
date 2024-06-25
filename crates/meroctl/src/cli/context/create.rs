use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::Context;
use calimero_server_primitives::admin::ApplicationListResult;
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::eyre;
use reqwest::{Client, Url};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;

use super::get_ip;
use crate::cli::RootArgs;
use crate::config::ConfigFile;

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
pub struct CreateCommand {
    /// The application ID to attach to the context
    #[clap(long, short = 'a')]
    application_id: Option<String>,

    /// Enable dev mode
    #[clap(long)]
    dev: bool,

    /// Path to use in dev mode (required in dev mode)
    #[clap(short, long, requires = "dev")]
    path: Option<Utf8PathBuf>,

    /// Version of the application (required in dev mode)
    #[clap(short, long, requires = "dev")]
    version: Option<Version>,
}
impl CreateCommand {
    pub async fn run(self, root_args: RootArgs) -> eyre::Result<()> {
        let path = root_args.home.join(&root_args.node_name);
        if ConfigFile::exists(&path) {
            if let Ok(config) = ConfigFile::load(&path) {
                let multiaddr = config
                    .network
                    .server
                    .listen
                    .first()
                    .ok_or_else(|| eyre!("No address."))?;
                let base_url = get_ip(multiaddr)?;

                match self.dev {
                    true => Ok(link_local_app(base_url, self.path, self.version).await?),
                    false => Ok(create_context(base_url, self.application_id).await?),
                }
            } else {
                Err(eyre!("Failed to load config file"))
            }
        } else {
            Err(eyre!("Config file does not exist"))
        }
    }
}

async fn create_context(base_url: Url, application_id: Option<String>) -> eyre::Result<()> {
    let application_id =
        application_id.ok_or_else(|| eyre!("Application ID is required for starting context"))?;

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
        return Err(eyre!(
            "Request failed with status: {}. Error: {}",
            status,
            error_text
        ));
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
            .map(|app| app.id.0.clone())
            .any(|app| app == *application_id);
        if x {
            return Ok(());
        } else {
            return Err(eyre!("The app with the id {} is not installed\nTo create a context, install an app first", application_id));
        }
    } else {
        return Err(eyre!("Request failed with status: {}", response.status()));
    }
}

async fn link_local_app(
    base_url: Url,
    path: Option<Utf8PathBuf>,
    version: Option<Version>,
) -> eyre::Result<()> {
    let path = path.ok_or_else(|| eyre!("Path is required in dev mode"))?;
    let version = version.ok_or_else(|| eyre!("Version is required in dev mode"))?;

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
        return Err(eyre!(
            "Application installation failed with status: {}. Error: {}",
            status,
            error_text
        ));
    }

    info!("Application installed successfully.");

    create_context(base_url, Some(application_id)).await?;

    Ok(())
}
