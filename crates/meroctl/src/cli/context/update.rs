use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    InstallApplicationResponse, InstallDevApplicationRequest, UpdateContextApplicationRequest,
    UpdateContextApplicationResponse,
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

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::identity::open_identity;
use crate::output::{ErrorLine, InfoLine};

#[derive(Debug, Parser)]
#[command(about = "Update app in context")]
pub struct UpdateCommand {
    #[clap(long, short = 'c', help = "ContextId where to install the application")]
    context_id: ContextId,

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

    #[arg(
        long = "as",
        help = "Public key of the executor",
        conflicts_with = "identity_name"
    )]
    pub executor: Option<PublicKey>,

    #[clap(
        short = 'i',
        long,
        value_name = "IDENTITY_NAME",
        help = "Name of the identity which you want to use as executor"
    )]
    identity_name: Option<String>,
}

impl UpdateCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        match self {
            Self {
                context_id,
                application_id: Some(application_id),
                path: None,
                metadata: None,
                watch: false,
                executor: executor_public_key,
                identity_name,
            } => {
                update_context_application(
                    environment,
                    &client,
                    multiaddr,
                    context_id,
                    application_id,
                    &config.identity,
                    executor_public_key,
                    &identity_name,
                )
                .await?;
            }
            Self {
                context_id,
                application_id: None,
                path: Some(path),
                metadata,
                executor: executor_public_key,
                identity_name,
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
                    executor_public_key,
                    &identity_name,
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
                        executor_public_key,
                        &identity_name,
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
    member_public_key: Option<PublicKey>,
    identity_name: &Option<String>,
) -> EyreResult<()> {
    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/contexts/{context_id}/application"),
    )?;

    let public_key = match member_public_key {
        Some(public_key) => public_key,
        None => open_identity(environment, identity_name.as_ref().unwrap())?.public_key,
    };

    let request = UpdateContextApplicationRequest::new(application_id, public_key);

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
    member_public_key: Option<PublicKey>,
    identity_name: &Option<String>,
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
            &identity_name,
        )
        .await?;
    }

    Ok(())
}
