use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    InstallApplicationResponse, InstallDevApplicationRequest, UpdateContextApplicationRequest,
    UpdateContextApplicationResponse,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, OptionExt, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use reqwest::Client;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::cli::Environment;
use crate::common::{
    do_request, fetch_multiaddr, load_config, multiaddr_to_url, resolve_alias, RequestType,
};
use crate::output::{ErrorLine, InfoLine};

#[derive(Debug, Parser)]
#[command(about = "Update app in context")]
pub struct UpdateCommand {
    #[clap(long, short = 'c', help = "Context to update")]
    context: Alias<ContextId>,

    #[clap(
        long,
        short = 'a',
        help = "The application ID to update in the context"
    )]
    application_id: Option<ApplicationId>,

    #[clap(
        long,
        conflicts_with = "application_id",
        help = "Path to the application file to watch and install locally"
    )]
    path: Option<Utf8PathBuf>,

    #[clap(
        long,
        conflicts_with = "application_id",
        help = "Metadata needed for the application installation"
    )]
    metadata: Option<String>,

    #[clap(
        long,
        short = 'w',
        conflicts_with = "application_id",
        requires = "path"
    )]
    watch: bool,

    #[arg(long = "as", help = "Public key of the executor")]
    pub executor: Alias<PublicKey>,
}

impl UpdateCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let executor_id = resolve_alias(multiaddr, &config.identity, self.executor, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        match self {
            Self {
                application_id: Some(application_id),
                path: None,
                metadata: None,
                watch: false,
                ..
            } => {
                update_context_application(
                    environment,
                    &client,
                    multiaddr,
                    context_id,
                    application_id,
                    &config.identity,
                    executor_id,
                )
                .await?;
            }
            Self {
                application_id: None,
                path: Some(path),
                metadata,
                ..
            } => {
                let metadata = metadata.map(String::into_bytes);

                let application_id = install_app(
                    environment,
                    &client,
                    multiaddr,
                    path.clone(),
                    metadata.clone(),
                    &config.identity,
                )
                .await?;

                update_context_application(
                    environment,
                    &client,
                    multiaddr,
                    context_id,
                    application_id,
                    &config.identity,
                    executor_id,
                )
                .await?;

                if self.watch {
                    watch_app_and_update_context(
                        environment,
                        &client,
                        multiaddr,
                        context_id,
                        path,
                        metadata,
                        &config.identity,
                        executor_id,
                    )
                    .await?;
                }
            }

            _ => bail!("Invalid command configuration"),
        }

        Ok(())
    }
}

async fn install_app(
    environment: &Environment,
    client: &Client,
    base_multiaddr: &Multiaddr,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
    keypair: &Keypair,
) -> EyreResult<ApplicationId> {
    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/install-dev-application")?;

    let request = InstallDevApplicationRequest::new(path, metadata.unwrap_or_default());

    let response: InstallApplicationResponse =
        do_request(client, url, Some(request), keypair, RequestType::Post).await?;

    environment.output.write(&response);

    Ok(response.data.application_id)
}

async fn update_context_application(
    environment: &Environment,
    client: &Client,
    base_multiaddr: &Multiaddr,
    context_id: ContextId,
    application_id: ApplicationId,
    keypair: &Keypair,
    member_public_key: PublicKey,
) -> EyreResult<()> {
    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/contexts/{context_id}/application"),
    )?;

    let request = UpdateContextApplicationRequest::new(application_id, member_public_key);

    let response: UpdateContextApplicationResponse =
        do_request(client, url, Some(request), keypair, RequestType::Post).await?;

    environment.output.write(&response);

    Ok(())
}

async fn watch_app_and_update_context(
    environment: &Environment,
    client: &Client,
    base_multiaddr: &Multiaddr,
    context_id: ContextId,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
    keypair: &Keypair,
    member_public_key: PublicKey,
) -> EyreResult<()> {
    let (tx, mut rx) = mpsc::channel(1);

    let handle = Handle::current();
    let mut watcher = notify::recommended_watcher(move |evt| {
        handle.block_on(async {
            drop(tx.send(evt).await);
        });
    })?;

    watcher.watch(path.as_std_path(), RecursiveMode::NonRecursive)?;

    environment
        .output
        .write(&InfoLine(&format!("Watching for changes to {path}")));

    while let Some(event) = rx.recv().await {
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                environment.output.write(&ErrorLine(&format!("{err:?}")));
                continue;
            }
        };

        match event.kind {
            EventKind::Modify(ModifyKind::Data(_)) => {}
            EventKind::Remove(_) => {
                environment
                    .output
                    .write(&ErrorLine("File removed, ignoring.."));
                continue;
            }
            EventKind::Any
            | EventKind::Access(_)
            | EventKind::Create(_)
            | EventKind::Modify(_)
            | EventKind::Other => continue,
        }

        let application_id = install_app(
            environment,
            client,
            base_multiaddr,
            path.clone(),
            metadata.clone(),
            keypair,
        )
        .await?;

        update_context_application(
            environment,
            client,
            base_multiaddr,
            context_id,
            application_id,
            keypair,
            member_public_key,
        )
        .await?;
    }

    Ok(())
}
