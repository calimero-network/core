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
use eyre::{bail, OptionExt, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::cli::Environment;
use crate::common::resolve_alias;
use crate::connection::ConnectionInfo;
use crate::output::{ErrorLine, InfoLine};

#[derive(Debug, Parser)]
#[command(about = "Update app in context")]
pub struct UpdateCommand {
    #[clap(
        long,
        short = 'c',
        help = "Context to update",
        default_value = "default"
    )]
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

    #[arg(
        long = "as",
        help = "Public key of the executor",
        default_value = "default"
    )]
    pub executor: Alias<PublicKey>,
}

impl UpdateCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let connection = environment.connection()?;

        let context_id = resolve_alias(connection, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let executor_id = resolve_alias(connection, self.executor, Some(context_id))
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
                    connection,
                    context_id,
                    application_id,
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

                let application_id =
                    install_app(environment, connection, path.clone(), metadata.clone()).await?;

                update_context_application(
                    environment,
                    connection,
                    context_id,
                    application_id,
                    executor_id,
                )
                .await?;

                if self.watch {
                    watch_app_and_update_context(
                        environment,
                        connection,
                        context_id,
                        path,
                        metadata,
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
    connection: &ConnectionInfo,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
) -> Result<ApplicationId> {
    let request = InstallDevApplicationRequest::new(path, metadata.unwrap_or_default());

    let response: InstallApplicationResponse = connection
        .post("admin-api/install-dev-application", request)
        .await?;

    environment.output.write(&response);

    Ok(response.data.application_id)
}

async fn update_context_application(
    environment: &Environment,
    connection: &ConnectionInfo,
    context_id: ContextId,
    application_id: ApplicationId,
    member_public_key: PublicKey,
) -> Result<()> {
    let request = UpdateContextApplicationRequest::new(application_id, member_public_key);

    let response: UpdateContextApplicationResponse = connection
        .post(
            &format!("admin-api/contexts/{}/application", context_id),
            request,
        )
        .await?;

    environment.output.write(&response);

    Ok(())
}

async fn watch_app_and_update_context(
    environment: &Environment,
    connection: &ConnectionInfo,
    context_id: ContextId,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
    member_public_key: PublicKey,
) -> Result<()> {
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

        let application_id =
            install_app(environment, connection, path.clone(), metadata.clone()).await?;

        update_context_application(
            environment,
            connection,
            context_id,
            application_id,
            member_public_key,
        )
        .await?;
    }

    Ok(())
}
