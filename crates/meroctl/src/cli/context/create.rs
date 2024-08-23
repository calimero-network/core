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

use crate::cli::RootArgs;
use crate::common::multiaddr_to_url;
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct CreateCommand {
    /// The application ID to attach to the context
    #[clap(long, short = 'a', exclusive = true)]
    application_id: Option<ApplicationId>,

    /// Path to the application file to watch and install locally
    #[clap(long, short = 'w', exclusive = true)]
    watch: Option<Utf8PathBuf>,
    metadata: Option<Vec<u8>>,
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
                watch: None,
                metadata: _,
            } => {
                let _ = create_context(multiaddr, app_id, &client).await?;
            }
            Self {
                application_id: None,
                watch: Some(path),
                metadata,
            } => {
                let path = path.canonicalize_utf8()?;

                let application_id =
                    install_app(multiaddr, path.clone(), &client, metadata.clone()).await?;

                let context_id = create_context(multiaddr, application_id, &client).await?;

                watch_app_and_update_context(multiaddr, context_id, path, &client, metadata)
                    .await?;
            }
            _ => bail!("Invalid command configuration"),
        }

        Ok(())
    }
}

async fn create_context(
    base_multiaddr: &Multiaddr,
    application_id: ApplicationId,
    client: &Client,
) -> EyreResult<ContextId> {
    if !app_installed(base_multiaddr, &application_id, client).await? {
        bail!("Application is not installed on node.")
    }

    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/contexts")?;
    let request = CreateContextRequest::new(application_id);

    let response = client.post(url).json(&request).send().await?;

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
    base_multiaddr: &Multiaddr,
    context_id: ContextId,
    path: Utf8PathBuf,
    client: &Client,
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
            install_app(base_multiaddr, path.clone(), client, metadata.clone()).await?;

        update_context_application(base_multiaddr, context_id, application_id, client).await?;
    }

    Ok(())
}

async fn update_context_application(
    base_multiaddr: &Multiaddr,
    context_id: ContextId,
    application_id: ApplicationId,
    client: &Client,
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
) -> EyreResult<bool> {
    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/application/{application_id}"),
    )?;
    let response = client.get(url).send().await?;

    if !response.status().is_success() {
        bail!("Request failed with status: {}", response.status())
    }

    let api_response: GetApplicationResponse = response.json().await?;

    Ok(api_response.data.application.is_some())
}

async fn install_app(
    base_multiaddr: &Multiaddr,
    path: Utf8PathBuf,
    client: &Client,
    metadata: Option<Vec<u8>>,
) -> EyreResult<ApplicationId> {
    let install_url = multiaddr_to_url(base_multiaddr, "admin-api/dev/install-application")?;

    let install_request =
        InstallDevApplicationRequest::new(path, None, metadata.unwrap_or_default());

    let install_response = client
        .post(install_url)
        .json(&install_request)
        .send()
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

    let response = install_response
        .json::<InstallApplicationResponse>()
        .await?;

    println!(
        "Application `\x1b[36m{}\x1b[0m` installed!",
        response.data.application_id
    );

    Ok(response.data.application_id)
}
