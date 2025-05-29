use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    CreateAliasRequest, CreateAliasResponse, CreateContextIdentityAlias, CreateContextRequest,
    CreateContextResponse, GetApplicationResponse, InstallApplicationResponse,
    InstallDevApplicationRequest, UpdateContextApplicationRequest,
    UpdateContextApplicationResponse,
};
use camino::Utf8PathBuf;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::identity::Keypair;
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use reqwest::Client;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::cli::{ConnectionInfo, Environment};
use crate::common::{create_alias, do_request, RequestType};
use crate::output::{ErrorLine, InfoLine, Report};

#[derive(Debug, Parser)]
#[command(about = "Create a new context")]
pub struct CreateCommand {
    #[clap(
        long,
        short = 'a',
        help = "The application ID to attach to the context"
    )]
    application_id: Option<ApplicationId>,

    #[clap(
        long,
        short = 'p',
        help = "The parameters to pass to the application initialization function"
    )]
    params: Option<String>,

    #[clap(
        long,
        short = 'w',
        conflicts_with = "application_id",
        help = "Path to the application file to watch and install locally"
    )]
    watch: Option<Utf8PathBuf>,

    #[clap(
        requires = "watch",
        help = "Metadata needed for the application installation"
    )]
    metadata: Option<String>,

    #[clap(
        short = 's',
        long = "seed",
        help = "The seed for the random generation of the context id"
    )]
    context_seed: Option<Hash>,

    #[clap(long, value_name = "PROTOCOL")]
    protocol: String,

    #[clap(long = "as", help = "Create an alias for the context identity")]
    identity: Option<Alias<PublicKey>>,

    #[clap(long = "name", help = "Create an alias for the context")]
    context: Option<Alias<ContextId>>,
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
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let connection = environment
            .connection
            .as_ref()
            .ok_or_else(|| eyre!("No connection configured"))?;

        

        let client = Client::new();
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
                    &client,
                    connection,
                    context_seed,
                    app_id,
                    params,
                    connection.auth_key.as_ref(),
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
                let application_id = install_app(
                    environment,
                    &client,
                    connection,
                    path.clone(),
                    metadata.clone(),
                    connection.auth_key.as_ref(),
                )
                .await?;

                let (context_id, member_public_key) = create_context(
                    environment,
                    &client,
                    connection,
                    context_seed,
                    application_id,
                    params,
                    connection.auth_key.as_ref(),
                    protocol,
                    identity,
                    context,
                )
                .await?;

                watch_app_and_update_context(
                    environment,
                    &client,
                    connection,
                    context_id,
                    path,
                    metadata,
                    connection.auth_key.as_ref(),
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
    environment: &Environment,
    client: &Client,
    connection: &ConnectionInfo,
    context_seed: Option<Hash>,
    application_id: ApplicationId,
    params: Option<String>,
    keypair: Option<&Keypair>,
    protocol: String,
    identity: Option<Alias<PublicKey>>,
    context: Option<Alias<ContextId>>,
) -> EyreResult<(ContextId, PublicKey)> {
    if !app_installed(connection, &application_id, client, keypair).await? {
        bail!("Application is not installed on node.")
    }

    let mut url = connection.api_url.clone();
    url.set_path("admin-api/dev/contexts");

    let request = CreateContextRequest::new(
        protocol,
        application_id,
        context_seed,
        params.map(String::into_bytes).unwrap_or_default(),
    );

    let response: CreateContextResponse =
        do_request(client, url, Some(request), keypair, RequestType::Post).await?;

    environment.output.write(&response);

    if let Some(alias) = identity {
        let alias_request = CreateAliasRequest {
            alias,
            value: CreateContextIdentityAlias {
                identity: response.data.member_public_key,
            },
        };

        let mut alias_url = connection.api_url.clone();
        alias_url.set_path(&format!(
            "admin-api/dev/alias/create/identity/{}",
            response.data.context_id
        ));

        let alias_response: CreateAliasResponse = do_request(
            client,
            alias_url,
            Some(alias_request),
            keypair,
            RequestType::Post,
        )
        .await?;

        environment.output.write(&alias_response);
    }
    if let Some(context_alias) = context {
        let res = create_alias(
            &connection.api_url,
            keypair.unwrap(),
            context_alias,
            None,
            response.data.context_id,
        )
        .await?;
        environment.output.write(&res);
    }
    Ok((response.data.context_id, response.data.member_public_key))
}

async fn watch_app_and_update_context(
    environment: &Environment,
    client: &Client,
    connection: &ConnectionInfo,
    context_id: ContextId,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
    keypair: Option<&Keypair>,
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
            connection,
            path.clone(),
            metadata.clone(),
            keypair,
        )
        .await?;

        update_context_application(
            environment,
            client,
            connection,
            context_id,
            application_id,
            keypair,
            member_public_key,
        )
        .await?;
    }

    Ok(())
}

async fn update_context_application(
    environment: &Environment,
    client: &Client,
    connection: &ConnectionInfo,
    context_id: ContextId,
    application_id: ApplicationId,
    keypair: Option<&Keypair>,
    member_public_key: PublicKey,
) -> EyreResult<()> {
    let mut url = connection.api_url.clone();
    url.set_path(&format!("admin-api/dev/contexts/{}", context_id));

    let request = UpdateContextApplicationRequest::new(application_id, member_public_key);

    let response: UpdateContextApplicationResponse =
        do_request(client, url, Some(request), keypair, RequestType::Post).await?;

    environment.output.write(&response);

    Ok(())
}

async fn app_installed(
    connection: &ConnectionInfo,
    application_id: &ApplicationId,
    client: &Client,
    keypair: Option<&Keypair>,
) -> eyre::Result<bool> {
    let mut url = connection.api_url.clone();
    url.set_path(&format!("admin-api/dev/applications/{application_id}"));

    let response: GetApplicationResponse =
        do_request(client, url, None::<()>, keypair, RequestType::Get).await?;

    Ok(response.data.application.is_some())
}

async fn install_app(
    environment: &Environment,
    client: &Client,
    connection: &ConnectionInfo,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
    keypair: Option<&Keypair>,
) -> EyreResult<ApplicationId> {
    let mut url = connection.api_url.clone();
    url.set_path("admin-api/dev/install-dev-application");

    let request = InstallDevApplicationRequest::new(path, metadata.unwrap_or_default());

    let response: InstallApplicationResponse =
        do_request(client, url, Some(request), keypair, RequestType::Post).await?;

    environment.output.write(&response);

    Ok(response.data.application_id)
}
