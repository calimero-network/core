#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "Acceptable for CLI"
)]

use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    CreateContextRequest, CreateContextResponse, GetApplicationResponse,
    InstallApplicationResponse, InstallDevApplicationRequest, UpdateContextApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use reqwest::Client;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::cli::RootArgs;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Debug, Parser)]
pub struct CreateCommand {
    /// The application ID to attach to the context
    #[clap(long, short = 'a')]
    application_id: Option<ApplicationId>,

    #[clap(long, short = 'p')]
    params: Option<String>,

    /// Path to the application file to watch and install locally
    #[clap(long, short = 'w', conflicts_with = "application_id")]
    watch: Option<Utf8PathBuf>,

    #[clap(requires = "watch")]
    metadata: Option<String>,

    #[clap(short = 's', long = "seed")]
    context_seed: Option<Hash>,
}

impl CreateCommand {
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let config = load_config(&args.home, &args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        match self {
            Self {
                application_id: Some(app_id),
                watch: None,
                context_seed,
                metadata: None,
                params,
            } => {
                let _ = create_context(
                    &client,
                    &multiaddr,
                    context_seed,
                    app_id,
                    params,
                    &config.identity,
                )
                .await?;
            }
            Self {
                application_id: None,
                watch: Some(path),
                context_seed,
                metadata,
                params,
            } => {
                let path = path.canonicalize_utf8()?;
                let metadata = metadata.map(String::into_bytes);

                let application_id = install_app(
                    &client,
                    &&multiaddr,
                    path.clone(),
                    metadata.clone(),
                    &config.identity,
                )
                .await?;

                let context_id = create_context(
                    &client,
                    &&multiaddr,
                    context_seed,
                    application_id,
                    params,
                    &config.identity,
                )
                .await?;

                watch_app_and_update_context(
                    &client,
                    &&multiaddr,
                    context_id,
                    path,
                    metadata,
                    &config.identity,
                )
                .await?;
            }
            _ => bail!("Invalid command configuration"),
        }

        Ok(())
    }
}

async fn create_context(
    client: &Client,
    base_multiaddr: &Multiaddr,
    context_seed: Option<Hash>,
    application_id: ApplicationId,
    params: Option<String>,
    keypair: &Keypair,
) -> EyreResult<ContextId> {
    if !app_installed(base_multiaddr, &application_id, client, keypair).await? {
        bail!("Application is not installed on node.")
    }

    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/contexts")?;
    let request = CreateContextRequest::new(
        application_id,
        context_seed,
        params.map(String::into_bytes).unwrap_or_default(),
    );

    let response = get_response(client, url, Some(request), keypair, RequestType::Post).await?;

    if response.status().is_success() {
        let context_response: CreateContextResponse = response.json().await?;

        let context_id = context_response.data.context_id;

        println!("Context `\x1b[36m{context_id}\x1b[0m` created!");

        println!(
            "Context{{\x1b[36m{context_id}\x1b[0m}} -> Application{{\x1b[36m{application_id}\x1b[0m}}",
        );

        return Ok(context_id);
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
    keypair: &Keypair,
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

        let application_id = install_app(
            client,
            base_multiaddr,
            path.clone(),
            metadata.clone(),
            keypair,
        )
        .await?;

        update_context_application(client, base_multiaddr, context_id, application_id, keypair)
            .await?;
    }

    Ok(())
}

async fn update_context_application(
    client: &Client,
    base_multiaddr: &Multiaddr,
    context_id: ContextId,
    application_id: ApplicationId,
    keypair: &Keypair,
) -> EyreResult<()> {
    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/contexts/{context_id}/application"),
    )?;

    let request = UpdateContextApplicationRequest::new(application_id);

    let response = get_response(client, url, Some(request), keypair, RequestType::Post).await?;

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
    keypair: &Keypair,
) -> eyre::Result<bool> {
    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/application/{application_id}"),
    )?;

    let response = get_response(client, url, None::<()>, keypair, RequestType::Get).await?;

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
    metadata: Option<Vec<u8>>,
    keypair: &Keypair,
) -> EyreResult<ApplicationId> {
    let install_url = multiaddr_to_url(base_multiaddr, "admin-api/dev/install-dev-application")?;

    let install_request = InstallDevApplicationRequest::new(path, metadata.unwrap_or_default());

    let install_response = get_response(
        client,
        install_url,
        Some(install_request),
        keypair,
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

    let response = install_response
        .json::<InstallApplicationResponse>()
        .await?;

    println!(
        "Application `\x1b[36m{}\x1b[0m` installed!",
        response.data.application_id
    );

    Ok(response.data.application_id)
}
