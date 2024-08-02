use camino::Utf8PathBuf;
use clap::Parser;
use libp2p::Multiaddr;
use reqwest::Client;
use tracing::info;

use crate::cli::RootArgs;
use crate::common::multiaddr_to_url;
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct CreateCommand {
    /// The application ID to attach to the context
    #[clap(long, short = 'a', exclusive = true)]
    application_id: Option<calimero_primitives::application::ApplicationId>,

    /// Path to the application file to watch and install locally
    #[clap(long, short = 'w', exclusive = true)]
    watch: Option<Utf8PathBuf>,
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

        let client = Client::new();

        match self {
            CreateCommand {
                application_id: Some(app_id),
                watch: None,
            } => create_context(multiaddr, app_id, &client).await,
            CreateCommand {
                application_id: None,
                watch: Some(watch_path),
            } => install_and_create_context(multiaddr, watch_path, &client).await,
            _ => eyre::bail!("Invalid command configuration"),
        }
    }
}

async fn create_context(
    base_multiaddr: &Multiaddr,
    application_id: calimero_primitives::application::ApplicationId,
    client: &Client,
) -> eyre::Result<()> {
    if !app_installed(&base_multiaddr, &application_id, client).await? {
        eyre::bail!("Application is not installed on node.")
    }

    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/contexts")?;
    let request = calimero_server_primitives::admin::CreateContextRequest { application_id };

    let response = client.post(url).json(&request).send().await?;

    if response.status().is_success() {
        let context_response: calimero_server_primitives::admin::CreateContextResponse =
            response.json().await?;
        let context = context_response.data.context;

        println!("Context created successfully:");
        println!("Context ID: {}", context.id);
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

async fn app_installed(
    base_multiaddr: &Multiaddr,
    application_id: &calimero_primitives::application::ApplicationId,
    client: &Client,
) -> eyre::Result<bool> {
    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/applications")?;
    let response = client.get(url).send().await?;

    if !response.status().is_success() {
        eyre::bail!("Request failed with status: {}", response.status())
    }

    let api_response: calimero_server_primitives::admin::ListApplicationsResponse =
        response.json().await?;
    let app_list = api_response.data.apps;
    let is_installed = app_list.iter().any(|app| &app.id == application_id);

    Ok(is_installed)
}

async fn install_and_create_context(
    base_multiaddr: &Multiaddr,
    path: Utf8PathBuf,
    client: &Client,
) -> eyre::Result<()> {
    let install_url = multiaddr_to_url(base_multiaddr, "admin-api/dev/install-application")?;

    let install_request = calimero_server_primitives::admin::InstallDevApplicationRequest {
        version: None,
        path,
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

    let response = install_response
        .json::<calimero_server_primitives::admin::InstallApplicationResponse>()
        .await?;

    info!("Application installed successfully.");

    create_context(base_multiaddr, response.data.application_id, client).await?;

    Ok(())
}
