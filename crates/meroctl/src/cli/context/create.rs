use camino::Utf8PathBuf;
use chrono::Utc;
use clap::{Args, Parser};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;
use semver::Version;
use sha2::{Digest, Sha256};
use tracing::info;

use crate::cli::context::common::multiaddr_to_url;
use crate::cli::RootArgs;
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct CreateCommand {
    /// The application ID to attach to the context
    #[clap(long, short = 'a', exclusive = true)]
    application_id: Option<String>,

    #[clap(flatten)]
    dev_args: Option<DevArgs>,
}

#[derive(Debug, Args)]
struct DevArgs {
    /// Enable dev mode
    #[clap(long)]
    dev: bool,

    /// Path to use in dev mode
    #[clap(
        short,
        long,
        help = "Path to use in dev mode (requires --dev and --version)"
    )]
    path: Utf8PathBuf,

    /// Version of the application
    #[clap(
        short,
        long,
        help = "Version of the application (requires --dev and --path)",
        default_value = "0.0.0"
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

        let client = Client::new();

        match self {
            CreateCommand {
                application_id: Some(app_id),
                dev_args: None,
            } => create_context(multiaddr, app_id, &client, config.identity).await,
            CreateCommand {
                application_id: None,
                dev_args: Some(dev_args),
            } => {
                link_local_app(
                    multiaddr,
                    dev_args.path,
                    dev_args.version,
                    &client,
                    config.identity,
                )
                .await
            }
            _ => eyre::bail!("Invalid command configuration"),
        }
    }
}

async fn create_context(
    base_multiaddr: &Multiaddr,
    application_id: String,
    client: &Client,
    keypair: Keypair,
) -> eyre::Result<()> {
    if !app_installed(&base_multiaddr, &application_id, client, &keypair).await? {
        eyre::bail!("Application is not installed on node.")
    }

    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/contexts")?;
    let request = calimero_server_primitives::admin::CreateContextRequest {
        application_id: calimero_primitives::application::ApplicationId(application_id),
    };

    let timestamp = Utc::now().timestamp().to_string();
    let signature = keypair.sign(timestamp.as_bytes())?;

    let response = client
        .post(url)
        .header("X-Signature", hex::encode(signature))
        .header("X-Timestamp", timestamp)
        .json(&request)
        .send()
        .await?;

    if response.status().is_success() {
        let context_response: calimero_server_primitives::admin::CreateContextResponse =
            response.json().await?;
        let context = context_response.data;

        println!("Context created successfully:");
        println!("ID: {}", context.context.id);
        println!("Application ID: {}", context.context.application_id);
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
    application_id: &String,
    client: &Client,
    keypair: &Keypair,
) -> eyre::Result<bool> {
    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/applications")?;

    let timestamp = Utc::now().timestamp().to_string();
    let signature = keypair.sign(timestamp.as_bytes())?;

    let response = client
        .get(url)
        .header("X-Signature", hex::encode(signature))
        .header("X-Timestamp", timestamp)
        .send()
        .await?;

    if !response.status().is_success() {
        eyre::bail!("Request failed with status: {}", response.status())
    }

    let api_response: calimero_server_primitives::admin::ListApplicationsResponse =
        response.json().await?;
    let app_list = api_response.data.apps;
    let is_installed = app_list.iter().any(|app| app.id.as_ref() == application_id);

    Ok(is_installed)
}

async fn link_local_app(
    base_multiaddr: &Multiaddr,
    path: Utf8PathBuf,
    version: Version,
    client: &Client,
    keypair: Keypair,
) -> eyre::Result<()> {
    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/install-application")?;

    let id = format!("{}:{}", version, path);
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    let application_id = hex::encode(hasher.finalize());

    let request = calimero_server_primitives::admin::InstallDevApplicationRequest {
        application_id: calimero_primitives::application::ApplicationId(application_id.clone()),
        version: version,
        path,
    };

    let timestamp = Utc::now().timestamp().to_string();
    let signature = keypair.sign(timestamp.as_bytes())?;

    let response = client
        .post(url)
        .header("X-Signature", hex::encode(signature))
        .header("X-Timestamp", timestamp)
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await?;
        eyre::bail!(
            "Application installation failed with status: {}. Error: {}",
            status,
            error_text
        )
    }

    info!("Application installed successfully.");

    create_context(base_multiaddr, application_id, &client, keypair).await?;

    Ok(())
}
