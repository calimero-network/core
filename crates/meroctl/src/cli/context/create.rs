#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "Acceptable for CLI"
)]

use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_server::admin::handlers::context::UpdateApplicationIdResponse;
use calimero_server_primitives::admin::{
    CreateContextRequest, CreateContextResponse, GetApplicationResponse,
};
use camino::Utf8PathBuf;
use clap::Parser;
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use reqwest::Client;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use super::update::UpdateCommand;
use crate::cli::app::get::{GetCommand, GetValues};
use crate::cli::app::install::InstallCommand;
use crate::cli::RootArgs;
use crate::common::{
    fetch_multiaddr, get_response, load_config, multiaddr_to_url, CliError, RequestType,
};

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
    pub async fn run(self, args: RootArgs) -> Result<CreateContextResponse, CliError> {
        let config = load_config(&args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let context_response: CreateContextResponse;

        match self {
            Self {
                application_id: Some(app_id),
                watch: None,
                context_seed,
                metadata: None,
                params,
                ..
            } => {
                context_response = create_context(
                    &multiaddr,
                    context_seed,
                    app_id,
                    params,
                    &config.identity,
                    &args,
                )
                .await?;
            }
            Self {
                application_id: None,
                watch: Some(path),
                context_seed,
                metadata,
                params,
                ..
            } => {
                let path = path
                    .canonicalize_utf8()
                    .map_err(|_| CliError::InternalError(format!("Canonicalize path failed")))?;

                let application_id = install_app(path.clone(), &metadata, &args).await?;

                context_response = create_context(
                    &&multiaddr,
                    context_seed,
                    application_id,
                    params,
                    &config.identity,
                    &args,
                )
                .await?;

                watch_app_and_update_context(
                    context_response.data.context_id,
                    path,
                    metadata,
                    &args,
                )
                .await?;
            }
            _ => {
                return Err(CliError::InternalError(format!(
                    "Invalid command configuration"
                )))
            }
        }

        Ok(context_response)
    }
}

async fn create_context(
    base_multiaddr: &Multiaddr,
    context_seed: Option<Hash>,
    application_id: ApplicationId,
    params: Option<String>,
    keypair: &Keypair,
    args: &RootArgs,
) -> Result<CreateContextResponse, CliError> {
    if app_installed(&application_id, &args).await.is_err() {
        return Err(CliError::InternalError(format!(
            "Application is not installed on node."
        )));
    }

    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/contexts")?;
    let request = CreateContextRequest::new(
        application_id,
        context_seed,
        params.map(String::into_bytes).unwrap_or_default(),
    );

    let response = get_response(
        &Client::new(),
        url,
        Some(request),
        keypair,
        RequestType::Post,
    )
    .await?;

    if !response.status().is_success() {
        return Err(CliError::MethodCallError(format!(
            "Create context request failed with status: {}",
            response.status()
        )));
    }

    let body = response
        .json::<CreateContextResponse>()
        .await
        .map_err(|e| CliError::MethodCallError(e.to_string()))?;

    Ok(body)
}

async fn watch_app_and_update_context(
    context_id: ContextId,
    path: Utf8PathBuf,
    metadata: Option<String>,
    args: &RootArgs,
) -> Result<(), CliError> {
    let (tx, mut rx) = mpsc::channel(1);

    let handle = Handle::current();
    let mut watcher = notify::recommended_watcher(move |evt| {
        handle.block_on(async {
            drop(tx.send(evt).await);
        });
    })
    .map_err(|err| CliError::InternalError(err.to_string()))?;

    watcher
        .watch(path.as_std_path(), RecursiveMode::NonRecursive)
        .map_err(|err| CliError::InternalError(err.to_string()))?;

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

        let application_id = install_app(path.clone(), &metadata, &args).await?;

        update_context_application(&args, context_id, application_id).await?;
    }

    Ok(())
}

async fn update_context_application(
    args: &RootArgs,
    context_id: ContextId,
    application_id: ApplicationId,
) -> Result<UpdateApplicationIdResponse, CliError> {
    let update = UpdateCommand {
        context_id,
        application_id,
    }
    .run(&args)
    .await?;

    Ok(update)
}

async fn app_installed(
    application_id: &ApplicationId,
    args: &RootArgs,
) -> Result<GetApplicationResponse, CliError> {
    let app_get = GetCommand {
        method: GetValues::Details,
        app_id: application_id.to_string(),
    }
    .run(args)
    .await?;

    Ok(app_get)
}

async fn install_app(
    path: Utf8PathBuf,
    metadata: &Option<String>,
    args: &RootArgs,
) -> Result<ApplicationId, CliError> {
    let application_id = InstallCommand {
        path: Some(path),
        url: None,
        metadata: metadata.clone(),
        hash: None,
    }
    .run(args)
    .await?;

    Ok(application_id.data.application_id)
}
