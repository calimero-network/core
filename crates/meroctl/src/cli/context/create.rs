<<<<<<< HEAD
use camino::Utf8PathBuf;
use chrono::Utc;
use clap::{Args, Parser};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;
use semver::Version;
use sha2::{Digest, Sha256};
use tracing::info;
=======
#![allow(clippy::print_stdout, clippy::print_stderr)]

use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    CreateContextRequest, CreateContextResponse, GetApplicationResponse,
    InstallApplicationResponse, InstallDevApplicationRequest, UpdateContextApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use libp2p::Multiaddr;
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use reqwest::Client;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
>>>>>>> origin/master

use crate::cli::RootArgs;
use crate::common::multiaddr_to_url;
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct CreateCommand {
    /// The application ID to attach to the context
    #[clap(long, short = 'a', conflicts_with = "watch")]
    application_id: Option<ApplicationId>,

    /// Path to the application file to watch and install locally
    #[clap(long, short = 'w')]
    watch: Option<Utf8PathBuf>,
    #[clap(requires = "watch")]
    metadata: Option<Vec<u8>>,

    #[clap(long, short = 'c', requires = "watch")]
    context_id: Option<ContextId>,

    #[clap(long, short = 'p')]
    params: Option<String>,
}

impl CreateCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

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

        match self {
            Self {
                application_id: Some(app_id),
<<<<<<< HEAD
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
=======
                watch: None,
                context_id: None,
                metadata: None,
                params,
            } => {
                let _ = create_context(&client, multiaddr, app_id, None, params).await?;
            }
            Self {
                application_id: None,
                watch: Some(path),
                context_id,
                metadata,
                params,
            } => {
                let path = path.canonicalize_utf8()?;
                let application_id =
                    install_app(&client, multiaddr, path.clone(), metadata.clone()).await?;
                let context_id = match context_id {
                    Some(context_id) => {
                        create_context(&client, multiaddr, application_id, Some(context_id), params)
                            .await?
                    }
                    None => {
                        create_context(&client, multiaddr, application_id, None, params).await?
                    }
                };
                watch_app_and_update_context(&client, multiaddr, context_id, path, metadata)
                    .await?;
            }
            _ => bail!("Invalid command configuration"),
>>>>>>> origin/master
        }

        Ok(())
    }
}

async fn create_context(
    client: &Client,
<<<<<<< HEAD
    keypair: Keypair,
) -> eyre::Result<()> {
    if !app_installed(&base_multiaddr, &application_id, client, &keypair).await? {
        eyre::bail!("Application is not installed on node.")
=======
    base_multiaddr: &Multiaddr,
    application_id: ApplicationId,
    context_id: Option<ContextId>,
    params: Option<String>,
) -> EyreResult<ContextId> {
    if !app_installed(base_multiaddr, &application_id, client).await? {
        bail!("Application is not installed on node.")
>>>>>>> origin/master
    }

    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/contexts")?;
    let request = CreateContextRequest::new(
        application_id,
        context_id,
        params.map(String::into_bytes).unwrap_or_default(),
    );

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
        let context_response: CreateContextResponse = response.json().await?;
        let context = context_response.data.context;

        println!("Context `\x1b[36m{}\x1b[0m` created!", context.id);

        println!(
            "Context{{\x1b[36m{}\x1b[0m}} -> Application{{\x1b[36m{}\x1b[0m}}",
            context.id, context.application_id
        );

        return Ok(context.id);
    }

    let status = response.status();
    let error_text = response.text().await?;

    bail!(
        "Request failed with status: {}. Error: {}",
        status,
        error_text
    );
}

async fn watch_app_and_update_context(
    client: &Client,
    base_multiaddr: &Multiaddr,
    context_id: ContextId,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
) -> EyreResult<()> {
    let (tx, mut rx) = mpsc::channel(1);

    let handle = Handle::current();
    let mut watcher = notify::recommended_watcher(move |evt| {
        handle.block_on(async {
            drop(tx.send(evt).await);
        });
    })?;

    watcher.watch(path.as_std_path(), RecursiveMode::NonRecursive)?;

    println!("(i) Watching for changes to \"\x1b[36m{path}\x1b[0m\"");

    while let Some(event) = rx.recv().await {
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                eprintln!("\x1b[1mERROR\x1b[0m: {err:?}");
                continue;
            }
        };

        match event.kind {
            EventKind::Modify(ModifyKind::Data(_)) => {}
            EventKind::Remove(_) => {
                eprintln!("\x1b[33mWARN\x1b[0m: file removed, ignoring..");
                continue;
            }
            EventKind::Any
            | EventKind::Access(_)
            | EventKind::Create(_)
            | EventKind::Modify(_)
            | EventKind::Other => continue,
        }

        let application_id =
            install_app(client, base_multiaddr, path.clone(), metadata.clone()).await?;

        update_context_application(client, base_multiaddr, context_id, application_id).await?;
    }

    Ok(())
}

async fn update_context_application(
    client: &Client,
    base_multiaddr: &Multiaddr,
    context_id: ContextId,
    application_id: ApplicationId,
) -> EyreResult<()> {
    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/contexts/{context_id}/application"),
    )?;

    let request = UpdateContextApplicationRequest::new(application_id);

    let response = client.post(url).json(&request).send().await?;

    if response.status().is_success() {
        println!(
            "Context{{\x1b[36m{context_id}\x1b[0m}} -> Application{{\x1b[36m{application_id}\x1b[0m}}"
        );

        return Ok(());
    }

    let status = response.status();
    let error_text = response.text().await?;

    bail!(
        "Request failed with status: {}. Error: {}",
        status,
        error_text
    );
}

async fn app_installed(
    base_multiaddr: &Multiaddr,
    application_id: &ApplicationId,
    client: &Client,
<<<<<<< HEAD
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
=======
) -> EyreResult<bool> {
    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/application/{application_id}"),
    )?;
    let response = client.get(url).send().await?;
>>>>>>> origin/master

    if !response.status().is_success() {
        bail!("Request failed with status: {}", response.status())
    }

    let api_response: GetApplicationResponse = response.json().await?;

    Ok(api_response.data.application.is_some())
}

async fn install_app(
    client: &Client,
    base_multiaddr: &Multiaddr,
    path: Utf8PathBuf,
<<<<<<< HEAD
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
=======
    metadata: Option<Vec<u8>>,
) -> EyreResult<ApplicationId> {
    let install_url = multiaddr_to_url(base_multiaddr, "admin-api/dev/install-application")?;

    let install_request =
        InstallDevApplicationRequest::new(path, None, metadata.unwrap_or_default());
>>>>>>> origin/master

    let timestamp = Utc::now().timestamp().to_string();
    let signature = keypair.sign(timestamp.as_bytes())?;

    let response = client
        .post(url)
        .header("X-Signature", hex::encode(signature))
        .header("X-Timestamp", timestamp)
        .json(&request)
        .send()
        .await?;

<<<<<<< HEAD
    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await?;
        eyre::bail!(
=======
    if !install_response.status().is_success() {
        let status = install_response.status();
        let error_text = install_response.text().await?;
        bail!(
>>>>>>> origin/master
            "Application installation failed with status: {}. Error: {}",
            status,
            error_text
        )
    }

    let response = install_response
        .json::<InstallApplicationResponse>()
        .await?;

<<<<<<< HEAD
    create_context(base_multiaddr, application_id, &client, keypair).await?;
=======
    println!(
        "Application `\x1b[36m{}\x1b[0m` installed!",
        response.data.application_id
    );
>>>>>>> origin/master

    Ok(response.data.application_id)
}
