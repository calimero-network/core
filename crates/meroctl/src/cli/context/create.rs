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
}

impl Report for CreateContextResponse {
    fn report(&self) {
        println!("id: {}", self.data.context_id);
        println!("member_public_key: {}", self.data.member_public_key);
    }
}

impl Report for UpdateContextApplicationResponse {
    fn report(&self) {
        println!("Context application updated");
    }
}

impl CreateCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
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
            } => {
                let _ = create_context(
                    environment,
                    &client,
                    multiaddr,
                    context_seed,
                    app_id,
                    params,
                    &config.identity,
                    protocol,
                    identity,
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
            } => {
                let path = path.canonicalize_utf8()?;
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

                let (context_id, member_public_key) = create_context(
                    environment,
                    &client,
                    multiaddr,
                    context_seed,
                    application_id,
                    params,
                    &config.identity,
                    protocol,
                    identity,
                )
                .await?;

                watch_app_and_update_context(
                    environment,
                    &client,
                    multiaddr,
                    context_id,
                    path,
                    metadata,
                    &config.identity,
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
    base_multiaddr: &Multiaddr,
    context_seed: Option<Hash>,
    application_id: ApplicationId,
    params: Option<String>,
    keypair: &Keypair,
    protocol: String,
    identity: Option<Alias<PublicKey>>,
) -> EyreResult<(ContextId, PublicKey)> {
    if !app_installed(base_multiaddr, &application_id, client, keypair).await? {
        bail!("Application is not installed on node.")
    }

    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/contexts")?;
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

        let alias_url = multiaddr_to_url(
            base_multiaddr,
            &format!(
                "admin-api/dev/alias/create/identity/{}",
                response.data.context_id
            ),
        )?;

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

    Ok((response.data.context_id, response.data.member_public_key))
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

async fn app_installed(
    base_multiaddr: &Multiaddr,
    application_id: &ApplicationId,
    client: &Client,
    keypair: &Keypair,
) -> eyre::Result<bool> {
    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/applications/{application_id}"),
    )?;

    let response: GetApplicationResponse =
        do_request(client, url, None::<()>, keypair, RequestType::Get).await?;

    Ok(response.data.application.is_some())
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
