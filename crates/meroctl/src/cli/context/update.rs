use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    InstallDevApplicationRequest, UpdateContextApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, OptionExt, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::cli::validation::validate_file_exists;
use crate::cli::Environment;
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

    #[arg(long, help = "Migration function name to execute during the update")]
    pub migrate_method: Option<String>,
}

impl UpdateCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let context_id = client
            .resolve_alias(self.context, None)
            .await?
            .value()
            .copied()
            .ok_or_eyre("unable to resolve")?;

        let executor_id = client
            .resolve_alias(self.executor, Some(context_id))
            .await?
            .value()
            .copied()
            .ok_or_eyre("unable to resolve")?;

        match self {
            Self {
                application_id: Some(application_id),
                path: None,
                metadata: None,
                watch: false,
                migrate_method,
                ..
            } => {
                let request = if let Some(method) = migrate_method {
                    UpdateContextApplicationRequest::with_migration(
                        application_id,
                        executor_id,
                        method,
                    )
                } else {
                    UpdateContextApplicationRequest::new(application_id, executor_id)
                };
                let _response = client
                    .update_context_application(&context_id, request)
                    .await?;
                environment.output.write(&_response);
            }
            Self {
                application_id: None,
                path: Some(path),
                metadata,
                migrate_method,
                ..
            } => {
                // Validate file exists before processing
                validate_file_exists(path.as_std_path())?;

                let metadata = metadata.map(String::into_bytes);

                let application_id = client
                    .install_dev_application(InstallDevApplicationRequest::new(
                        path.clone(),
                        metadata.clone().unwrap_or_default(),
                        Some("unknown".to_owned()),
                        Some("0.0.0".to_owned()),
                    ))
                    .await?
                    .data
                    .application_id;

                let request = if let Some(method) = migrate_method.clone() {
                    UpdateContextApplicationRequest::with_migration(
                        application_id,
                        executor_id,
                        method,
                    )
                } else {
                    UpdateContextApplicationRequest::new(application_id, executor_id)
                };
                let _response = client
                    .update_context_application(&context_id, request)
                    .await?;
                environment.output.write(&_response);

                if self.watch {
                    watch_app_and_update_context(
                        environment,
                        context_id,
                        path,
                        metadata,
                        executor_id,
                        migrate_method,
                    )
                    .await?;
                }
            }

            _ => bail!("Invalid command configuration"),
        }

        Ok(())
    }
}

async fn watch_app_and_update_context(
    environment: &mut Environment,
    context_id: ContextId,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
    member_public_key: PublicKey,
    migrate_method: Option<String>,
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

        let client = environment.client()?;
        let application_id = client
            .install_dev_application(InstallDevApplicationRequest::new(
                path.clone(),
                metadata.clone().unwrap_or_default(),
                Some("unknown".to_owned()),
                Some("0.0.0".to_owned()),
            ))
            .await?
            .data
            .application_id;

        let client = environment.client()?;
        let request = if let Some(ref method) = migrate_method {
            UpdateContextApplicationRequest::with_migration(
                application_id,
                member_public_key,
                method.clone(),
            )
        } else {
            UpdateContextApplicationRequest::new(application_id, member_public_key)
        };
        let response = client
            .update_context_application(&context_id, request)
            .await?;
        environment.output.write(&response);
    }

    Ok(())
}
