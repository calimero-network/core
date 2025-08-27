use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    CreateAliasRequest, CreateAliasResponse, CreateContextIdentityAlias, CreateContextRequest,
    CreateContextResponse, GetApplicationResponse, InstallDevApplicationRequest,
    UpdateContextApplicationRequest, UpdateContextApplicationResponse,
};
use camino::Utf8PathBuf;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{bail, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::cli::Environment;
use crate::client::Client;
use crate::output::{ErrorLine, InfoLine, Report};

#[derive(Debug, Parser)]
#[command(about = "Create a new context")]
pub struct CreateCommand {
    #[clap(
        long,
        short = 'a',
        help = "The application ID to attach to the context"
    )]
    pub application_id: Option<ApplicationId>,

    #[clap(
        long,
        short = 'p',
        help = "The parameters to pass to the application initialization function"
    )]
    pub params: Option<String>,

    #[clap(
        long,
        short = 'w',
        conflicts_with = "application_id",
        help = "Path to the application file to watch and install locally"
    )]
    pub watch: Option<Utf8PathBuf>,

    #[clap(
        requires = "watch",
        help = "Metadata needed for the application installation"
    )]
    pub metadata: Option<String>,

    #[clap(
        short = 's',
        long = "seed",
        help = "The seed for the random generation of the context id"
    )]
    pub context_seed: Option<Hash>,

    #[clap(long, value_name = "PROTOCOL")]
    pub protocol: String,

    #[clap(long = "as", help = "Create an alias for the context identity")]
    pub identity: Option<Alias<PublicKey>>,

    #[clap(long = "name", help = "Create an alias for the context")]
    pub context: Option<Alias<ContextId>>,
}

impl Report for CreateContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Context Created").fg(Color::Green)]);
        let _ = table.add_row(vec![format!("Context ID: {}", self.data.context_id)]);
        let _ = table.add_row(vec![format!(
            "Member Public Key: {}",
            self.data.member_public_key
        )]);
        println!("{table}");
    }
}

impl Report for UpdateContextApplicationResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Context Updated").fg(Color::Green)]);
        let _ = table.add_row(vec!["Application successfully updated"]);
        println!("{table}");
    }
}

impl CreateCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let client_clone = client.clone();

        match self {
            Self {
                application_id: Some(app_id),
                watch: None,
                context_seed,
                metadata: None,
                params,
                protocol,
                identity,
                context,
            } => {
                let _ = create_context(
                    environment,
                    &client_clone,
                    context_seed,
                    app_id,
                    params,
                    protocol,
                    identity,
                    context,
                )
                .await?;
            }
            Self {
                application_id: None,
                watch: Some(path),
                context_seed,
                metadata,
                params,
                protocol,
                identity,
                context,
            } => {
                let path = path.canonicalize_utf8()?;
                let metadata = metadata.map(String::into_bytes);
                let client = environment.client()?;
                let application_id = client
                    .install_dev_application(InstallDevApplicationRequest::new(
                        path.clone(),
                        metadata.clone().unwrap_or_default(),
                    ))
                    .await?
                    .data
                    .application_id;

                let (context_id, member_public_key) = create_context(
                    environment,
                    &client_clone,
                    context_seed,
                    application_id,
                    params,
                    protocol,
                    identity,
                    context,
                )
                .await?;

                watch_app_and_update_context(
                    environment,
                    &client_clone,
                    context_id,
                    path,
                    metadata,
                    member_public_key,
                )
                .await?;
            }
            _ => bail!("Invalid command configuration"),
        }

        Ok(())
    }
}

pub async fn create_context(
    environment: &mut Environment,
    client: &Client,
    context_seed: Option<Hash>,
    application_id: ApplicationId,
    params: Option<String>,
    protocol: String,
    identity: Option<Alias<PublicKey>>,
    context: Option<Alias<ContextId>>,
) -> Result<(ContextId, PublicKey)> {
    let response: GetApplicationResponse = client.get_application(&application_id).await?;

    if !response.data.application.is_some() {
        bail!("Application is not installed on node.")
    }

    let request = CreateContextRequest::new(
        protocol,
        application_id,
        context_seed,
        params.map(String::into_bytes).unwrap_or_default(),
    );

    let response: CreateContextResponse = client.create_context(request).await?;

    environment.output.write(&response);

    if let Some(alias) = identity {
        let alias_request = CreateAliasRequest {
            alias,
            value: CreateContextIdentityAlias {
                identity: response.data.member_public_key,
            },
        };

        let alias_response: CreateAliasResponse = client
            .create_context_identity_alias(&response.data.context_id, alias_request)
            .await?;

        environment.output.write(&alias_response);
    }
    if let Some(context_alias) = context {
        let res = client
            .create_alias(context_alias, Some(response.data.context_id))
            .await?;
        environment.output.write(&res);
    }
    Ok((response.data.context_id, response.data.member_public_key))
}

async fn watch_app_and_update_context(
    environment: &mut Environment,
    client: &Client,
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

        let client = environment.client()?;
        let application_id = client
            .install_dev_application(InstallDevApplicationRequest::new(
                path.clone(),
                metadata.clone().unwrap_or_default(),
            ))
            .await?
            .data
            .application_id;

        let request = UpdateContextApplicationRequest::new(application_id, member_public_key);
        let response: UpdateContextApplicationResponse = client
            .update_context_application(&context_id, request)
            .await?;
        environment.output.write(&response);
    }

    Ok(())
}
